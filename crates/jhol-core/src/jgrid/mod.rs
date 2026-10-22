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
//! use jhol_core::jgrid::{JGridPipeline, ResolvedPackage};
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
pub use materialize::{Cloner, LayoutPackage, Materializer, ResolvedPackage};
pub use store::{BundleCache, GridBuilder as StoreBuilder, GridGc, GridStore, TreeCache};

/// One package tree to add during a single JGrid generation update.
pub struct PackageIngest {
    pub name: String,
    pub version: String,
    pub files: Vec<(String, u16, Vec<u8>)>,
}

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
            match GridStore::open(&grid_path) {
                Ok(s) => s,
                Err(e) => {
                    // The grid is a rebuildable cache: a corrupt or
                    // never-finalized store must not disable the pipeline
                    // forever, so rebuild it in place.
                    if std::env::var("JHOL_VERBOSE").is_ok() {
                        eprintln!(
                            "warning: rebuilding unreadable JGrid store ({}): {}",
                            grid_path.display(),
                            e
                        );
                    }
                    let _ = std::fs::remove_file(&grid_path);
                    GridStore::create(&grid_path)?
                }
            }
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

    /// Materialize exact lockfile paths, including nested duplicate versions.
    pub fn materialize_layout(
        &self,
        packages: &[LayoutPackage],
        node_modules: &Path,
    ) -> Result<bool, String> {
        let materializer =
            Materializer::new(&self.grid_store, &self.tree_cache, &self.bundle_cache);
        materializer.materialize_layout(packages, node_modules)
    }

    /// Ingest file entries into the JGrid store
    pub fn ingest(
        &mut self,
        name: &str,
        version: &str,
        file_entries: &[(&str, u16, &[u8])],
    ) -> Result<IngestResult, String> {
        self.rewrite_store(|builder| ingest_raw_files(name, version, file_entries, builder))
    }

    /// Add many package trees with one store copy, finalization, and publication.
    /// Each iterator item is consumed before the next, bounding file-content memory
    /// to one package tree at a time.
    pub fn ingest_batch(
        &mut self,
        packages: impl IntoIterator<Item = PackageIngest>,
    ) -> Result<usize, String> {
        let mut packages = packages.into_iter().peekable();
        if packages.peek().is_none() {
            return Ok(0);
        }

        self.rewrite_store(|builder| {
            let mut count = 0;
            for package in packages {
                let entries: Vec<(&str, u16, &[u8])> = package
                    .files
                    .iter()
                    .map(|(path, mode, content)| (path.as_str(), *mode, content.as_slice()))
                    .collect();
                ingest_raw_files(&package.name, &package.version, &entries, builder)?;
                count += 1;
            }
            Ok(count)
        })
    }

    fn rewrite_store<T>(
        &mut self,
        append: impl FnOnce(&mut StoreBuilder) -> Result<T, String>,
    ) -> Result<T, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

        let grid_path = self.grid_store.path().to_path_buf();
        let _writer_lock = crate::file_lock::FileLock::lock(&grid_path)
            .map_err(|e| format!("lock JGrid writer: {e}"))?;
        // Reopen only after locking so another process's published generation is
        // included in this rewrite. Keep its mmap alive until our rename completes.
        let current_store = GridStore::open(&grid_path)?;
        let nonce = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let tmp_path = grid_path.with_file_name(format!(
            "store.jgrid.tmp.{}.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            nonce
        ));

        let result = (|| {
            let mut builder = StoreBuilder::create(&tmp_path)?;
            for manifest in current_store.manifests() {
                let mut files = Vec::with_capacity(manifest.files.len());
                for file in &manifest.files {
                    let content =
                        current_store
                            .read_block(&file.content_hash)
                            .ok_or_else(|| {
                                format!(
                                    "missing existing JGrid block for {}@{}",
                                    manifest.name, manifest.version
                                )
                            })?;
                    let copied_hash = builder.add_block(content);
                    if copied_hash != file.content_hash {
                        return Err(format!(
                            "JGrid block hash mismatch for {}@{}",
                            manifest.name, manifest.version
                        ));
                    }
                    files.push((
                        current_store
                            .string_table()
                            .get(file.path_offset)
                            .to_string(),
                        file.mode,
                        file.content_hash,
                    ));
                }
                builder.add_manifest(&manifest.name, &manifest.version, &files)?;
            }

            let result = append(&mut builder)?;
            builder.finalize()?;
            std::fs::rename(&tmp_path, &grid_path)
                .map_err(|e| format!("replace JGrid store: {e}"))?;
            Ok(result)
        })();

        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
            return result;
        }
        *self = Self::new(grid_path.parent().unwrap_or(Path::new(".")))?;
        result
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

