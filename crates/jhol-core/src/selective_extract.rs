//! Feature-sheaf selective extract — Soul Path quality plane.
//!
//! Default stalks: legal + types + runtime + bin.
//! Store stays full; project may project a sheaf section.
//! LICENSE / *.d.ts always kept on default path.

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tar::Archive;

use crate::utils;

const MAX_PRE_MANIFEST_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PRE_MANIFEST_ENTRIES: usize = 4096;

/// Feature stalks for selective project materialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureSheaf {
    /// legal + types + runtime + bin (default)
    Default,
    /// everything
    Full,
    /// runtime + bin only
    Minimal,
}

impl FeatureSheaf {
    pub fn from_env() -> Self {
        match std::env::var("JHOL_EXTRACT_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "full" | "strict" => Self::Full,
            "minimal" | "min" => Self::Minimal,
            _ => Self::Default,
        }
    }
}

/// Always keep (legal + types + core).
const LEGAL_TYPES_CORE: &[&str] = &[
    "package.json",
    "license",
    "license.md",
    "license.txt",
    "licence",
    "copying",
    "notice",
    "notice.md",
];

/// Runtime / bin directory stalks.
const RUNTIME_DIRS: &[&str] = &["dist/", "lib/", "src/", "bin/", "build/", "esm/", "cjs/"];

/// Never extract on Default/Minimal (not on Full).
const SKIP_NOISE: &[&str] = &[
    "test/",
    "tests/",
    "__tests__/",
    "coverage/",
    ".github/",
    ".gitlab/",
    "docs/",
    "example/",
    "examples/",
    "benchmark/",
    "benchmarks/",
    ".npmignore",
    ".gitignore",
    ".travis.yml",
    ".circleci/",
    "makefile",
];

pub fn extract_selective(
    tarball_path: &Path,
    dest_dir: &Path,
    package_name: &str,
) -> Result<usize, String> {
    let package_name = utils::sanitize_package_name(package_name)
        .ok_or_else(|| format!("Invalid package name: {}", package_name))?;
    extract_selective_to_path(tarball_path, &dest_dir.join(package_name))
}

pub fn extract_selective_to_path(
    tarball_path: &Path,
    package_root: &Path,
) -> Result<usize, String> {
    extract_with_sheaf(tarball_path, package_root, FeatureSheaf::from_env())
}

/// Prefer linking/copying from full store_unpacked with feature filter.
pub fn project_from_store(
    store_unpacked: &Path,
    package_root: &Path,
    sheaf: FeatureSheaf,
) -> Result<usize, String> {
    if !store_unpacked.exists() {
        return Err(format!(
            "store unpacked missing: {}",
            store_unpacked.display()
        ));
    }
    // KPSL default: whole-dir link for Full and Default (warm path speed).
    // Minimal still filters files for size-sensitive installs.
    if sheaf == FeatureSheaf::Full || sheaf == FeatureSheaf::Default {
        return crate::materialize::place(store_unpacked, package_root);
    }

    fs::create_dir_all(package_root).map_err(|e| e.to_string())?;
    let mut count = 0usize;
    let mut entry_extra: HashSet<String> = HashSet::new();

    // First pass: find package.json for entry-point discovery.
    let pj = store_unpacked.join("package.json");
    if pj.exists() {
        if let Ok(raw) = fs::read_to_string(&pj) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                collect_entry_paths(&v, &mut entry_extra);
            }
        }
    }

    walk_project(
        store_unpacked,
        store_unpacked,
        package_root,
        sheaf,
        &entry_extra,
        &mut count,
    )?;
    Ok(count)
}

fn walk_project(
    root: &Path,
    cur: &Path,
    dest_root: &Path,
    sheaf: FeatureSheaf,
    entry_extra: &HashSet<String>,
    count: &mut usize,
) -> Result<(), String> {
    let entries = fs::read_dir(cur).map_err(|e| e.to_string())?;
    for ent in entries.flatten() {
        let path = ent.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        // `is_dir()` follows symlinks, and the store scaffolding contains cycles
        // (see `copy_tree_recursive`). Treat a symlinked directory as a leaf so the
        // walk terminates.
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() && !meta.file_type().is_symlink() {
            if sheaf != FeatureSheaf::Full && should_skip_noise(&rel) {
                continue;
            }
            walk_project(root, &path, dest_root, sheaf, entry_extra, count)?;
            continue;
        }
        if !should_keep_file(&rel, sheaf, entry_extra) {
            continue;
        }
        let dest = dest_root.join(path.strip_prefix(root).unwrap());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if crate::cas::link_package(&path, &dest).is_err() {
            fs::copy(&path, &dest).map_err(|e| e.to_string())?;
        }
        *count += 1;
    }
    Ok(())
}

