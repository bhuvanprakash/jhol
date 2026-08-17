//! JGrid — The Jhol Instant Materialization Pipeline
//!
//! A content-addressed, memory-mappable, clone-ready package storage format
//! that eliminates tarballs as the canonical cache artifact.
//!
//! ## Architecture
//!
//! Three-tier materialization pyramid:
//! - **L0**: Clone-ready bundles — entire `node_modules/` cloned with one syscall
//! - **L1**: Pre-assembled directory trees — per-package, clonefile-ready
//! - **L2**: JGrid content blocks — raw, mmap'd, content-addressed file data
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use jhol_pipeline::{JGridPipeline, ResolvedPackage};
//!
//! let pipeline = JGridPipeline::new("~/.jhol-cache")?;
//! let packages = vec![ResolvedPackage {
//!     name: "lodash".into(),
//!     version: "4.17.21".into(),
//! }];
//!
//! if pipeline.materialize(&packages, "./node_modules")? {
//!     println!("Instant install!");
//! }
//! ```

pub mod format;
pub mod ingester;
pub mod materialize;
pub mod store;

use std::path::Path;

pub use format::{
    build_directory_tree, DirEntry, FileEntry, GridHeader, Manifest,
    StringTable as FormatStringTable, SwissTable, TreeDagEntry, BLOCK_ALIGN, HEADER_SIZE,
};
pub use ingester::{hash_content, ingest_raw_files, verify_content, IngestResult};
pub use materialize::{Cloner, Materializer, ResolvedPackage};
pub use store::{BundleCache, GridBuilder as StoreBuilder, GridStore, TreeCache};

/// The top-level JGrid Pipeline API
pub struct JGridPipeline {
    grid_store: GridStore,
    tree_cache: TreeCache,
    bundle_cache: BundleCache,
}

impl JGridPipeline {
    /// Open or create the JGrid pipeline at the given cache directory
    pub fn new(cache_dir: &Path) -> Result<Self, String> {
        let grid_path = cache_dir.join("store.jgrid");

        let grid_store = if grid_path.exists() {
            GridStore::open(&grid_path)?
        } else {
            GridStore::create(&grid_path)?
        };

        let tree_cache = TreeCache::new(cache_dir);
        let bundle_cache = BundleCache::new(cache_dir);

        Ok(Self {
            grid_store,
            tree_cache,
            bundle_cache,
        })
    }

    /// Materialize a set of resolved packages into node_modules
    ///
    /// Returns Ok(true) if materialized via pipeline (L0/L1/L2 hit).
    /// Returns Ok(false) if cache miss — caller should fall back to traditional install.
    pub fn materialize(
        &self,
        packages: &[ResolvedPackage],
        node_modules: &Path,
    ) -> Result<bool, String> {
        let materializer =
            Materializer::new(&self.grid_store, &self.tree_cache, &self.bundle_cache);
        materializer.materialize(packages, node_modules)
    }

    /// Ingest file entries into the JGrid store
    pub fn ingest(
        &mut self,
        name: &str,
        version: &str,
        file_entries: &[(&str, u16, &[u8])],
    ) -> Result<IngestResult, String> {
        // We need to rebuild the store to add new content
        // For simplicity, we open the grid, read it, add new content, and rewrite
        let grid_path = self.grid_store.path().to_path_buf();

        let mut builder = StoreBuilder::create(&grid_path)?;

        // Re-add existing manifests
        for manifest in self.grid_store.manifests() {
            let mut files = Vec::new();
            for file in &manifest.files {
                if let Some(content) = self.grid_store.read_block(&file.content_hash) {
                    // Add the block back (deduped by builder)
                    builder.add_block(content);
                    files.push((
                        self.grid_store
                            .string_table()
                            .get(file.path_offset)
                            .to_string(),
                        file.mode,
                        file.content_hash,
                    ));
                }
            }
            builder.add_manifest(&manifest.name, &manifest.version, &files)?;
        }

        // Ingest new files
        let result = ingest_raw_files(name, version, file_entries, &mut builder)?;

        builder.finalize()?;

        // Re-open
        *self = Self::new(&grid_path.parent().unwrap_or(Path::new(".")))?;

        Ok(result)
    }

    /// Check if a bundle exists for the given dependency set
    pub fn has_bundle(&self, packages: &[ResolvedPackage]) -> bool {
        let refs: Vec<(&str, &str)> = packages
            .iter()
            .map(|p| (p.name.as_str(), p.version.as_str()))
            .collect();
        let hash = BundleCache::compute_dep_set_hash(&refs);
        self.bundle_cache.has_bundle(&hash)
    }

    /// Access the underlying grid store
    pub fn store(&self) -> &GridStore {
        &self.grid_store
    }

    pub fn tree_cache(&self) -> &TreeCache {
        &self.tree_cache
    }

    pub fn bundle_cache(&self) -> &BundleCache {
        &self.bundle_cache
    }

    /// Get the total number of unique content blocks in the store
    pub fn block_count(&self) -> u64 {
        self.grid_store.header().get_block_count()
    }

    /// Get the number of package manifests
    pub fn manifest_count(&self) -> u32 {
        self.grid_store.header().get_manifest_count()
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_full_pipeline_roundtrip() {
        let dir = tempdir().unwrap();
        let mut pipeline = JGridPipeline::new(dir.path()).unwrap();

        // Ingest a package
        let result = pipeline
            .ingest(
                "my-pkg",
                "2.0.0",
                &[
                    ("index.js", 0o644, b"module.exports = 42"),
                    ("package.json", 0o644, br#"{"name":"my-pkg"}"#),
                ],
            )
            .unwrap();

        assert_eq!(result.name, "my-pkg");
        assert_eq!(result.version, "2.0.0");
        assert_eq!(result.files.len(), 2);

        // Materialize
        let nm = dir.path().join("node_modules");
        let success = pipeline
            .materialize(
                &[ResolvedPackage {
                    name: "my-pkg".into(),
                    version: "2.0.0".into(),
                }],
                &nm,
            )
            .unwrap();

        assert!(success);
        assert!(nm.join("my-pkg/index.js").exists());

        // Verify content
        let content = std::fs::read(nm.join("my-pkg/index.js")).unwrap();
        assert_eq!(content, b"module.exports = 42");

        // L0 on second call
        std::fs::remove_dir_all(&nm).unwrap();
        let success2 = pipeline
            .materialize(
                &[ResolvedPackage {
                    name: "my-pkg".into(),
                    version: "2.0.0".into(),
                }],
                &nm,
            )
            .unwrap();
        assert!(success2);
        assert!(nm.join("my-pkg/index.js").exists());
    }

    #[test]
    fn test_empty_pipeline() {
        let dir = tempdir().unwrap();
        let pipeline = JGridPipeline::new(dir.path()).unwrap();
        assert_eq!(pipeline.block_count(), 0);
        assert_eq!(pipeline.manifest_count(), 0);
    }

    #[test]
    fn test_pipeline_cache_miss() {
        let dir = tempdir().unwrap();
        let pipeline = JGridPipeline::new(dir.path()).unwrap();

        let result = pipeline
            .materialize(
                &[ResolvedPackage {
                    name: "ghost".into(),
                    version: "0.0.0".into(),
                }],
                &dir.path().join("node_modules"),
            )
            .unwrap();

        assert!(!result, "unknown package should return false (cache miss)");
    }
}