/// Rebuild the on-disk JGrid store, keeping only content blocks referenced by a
/// manifest (orphans come from replaced/uninstalled package versions).
///
/// Rebuilds into a sibling temp file and renames it into place. The rename cannot
/// truncate the live mapping that `GridStore` holds — doing so would make the next
/// `read_block` copy from pages that no longer exist and die with SIGBUS (the bug
/// fixed in E31), so the store is explicitly dropped before the rename.
///
/// Returns `(orphan_blocks_removed, bytes_before, bytes_after)`.
pub fn gc_store_in_place(store_path: &Path) -> Result<(u64, u64, u64), String> {
    if !store_path.exists() {
        return Ok((0, 0, 0));
    }
    let before = std::fs::metadata(store_path).map(|m| m.len()).unwrap_or(0);
    let file_name = store_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store.jgrid".to_string());
    let tmp_path = store_path.with_file_name(format!("{}.gc{}", file_name, std::process::id()));

    let removed = {
        // Scope the store: its mmap must be dropped before we rename over the path.
        let store = GridStore::open(store_path)?;
        match GridGc::run(&store, &tmp_path) {
            Ok(removed) => removed,
            Err(e) => {
                drop(store);
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e);
            }
        }
    };

    if removed == 0 {
        let _ = std::fs::remove_file(&tmp_path);
        return Ok((0, before, before));
    }

    std::fs::rename(&tmp_path, store_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("replace jgrid store: {e}")
    })?;
    let after = std::fs::metadata(store_path).map(|m| m.len()).unwrap_or(0);
    Ok((removed, before, after))
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
    fn batch_ingest_publishes_all_packages_in_one_reopenable_generation() {
        let dir = tempdir().unwrap();
        let mut pipeline = JGridPipeline::new(dir.path()).unwrap();
        pipeline
            .ingest("existing", "1.0.0", &[("index.js", 0o644, b"old")])
            .unwrap();
        let ingested = pipeline
            .ingest_batch(vec![
                PackageIngest {
                    name: "alpha".into(),
                    version: "1.0.0".into(),
                    files: vec![("index.js".into(), 0o644, b"shared".to_vec())],
                },
                PackageIngest {
                    name: "beta".into(),
                    version: "2.0.0".into(),
                    files: vec![("index.js".into(), 0o755, b"shared".to_vec())],
                },
            ])
            .unwrap();

        assert_eq!(ingested, 2);
        assert_eq!(pipeline.manifest_count(), 3);
        assert!(pipeline
            .store()
            .find_manifest("existing", "1.0.0")
            .is_some());
        for (name, version) in [("alpha", "1.0.0"), ("beta", "2.0.0")] {
            let manifest = pipeline.store().find_manifest(name, version).unwrap();
            assert_eq!(manifest.files.len(), 1);
            assert_eq!(
                pipeline.store().read_block(&manifest.files[0].content_hash),
                Some(&b"shared"[..])
            );
        }
        let reopened = JGridPipeline::new(dir.path()).unwrap();
        assert_eq!(reopened.manifest_count(), 3);
    }

    #[test]
    fn test_empty_pipeline() {
        let dir = tempdir().unwrap();
        let pipeline = JGridPipeline::new(dir.path()).unwrap();
        assert_eq!(pipeline.block_count(), 0);
        assert_eq!(pipeline.manifest_count(), 0);
    }

    #[test]
    fn gc_store_in_place_drops_orphans_and_keeps_store_readable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("store.jgrid");
        {
            let mut b = StoreBuilder::create(&path).unwrap();
            let referenced = b.add_block(b"referenced");
            let _orphan = b.add_block(b"orphan-block");
            b.add_manifest(
                "pkg",
                "1.0.0",
                &[("index.js".to_string(), 0o644, referenced)],
            )
            .unwrap();
            b.finalize().unwrap();
        }

        let before = std::fs::metadata(&path).unwrap().len();
        let (removed, bytes_before, after) = gc_store_in_place(&path).unwrap();
        assert!(removed >= 1, "expected an orphan block to be dropped");
        assert_eq!(bytes_before, before);
        assert!(
            after < before,
            "store should shrink: {} -> {}",
            before,
            after
        );

        // The rebuilt store must still open and keep its manifest.
        let store = GridStore::open(&path).unwrap();
        assert!(store.find_manifest("pkg", "1.0.0").is_some());

        // A second GC pass has nothing left to do and must not corrupt anything.
        let (again, _, _) = gc_store_in_place(&path).unwrap();
        assert_eq!(again, 0);
        assert!(GridStore::open(&path).is_ok());
    }

    #[test]
    fn gc_store_in_place_is_a_noop_when_the_store_is_absent() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope.jgrid");
        assert_eq!(gc_store_in_place(&missing).unwrap(), (0, 0, 0));
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

    /// Regression: `GridStore::create` used to write a zeroed header, so the
    /// store it created could not be opened on the next run
    /// (`invalid JGrid magic or version`) and the pipeline fast path was dead.
    /// A freshly created store must be immediately readable.
    #[test]
    fn test_created_store_is_immediately_readable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("store.jgrid");

        {
            let created = GridStore::create(&path).unwrap();
            assert_eq!(created.header().get_manifest_count(), 0);
        }

        // This is the exact call that failed before the fix.
        let reopened = GridStore::open(&path).expect("freshly created store must open");
        assert_eq!(reopened.header().get_manifest_count(), 0);
        assert_eq!(reopened.header().get_block_count(), 0);
        assert!(reopened.find_manifest("anything", "0.0.0").is_none());

        // Magic must be the real one, not zeroes.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"JHGR", "store header magic");
        assert_ne!(&bytes[..HEADER_SIZE], &vec![0u8; HEADER_SIZE][..]);
    }

    /// The user-visible form of the same bug: constructing the pipeline twice
    /// (i.e. the second `jhol install`) must not fail to read the store.
    #[test]
    fn test_pipeline_reopen_across_runs() {
        let dir = tempdir().unwrap();

        let mut first = JGridPipeline::new(dir.path()).unwrap();
        first
            .ingest(
                "reopen-pkg",
                "1.0.0",
                &[("index.js", 0o644, b"module.exports = 1")],
            )
            .unwrap();
        drop(first);

        // Second process/run: must open the existing store, not rebuild it.
        let second = JGridPipeline::new(dir.path()).unwrap();
        assert_eq!(second.manifest_count(), 1);
        assert!(
            second
                .store()
                .find_manifest("reopen-pkg", "1.0.0")
                .is_some(),
            "manifest written by the first run must survive reopening"
        );
    }

    /// A corrupt/foreign store is a rebuildable cache: `new()` must self-heal
    /// instead of disabling the pipeline forever.
    #[test]
    fn test_pipeline_self_heals_corrupt_store() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("store.jgrid");
        std::fs::write(&path, b"not a jgrid at all").unwrap();

        let pipeline = JGridPipeline::new(dir.path()).expect("corrupt store must self-heal");
        assert_eq!(pipeline.manifest_count(), 0);
        assert!(GridStore::open(&path).is_ok(), "store must be usable again");
    }
}