pub fn extract_with_sheaf(
    tarball_path: &Path,
    package_root: &Path,
    sheaf: FeatureSheaf,
) -> Result<usize, String> {
    let __dbg = std::env::var("JHOL_DEBUG_LAYOUT").is_ok();
    // Prefer store_unpacked sibling if this is a store tarball path.
    if let Some(hash) = tarball_path.file_stem().and_then(|s| s.to_str()) {
        if hash.len() >= 8 {
            let cache = crate::utils::get_cache_dir();
            let shard = &hash[..2.min(hash.len())];
            let unpacked = PathBuf::from(&cache)
                .join("store_unpacked")
                .join(shard)
                .join(hash);
            let complete = unpacked.exists() && crate::registry::store_extract_complete(&unpacked);
            if __dbg {
                crate::utils::log(&format!(
                    "[extract] tarball={} dest={} store_dir={} exists={} complete={} -> {}",
                    &hash[..12.min(hash.len())],
                    package_root.display(),
                    unpacked.display(),
                    unpacked.exists(),
                    complete,
                    if complete {
                        "project_from_store"
                    } else {
                        "streaming"
                    }
                ));
            }
            let complete = complete
                || crate::registry::ensure_store_unpacked_for_tarball(tarball_path)
                    .unwrap_or(false);
            if complete {
                let r = project_from_store(&unpacked, package_root, sheaf);
                if __dbg {
                    crate::utils::log(&format!(
                        "[extract]   project_from_store -> {:?}; dest now a symlink: {}; dest exists: {}",
                        r.as_ref().map(|n| *n).map_err(|e| e.as_str()),
                        package_root.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false),
                        package_root.exists()
                    ));
                }
                return r;
            }
        }
    } else if __dbg {
        crate::utils::log(&format!(
            "[extract] tarball={} has no usable stem; streaming into {}",
            tarball_path.display(),
            package_root.display()
        ));
    }

    let file = File::open(tarball_path).map_err(|e| format!("Failed to open tarball: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    fs::create_dir_all(package_root).map_err(|e| format!("Failed to create dest dir: {}", e))?;

    // Selective mode needs package.json before it can apply the package's file map.
    // Keep this pre-manifest buffer bounded; on unusual archives, safely extract
    // the remaining package in full rather than risk unbounded memory or drop files.
    let selective = sheaf != FeatureSheaf::Full;
    let mut pre_buffer: Vec<(String, Vec<u8>, bool)> = Vec::new();
    let mut pre_buffer_bytes = 0u64;
    let mut entry_extra: HashSet<String> = HashSet::new();
    let mut seen_package_json = false;
    let mut full_fallback = false;
    let mut extracted_count = 0usize;

    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read archive: {}", e))?
    {
        let mut entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to get path: {}", e))?
            .to_string_lossy()
            .to_string();
        let Some(rel_path) = normalize_tarball_rel_path(&path) else {
            continue;
        };
        let is_dir = entry.header().entry_type().is_dir();
        let is_manifest = rel_path == "package.json";
        if selective && !seen_package_json && !full_fallback {
            let entry_size = if is_dir {
                0
            } else {
                entry
                    .header()
                    .size()
                    .map_err(|e| format!("read archive entry size: {}", e))?
            };
            if pre_buffer.len() >= MAX_PRE_MANIFEST_ENTRIES
                || pre_buffer_bytes.saturating_add(entry_size) > MAX_PRE_MANIFEST_BUFFER_BYTES
            {
                full_fallback = true;
                for (rp, data, is_buffered_dir) in pre_buffer.drain(..) {
                    write_buffered_entry(package_root, &rp, &data, is_buffered_dir)?;
                    extracted_count += usize::from(!is_buffered_dir);
                }
                pre_buffer_bytes = 0;
            }
        }
        let skip_entry = selective
            && !full_fallback
            && seen_package_json
            && !should_keep_file(&rel_path, sheaf, &entry_extra);
        if skip_entry {
            if !is_dir {
                std::io::copy(&mut entry, &mut std::io::sink())
                    .map_err(|e| format!("skip archive entry: {}", e))?;
            }
            continue;
        }
        let needs_buffer = selective && !full_fallback && (!seen_package_json || is_manifest);
        let mut data = Vec::new();
        if !is_dir && needs_buffer {
            std::io::Read::read_to_end(&mut entry, &mut data)
                .map_err(|e| format!("read entry: {}", e))?;
        }
        if is_manifest && selective && !full_fallback {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&data) {
                collect_entry_paths(&v, &mut entry_extra);
            }
            seen_package_json = true;
            // Flush pre-buffer now that we know entry_extra
            for (rp, d, id) in pre_buffer.drain(..) {
                if !should_keep_file(&rp, sheaf, &entry_extra) {
                    continue;
                }
                write_buffered_entry(package_root, &rp, &d, id)?;
                extracted_count += usize::from(!id);
            }
            // Write package.json itself
            write_buffered_entry(package_root, &rel_path, &data, false)?;
            extracted_count += 1;
            continue;
        }
        if selective && !seen_package_json && !full_fallback {
            let entry_size = if is_dir {
                0
            } else {
                u64::try_from(data.len()).unwrap_or(u64::MAX)
            };
            pre_buffer.push((rel_path, data, is_dir));
            pre_buffer_bytes = pre_buffer_bytes.saturating_add(entry_size);
            continue;
        }
        if is_manifest {
            seen_package_json = true;
        }
        // Entries excluded by the selected sheaf were drained without allocating
        // their contents above.
        let dest_path = package_root.join(&rel_path);
        if is_dir {
            fs::create_dir_all(&dest_path).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            let mut output =
                File::create(&dest_path).map_err(|e| format!("Failed to create file: {}", e))?;
            std::io::copy(&mut entry, &mut output)
                .map_err(|e| format!("Failed to unpack file: {}", e))?;
            extracted_count += 1;
        }
    }
    // Flush any remaining pre-buffer entries if package.json was never found
    for (rel_path, data, is_dir) in pre_buffer {
        if !should_keep_file(&rel_path, sheaf, &entry_extra) {
            continue;
        }
        write_buffered_entry(package_root, &rel_path, &data, is_dir)?;
        extracted_count += usize::from(!is_dir);
    }
    Ok(extracted_count)
}

