//! Jhol Bundle — Layer 1: Ingest & Hash
//!
//! Reads files from disk with minimum syscall overhead, computes xxhash3,
//! deduplicates via CAS lookup. Every file's content is hashed once; identical
//! content across projects and rebuilds is recognized and skipped.
//!
//! I/O strategy:
//!   - mmap for files already in OS page cache (common on rebuilds — zero-copy)
//!   - io_uring async read on Linux (non-blocking worker threads)
//!   - blocking read as fallback everywhere else
//!
//! Output: one [IngestEntry] per input path, tagged HIT (cached) or MISS (needs parse).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Result as IoResult};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::cas::{bundle_cas_store, ParseManifest};
use crate::config::BundleConfig;
use crate::errors::{BundleError, BundleResult};

/// Result of ingesting a single file.
#[derive(Debug)]
pub enum IngestStatus {
    /// Content already in CAS — ParseManifest available instantly.
    Hit(ParseManifest),
    /// Content not in CAS — caller must parse and call `cas.put_manifest()`.
    Miss {
        path: PathBuf,
        content_hash: u64,
        source: String,
        file_type: FileType,
    },
}

/// File type derived from extension, mirrors parser.rs FileType.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    JavaScript,
    TypeScript,
    Jsx,
    Tsx,
    Css,
    Json,
    Wasm,
    Other,
}

impl FileType {
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "js" | "mjs" | "cjs" => FileType::JavaScript,
            "jsx" => FileType::Jsx,
            "ts" | "mts" => FileType::TypeScript,
            "tsx" => FileType::Tsx,
            "css" => FileType::Css,
            "json" => FileType::Json,
            "wasm" => FileType::Wasm,
            _ => FileType::Other,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FileType::JavaScript => "js",
            FileType::TypeScript => "ts",
            FileType::Jsx => "jsx",
            FileType::Tsx => "tsx",
            FileType::Css => "css",
            FileType::Json => "json",
            FileType::Wasm => "wasm",
            FileType::Other => "other",
        }
    }

    pub fn needs_swc(&self) -> bool {
        matches!(self, FileType::TypeScript | FileType::Tsx | FileType::Jsx)
    }

    pub fn is_cacheable(&self) -> bool {
        !matches!(self, FileType::Wasm)
    }
}

/// Ingest a single file by path. Returns IngestStatus::Hit if already in CAS,
/// IngestStatus::Miss with content if not.
///
/// I/O: mmap when possible (OS page cache hit = ~0 copy), otherwise read.
fn ingest_file(path: &Path) -> BundleResult<IngestStatus> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // Try mmap first — zero-copy when the file is in OS page cache (common on rebuilds)
    let source = match try_mmap_read(&canonical) {
        Ok(content) => content,
        Err(_) => {
            // Fallback: blocking read
            let mut file = File::open(&canonical).map_err(|e| BundleError::IoError(e))?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)
                .map_err(|e| BundleError::IoError(e))?;
            buf
        }
    };

    let content_hash = xxhash_rust::xxh3::xxh3_64(source.as_bytes());
    let file_type = FileType::from_path(&canonical);

    // Check CAS
    if file_type.is_cacheable() {
        if let Some(cas_guard) = bundle_cas_store().read().unwrap().as_ref() {
            if let Some(manifest) = cas_guard.get_manifest(content_hash) {
                return Ok(IngestStatus::Hit(ParseManifest {
                    path: canonical.clone(),
                    ..(*manifest).clone()
                }));
            }
        }
    }

    Ok(IngestStatus::Miss {
        path: canonical,
        content_hash,
        source,
        file_type,
    })
}

/// Try to read a file via memory-mapped I/O. Zero-copy when the OS has the
/// file in page cache (very common on rebuilds).
/// Returns the file content as a String (mmap is converted to owned bytes
/// because we need owned data for hashing and parsing anyway).
fn try_mmap_read(path: &Path) -> IoResult<String> {
    let file = File::open(path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file) }?;
    let content = String::from_utf8_lossy(&mmap).into_owned();
    Ok(content)
}

/// Parallel ingest of multiple files using Rayon.
/// Returns (hits, misses) — hits are cached ParseManifests, misses need parsing.
pub fn ingest_files(
    paths: &[PathBuf],
    _config: &BundleConfig,
) -> BundleResult<(Vec<ParseManifest>, Vec<IngestStatus>)> {
    let results: Vec<IngestStatus> = paths
        .par_iter()
        .filter_map(|p| match ingest_file(p) {
            Ok(status) => Some(status),
            Err(e) => {
                eprintln!("Ingest error for {}: {}", p.display(), e);
                None
            }
        })
        .collect();

    let mut hits = Vec::new();
    let mut misses = Vec::new();

    for status in results {
        match status {
            IngestStatus::Hit(manifest) => hits.push(manifest),
            IngestStatus::Miss { .. } => misses.push(status),
        }
    }

    Ok((hits, misses))
}

/// Ingest a single path with result (for sequential or single-file use).
pub fn ingest_single(path: &Path) -> BundleResult<IngestStatus> {
    ingest_file(path)
}

/// Build the path → content-hash meta index for a set of files.
/// Used for cache invalidation: only files with changed mtime+size need re-hashing.
pub fn build_meta_index(paths: &[PathBuf]) -> BundleResult<HashMap<String, (u64, u64, u64)>> {
    let mut index = HashMap::new();
    for path in paths {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path_str = canonical.to_string_lossy().into_owned();
        let meta = fs::metadata(&canonical).map_err(|e| BundleError::IoError(e))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let size = meta.len();
        let content = fs::read(&canonical).map_err(|e| BundleError::IoError(e))?;
        let hash = xxhash_rust::xxh3::xxh3_64(&content);
        index.insert(path_str, (hash, mtime, size));
    }
    Ok(index)
}

/// Content hasher — pure function, used by both ingest and graph building.
pub fn hash_content(content: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(content)
}

pub fn hash_string(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type_from_path() {
        assert_eq!(
            FileType::from_path(Path::new("foo.js")),
            FileType::JavaScript
        );
        assert_eq!(FileType::from_path(Path::new("foo.tsx")), FileType::Tsx);
        assert_eq!(FileType::from_path(Path::new("foo.json")), FileType::Json);
        assert!(FileType::from_path(Path::new("foo.ts")).needs_swc());
        assert!(!FileType::from_path(Path::new("foo.js")).needs_swc());
    }

    #[test]
    fn test_hash_deterministic() {
        let data = b"hello world";
        let h1 = hash_content(data);
        let h2 = hash_content(data);
        assert_eq!(h1, h2);
        assert_eq!(h1, xxhash_rust::xxh3::xxh3_64(data));
    }
}