fn write_buffered_entry(
    package_root: &Path,
    rel_path: &str,
    data: &[u8],
    is_dir: bool,
) -> Result<(), String> {
    let dest_path = package_root.join(rel_path);
    if is_dir {
        fs::create_dir_all(&dest_path).map_err(|e| format!("Failed to create dir: {}", e))?;
    } else {
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create parent: {}", e))?;
        }
        fs::write(&dest_path, data).map_err(|e| format!("Failed to unpack file: {}", e))?;
    }
    Ok(())
}

fn collect_entry_paths(pkg: &serde_json::Value, out: &mut HashSet<String>) {
    let push = |s: &str, out: &mut HashSet<String>| {
        let t = s.trim().trim_start_matches("./");
        if !t.is_empty() {
            out.insert(t.replace('\\', "/"));
        }
    };
    for key in ["main", "module", "browser", "types", "typings"] {
        if let Some(s) = pkg.get(key).and_then(|v| v.as_str()) {
            push(s, out);
        }
    }
    if let Some(bin) = pkg.get("bin") {
        if let Some(s) = bin.as_str() {
            push(s, out);
        } else if let Some(obj) = bin.as_object() {
            for v in obj.values() {
                if let Some(s) = v.as_str() {
                    push(s, out);
                }
            }
        }
    }
    if let Some(exports) = pkg.get("exports") {
        walk_exports(exports, out);
    }
}

fn walk_exports(v: &serde_json::Value, out: &mut HashSet<String>) {
    match v {
        serde_json::Value::String(s) => {
            let t = s.trim().trim_start_matches("./");
            if !t.is_empty() {
                out.insert(t.replace('\\', "/"));
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                walk_exports(val, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                walk_exports(val, out);
            }
        }
        _ => {}
    }
}

fn should_skip_noise(path: &str) -> bool {
    let path_lower = path.to_lowercase().replace('\\', "/");
    for pattern in SKIP_NOISE {
        if pattern.ends_with('/') {
            if path_lower.starts_with(pattern) || path_lower.contains(&format!("/{}", pattern)) {
                return true;
            }
        } else if path_lower == *pattern || path_lower.ends_with(&format!("/{}", pattern)) {
            return true;
        }
    }
    // Drop pure markdown except LICENSE* already handled in keep.
    if path_lower.ends_with(".md") || path_lower.ends_with(".markdown") {
        let base = path_lower.rsplit('/').next().unwrap_or(&path_lower);
        if !(base.starts_with("license")
            || base.starts_with("licence")
            || base.starts_with("notice")
            || base.starts_with("copying"))
        {
            return true;
        }
    }
    false
}

fn should_keep_file(path: &str, sheaf: FeatureSheaf, entry_extra: &HashSet<String>) -> bool {
    if sheaf == FeatureSheaf::Full {
        return true;
    }
    let path_norm = path.replace('\\', "/");
    let path_lower = path_norm.to_lowercase();

    // Declared entry points always.
    if entry_extra.iter().any(|e| {
        let e = e.to_lowercase();
        path_lower == e
            || path_lower.ends_with(&format!("/{}", e))
            || path_lower.starts_with(&format!("{}/", e.trim_end_matches('/')))
    }) {
        return true;
    }

    // legal
    let base = path_lower.rsplit('/').next().unwrap_or(&path_lower);
    for p in LEGAL_TYPES_CORE {
        if base == *p
            || base.starts_with("license")
            || base.starts_with("licence")
            || base.starts_with("copying")
            || base.starts_with("notice")
        {
            return true;
        }
    }

    // types stalk (always on Default)
    if sheaf == FeatureSheaf::Default {
        if path_lower.ends_with(".d.ts")
            || path_lower.ends_with(".d.mts")
            || path_lower.ends_with(".d.cts")
            || path_lower.starts_with("types/")
            || path_lower.contains("/types/")
            || path_lower.starts_with("typings/")
            || path_lower.contains("/typings/")
        {
            return true;
        }
        // Keep .ts only under types/ already covered; keep package types field files.
        if path_lower == "tsconfig.json" {
            return true;
        }
    }

    // runtime dirs
    for d in RUNTIME_DIRS {
        if path_lower.starts_with(d) || path_lower.contains(&format!("/{}", d)) {
            return true;
        }
    }

    // index + js modules
    if matches!(
        base,
        "index.js" | "index.mjs" | "index.cjs" | "index.ts" | "index.mts" | "index.cts"
    ) {
        return true;
    }
    if path_lower.ends_with(".js")
        || path_lower.ends_with(".mjs")
        || path_lower.ends_with(".cjs")
        || path_lower.ends_with(".json")
        || path_lower.ends_with(".node")
        || path_lower.ends_with(".wasm")
        || path_lower.ends_with(".css")
        || path_lower.ends_with(".map")
    {
        return true;
    }

    // bin scripts without extension
    if path_lower.starts_with("bin/") {
        return true;
    }

    false
}

fn normalize_tarball_rel_path(path: &str) -> Option<String> {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let first = parts.next()?;
    if first != "package" {
        // Some tarballs use package-name/ prefix
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            return None;
        }
        return Some(rest.join("/"));
    }
    let rel = parts.collect::<Vec<_>>().join("/");
    if rel.is_empty() {
        None
    } else {
        Some(rel)
    }
}

/// Get estimated size savings from selective extraction
pub fn estimate_savings(tarball_path: &Path) -> Result<(u64, u64, f64), String> {
    let file = File::open(tarball_path).map_err(|e| format!("Failed to open tarball: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut total_size = 0u64;
    let mut essential_size = 0u64;
    let empty = HashSet::new();
    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read archive: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let size = entry
            .header()
            .size()
            .map_err(|e| format!("Failed to get size: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to get path: {}", e))?
            .to_string_lossy()
            .to_string();
        total_size += size;
        if let Some(rel_path) = normalize_tarball_rel_path(&path) {
            if should_keep_file(&rel_path, FeatureSheaf::Default, &empty) {
                essential_size += size;
            }
        }
    }
    let savings = if total_size > 0 {
        ((total_size.saturating_sub(essential_size)) as f64 / total_size as f64) * 100.0
    } else {
        0.0
    };
    Ok((total_size, essential_size, savings))
}

/// Extract tarball from a reader (streaming — no temp file needed).
pub fn extract_tarball_from_reader<R: std::io::Read>(
    reader: R,
    dest_dir: &Path,
) -> Result<(usize, String), String> {
    let mut hasher = Sha256::new();
    let mut tee = TeeReader::new(reader, &mut hasher);
    let decoder = GzDecoder::new(&mut tee);
    let mut archive = Archive::new(decoder);
    fs::create_dir_all(dest_dir).map_err(|e| format!("Failed to create dest dir: {}", e))?;
    let mut count = 0usize;
    for entry in archive.entries().map_err(|e| format!("Tar error: {}", e))? {
        let mut entry = entry.map_err(|e| format!("Tar entry error: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Tar path error: {}", e))?
            .to_path_buf();
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let dest = dest_dir.join(&path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        }
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&dest).ok();
        } else {
            let mut out = fs::File::create(&dest).map_err(|e| format!("create: {}", e))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("write: {}", e))?;
        }
        count += 1;
    }
    let hash = format!("{:x}", hasher.finalize());
    Ok((count, hash))
}

struct TeeReader<'a, R: std::io::Read> {
    inner: R,
    hasher: &'a mut Sha256,
}

impl<'a, R: std::io::Read> TeeReader<'a, R> {
    fn new(inner: R, hasher: &'a mut Sha256) -> Self {
        Self { inner, hasher }
    }
}

impl<'a, R: std::io::Read> std::io::Read for TeeReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_from_store_replaces_a_leftover_destination_file() {
        // The E145-era failure: an earlier failed run left a file at the
        // destination, and copying onto it aborted the install with
        // `File exists (os error 17)`. Extraction must complete instead.
        let base = temp_dir("leftover");
        let store = base.join("store/pkg");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("index.js"), b"fresh").unwrap();
        fs::write(
            store.join("package.json"),
            br#"{"name":"pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        let dest = base.join("dest/pkg");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("index.js"), b"stale").unwrap();
        let n = project_from_store(&store, &dest, FeatureSheaf::Default).expect("must not abort");
        assert!(n >= 1);
        assert_eq!(fs::read_to_string(dest.join("index.js")).unwrap(), "fresh");
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "jhol-ex-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|x| x.subsec_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    use super::*;

    #[test]
    fn selective_tar_extraction_drains_skipped_entries_without_writing_them() {
        let base = temp_dir("selective-stream");
        let tarball = base.join("fixture.tgz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&tarball).unwrap(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let append = |archive: &mut tar::Builder<_>, path: &str, bytes: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, path, bytes).unwrap();
        };
        append(&mut archive, "package/index.js", b"module.exports = 1;");
        append(
            &mut archive,
            "package/package.json",
            br#"{"name":"pkg","version":"1.0.0"}"#,
        );
        append(
            &mut archive,
            "package/test/large.fixture",
            &vec![b'x'; 1 << 20],
        );
        append(
            &mut archive,
            "package/dist/later.js",
            b"module.exports = 2;",
        );
        archive.into_inner().unwrap().finish().unwrap();

        let destination = base.join("dest");
        extract_with_sheaf(&tarball, &destination, FeatureSheaf::Default).unwrap();
        assert!(destination.join("package.json").is_file());
        assert_eq!(
            fs::read(destination.join("index.js")).unwrap(),
            b"module.exports = 1;"
        );
        assert_eq!(
            fs::read(destination.join("dist/later.js")).unwrap(),
            b"module.exports = 2;"
        );
        assert!(!destination.join("test/large.fixture").exists());
    }

    #[test]
    fn late_manifest_falls_back_to_full_extraction_with_bounded_buffer() {
        let base = temp_dir("late-manifest");
        let tarball = base.join("fixture.tgz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&tarball).unwrap(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(encoder);
        let append = |archive: &mut tar::Builder<_>, path: &str, bytes: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, path, bytes).unwrap();
        };
        append(
            &mut archive,
            "package/test/before.bin",
            b"keep before manifest",
        );
        append(
            &mut archive,
            "package/test/oversize.bin",
            &vec![b'x'; MAX_PRE_MANIFEST_BUFFER_BYTES as usize + 1],
        );
        append(
            &mut archive,
            "package/package.json",
            br#"{"name":"pkg","version":"1.0.0"}"#,
        );
        append(&mut archive, "package/index.js", b"module.exports = 1;");
        archive.into_inner().unwrap().finish().unwrap();

        let destination = base.join("dest");
        extract_with_sheaf(&tarball, &destination, FeatureSheaf::Default).unwrap();
        assert_eq!(
            fs::read(destination.join("test/before.bin")).unwrap(),
            b"keep before manifest"
        );
        assert_eq!(
            fs::metadata(destination.join("test/oversize.bin"))
                .unwrap()
                .len(),
            MAX_PRE_MANIFEST_BUFFER_BYTES + 1
        );
        assert!(destination.join("package.json").is_file());
        assert_eq!(
            fs::read(destination.join("index.js")).unwrap(),
            b"module.exports = 1;"
        );
    }

    #[test]
    fn test_keep_license_and_dts() {
        let empty = HashSet::new();
        assert!(should_keep_file("LICENSE", FeatureSheaf::Default, &empty));
        assert!(should_keep_file(
            "index.d.ts",
            FeatureSheaf::Default,
            &empty
        ));
        assert!(should_keep_file(
            "dist/index.js",
            FeatureSheaf::Default,
            &empty
        ));
        assert!(
            !should_keep_file("test/foo.js", FeatureSheaf::Default, &empty)
                || should_skip_noise("test/foo.js")
        );
        assert!(should_skip_noise("test/foo.js"));
        assert!(should_keep_file("README.md", FeatureSheaf::Full, &empty));
    }

    #[test]
    fn test_entry_extra() {
        let mut extra = HashSet::new();
        extra.insert("build/main.js".into());
        assert!(should_keep_file(
            "build/main.js",
            FeatureSheaf::Minimal,
            &extra
        ));
    }

    #[test]
    fn test_normalize() {
        assert_eq!(
            normalize_tarball_rel_path("package/package.json"),
            Some("package.json".into())
        );
    }
}

#[cfg(test)]
mod symlink_loop_tests {

    #[test]
    fn materialization_copy_preserves_package_files() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("store/pkg");
        let dest = td.path().join("node_modules/pkg");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("package.json"), br#"{"name":"pkg"}"#).unwrap();
        std::fs::write(src.join("lib/index.js"), b"module.exports = 1;").unwrap();
        std::fs::write(src.join("bin/tool"), b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::create_dir_all(dest.join("stale")).unwrap();
        std::fs::write(dest.join("stale/old.txt"), b"stale").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(src.join("bin/tool"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::os::unix::fs::symlink("index.js", src.join("lib/alias.js")).unwrap();
        }

        crate::materialize::copy_tree(&src, &dest).expect("copy package tree");

        assert!(!dest.join("stale").exists(), "stale destination removed");
        assert_eq!(
            std::fs::read(dest.join("lib/index.js")).unwrap(),
            b"module.exports = 1;"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            assert_eq!(
                std::fs::read_link(dest.join("lib/alias.js")).unwrap(),
                std::path::PathBuf::from("index.js")
            );
            assert_eq!(
                std::fs::metadata(dest.join("bin/tool"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0o111,
                "executable bits must survive materialization"
            );
            assert_eq!(
                std::fs::metadata(dest.join("lib")).unwrap().mode() & 0o777,
                std::fs::metadata(src.join("lib")).unwrap().mode() & 0o777,
                "directory permissions must survive materialization"
            );
        }
        std::fs::write(dest.join("lib/index.js"), b"edited in node_modules").unwrap();
        assert_eq!(
            std::fs::read(src.join("lib/index.js")).unwrap(),
            b"module.exports = 1;",
            "editing the materialized package must not mutate the store"
        );
    }

    #[test]
    fn invalid_copy_source_keeps_existing_destination() {
        let td = tempfile::tempdir().unwrap();
        let missing_source = td.path().join("missing-source");
        let dest = td.path().join("node_modules/pkg");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("keep.txt"), b"existing install").unwrap();

        assert!(crate::materialize::copy_tree(&missing_source, &dest).is_err());
        assert_eq!(
            std::fs::read(dest.join("keep.txt")).unwrap(),
            b"existing install"
        );
    }

    /// A cyclic symlink structure (as the store's relink scaffolding produces) must
    /// not make the copier recurse forever: it reproduces the link and stops.
    #[test]
    #[cfg(unix)]
    fn cyclic_symlinks_do_not_loop() {
        let td = tempfile::tempdir().unwrap();
        let a = td.path().join("a");
        let b = td.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("file.txt"), b"content").unwrap();
        // a/node_modules/b -> b ; b/node_modules/a -> a
        std::fs::create_dir_all(a.join("node_modules")).unwrap();
        std::fs::create_dir_all(b.join("node_modules")).unwrap();
        std::os::unix::fs::symlink(&b, a.join("node_modules/b")).unwrap();
        std::os::unix::fs::symlink(&a, b.join("node_modules/a")).unwrap();

        let dest = td.path().join("out");
        let mut count = 0usize;
        crate::materialize::copy_tree_recursive(&a, &dest, &mut count).expect("must terminate");
        assert!(count > 0, "the real file should have been copied");
        assert!(dest.join("file.txt").exists());
        // the cyclic link is reproduced as a link, not descended into
        assert!(std::fs::symlink_metadata(dest.join("node_modules/b"))
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false));
    }
}
