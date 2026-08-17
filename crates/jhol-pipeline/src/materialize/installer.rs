use std::path::Path;

use crate::ingester::materialize_tree_from_store;
use crate::materialize::Cloner;
use crate::store::{BundleCache, GridStore, TreeCache};

/// A resolved package reference (from jhol-core's resolution)
#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
}

/// The main installer implementing L0/L1/L2 materialization strategy
pub struct Materializer<'a> {
    grid_store: &'a GridStore,
    tree_cache: &'a TreeCache,
    bundle_cache: &'a BundleCache,
}

impl<'a> Materializer<'a> {
    pub fn new(
        grid_store: &'a GridStore,
        tree_cache: &'a TreeCache,
        bundle_cache: &'a BundleCache,
    ) -> Self {
        Self {
            grid_store,
            tree_cache,
            bundle_cache,
        }
    }

    /// Try to materialize a set of packages into node_modules
    ///
    /// Returns Ok(true) if materialized via pipeline.
    /// Returns Ok(false) if cache miss (caller should fall back to traditional path).
    pub fn materialize(
        &self,
        packages: &[ResolvedPackage],
        node_modules: &Path,
    ) -> Result<bool, String> {
        if packages.is_empty() {
            return Ok(true);
        }

        // Step 1: Try L0 — Clone-Ready Bundle
        let dep_refs: Vec<(&str, &str)> = packages
            .iter()
            .map(|p| (p.name.as_str(), p.version.as_str()))
            .collect();
        let dep_hash = BundleCache::compute_dep_set_hash(&dep_refs);

        if self.bundle_cache.has_bundle(&dep_hash) {
            let bundle_nm = self.bundle_cache.bundle_node_modules_path(&dep_hash);
            Cloner::clone_directory(&bundle_nm, node_modules)?;
            return Ok(true);
        }

        // Step 2: Try L1 — Per-Package Trees
        if self.try_materialize_from_trees(packages, node_modules, &dep_hash)? {
            return Ok(true);
        }

        // Step 3: Try L2 — Build trees from JGrid content blocks
        if self.try_materialize_from_grid(packages, node_modules, &dep_hash)? {
            return Ok(true);
        }

        // Cache miss — caller should fall back
        Ok(false)
    }

    /// L1: Clone pre-assembled trees for each package
    fn try_materialize_from_trees(
        &self,
        packages: &[ResolvedPackage],
        node_modules: &Path,
        dep_hash: &str,
    ) -> Result<bool, String> {
        let mut all_have_trees = true;
        let mut tree_info: Vec<(&ResolvedPackage, [u8; 32])> = Vec::new();

        for pkg in packages {
            let manifest = self.grid_store.find_manifest(&pkg.name, &pkg.version);
            match manifest {
                Some(m) if m.tree_hash != [0u8; 32] && self.tree_cache.has_tree(&m.tree_hash) => {
                    tree_info.push((pkg, m.tree_hash));
                }
                _ => {
                    all_have_trees = false;
                    break;
                }
            }
        }

        if !all_have_trees {
            return Ok(false);
        }

        // All trees exist — clone into staging directory
        let _staging_nm = self.build_staging_and_clone(node_modules, dep_hash, |staging_nm| {
            for (pkg, tree_hash) in &tree_info {
                let tree_path = self.tree_cache.tree_path(tree_hash);
                let pkg_dir = staging_nm.join(&pkg.name);
                Cloner::clone_directory(&tree_path, &pkg_dir)?;
            }
            Ok(())
        })?;

        Ok(true)
    }

    /// L2: Build trees from JGrid content blocks
    fn try_materialize_from_grid(
        &self,
        packages: &[ResolvedPackage],
        node_modules: &Path,
        dep_hash: &str,
    ) -> Result<bool, String> {
        // Check if all packages exist in the grid
        for pkg in packages {
            if self
                .grid_store
                .find_manifest(&pkg.name, &pkg.version)
                .is_none()
            {
                return Ok(false);
            }
        }

        // Build trees from JGrid
        let mut tree_info: Vec<(String, [u8; 32])> = Vec::new();
        for pkg in packages {
            let manifest = self
                .grid_store
                .find_manifest(&pkg.name, &pkg.version)
                .unwrap();
            let tree_path = self.tree_cache.tree_path(&manifest.tree_hash);

            if !self.tree_cache.has_tree(&manifest.tree_hash) {
                materialize_tree_from_store(
                    self.grid_store,
                    manifest,
                    self.grid_store.string_table(),
                    &tree_path,
                )?;
            }

            tree_info.push((pkg.name.clone(), manifest.tree_hash));
        }

        // Clone into staging
        let _staging_nm = self.build_staging_and_clone(node_modules, dep_hash, |staging_nm| {
            for (pkg_name, tree_hash) in &tree_info {
                let tree_path = self.tree_cache.tree_path(tree_hash);
                let pkg_dir = staging_nm.join(pkg_name);
                Cloner::clone_directory(&tree_path, &pkg_dir)?;
            }
            Ok(())
        })?;

        Ok(true)
    }

    /// Build staging directory, clone trees into it, save as bundle, then clone to final destination
    fn build_staging_and_clone(
        &self,
        node_modules: &Path,
        dep_hash: &str,
        populate: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        // If final location already has content, remove it for clean clone
        if node_modules.exists() {
            std::fs::remove_dir_all(node_modules)
                .map_err(|e| format!("remove existing node_modules: {e}"))?;
        }

        // Ensure parent exists
        if let Some(parent) = node_modules.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create parent {parent:?}: {e}"))?;
        }

        // Create staging directory
        let staging = self.bundle_cache.base_dir().join("staging");
        if staging.exists() {
            std::fs::remove_dir_all(&staging).ok();
        }
        let staging_nm = staging.join("node_modules");
        std::fs::create_dir_all(&staging_nm).map_err(|e| format!("create staging: {e}"))?;

        // Populate staging with cloned trees
        populate(&staging_nm)?;

        // Save as bundle
        let bundle_nm = self.bundle_cache.bundle_node_modules_path(dep_hash);
        if bundle_nm.exists() {
            std::fs::remove_dir_all(&bundle_nm.parent().unwrap()).ok();
        }
        if let Some(parent) = bundle_nm.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create bundle parent: {e}"))?;
        }

        // Use cloning for bundle too
        Cloner::clone_directory(&staging_nm, &bundle_nm)?;

        // Clean up staging
        std::fs::remove_dir_all(&staging).ok();

        // Final clone to node_modules
        Cloner::clone_directory(&bundle_nm, node_modules)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingester::ingest_raw_files;
    use crate::store::{BundleCache, GridBuilder, TreeCache};
    use tempfile::tempdir;

    fn setup_test_store(
        dir: &Path,
        packages: &[(&str, &str, &[(&str, u16, &[u8])])],
    ) -> (GridStore, TreeCache, BundleCache) {
        let jgrid_path = dir.join("store.jgrid");
        let mut builder = GridBuilder::create(&jgrid_path).unwrap();

        for (name, version, files) in packages {
            ingest_raw_files(name, version, files, &mut builder).unwrap();
        }

        builder.finalize().unwrap();
        let store = GridStore::open(&jgrid_path).unwrap();
        let tree_cache = TreeCache::new(dir);
        let bundle_cache = BundleCache::new(dir);

        (store, tree_cache, bundle_cache)
    }

    #[test]
    fn test_l2_materialization() {
        let dir = tempdir().unwrap();
        let nm = dir.path().join("node_modules");

        let (store, tree_cache, bundle_cache) = setup_test_store(
            dir.path(),
            &[
                ("pkg-a", "1.0.0", &[("index.js", 0o644, b"a" as &[u8])]),
                ("pkg-b", "2.0.0", &[("index.js", 0o644, b"b")]),
            ],
        );

        let materializer = Materializer::new(&store, &tree_cache, &bundle_cache);
        let packages = vec![
            ResolvedPackage {
                name: "pkg-a".into(),
                version: "1.0.0".into(),
            },
            ResolvedPackage {
                name: "pkg-b".into(),
                version: "2.0.0".into(),
            },
        ];

        let result = materializer.materialize(&packages, &nm).unwrap();
        assert!(result, "L2 materialization should succeed");

        assert!(nm.join("pkg-a/index.js").exists());
        assert!(nm.join("pkg-b/index.js").exists());
        assert_eq!(std::fs::read(nm.join("pkg-a/index.js")).unwrap(), b"a");
        assert_eq!(std::fs::read(nm.join("pkg-b/index.js")).unwrap(), b"b");
    }

    #[test]
    fn test_l0_materialization() {
        let dir = tempdir().unwrap();
        let nm = dir.path().join("node_modules");

        let (store, tree_cache, bundle_cache) =
            setup_test_store(dir.path(), &[("pkg", "1.0.0", &[("f.js", 0o644, b"data")])]);

        // First call builds the bundle (L2)
        let materializer = Materializer::new(&store, &tree_cache, &bundle_cache);
        let packages = vec![ResolvedPackage {
            name: "pkg".into(),
            version: "1.0.0".into(),
        }];
        materializer.materialize(&packages, &nm).unwrap();

        // Clean and re-install
        std::fs::remove_dir_all(&nm).unwrap();

        // Second call should hit L0
        let result = materializer.materialize(&packages, &nm).unwrap();
        assert!(result);
        assert!(nm.join("pkg/f.js").exists());
    }

    #[test]
    fn test_cache_miss_returns_false() {
        let dir = tempdir().unwrap();
        let nm = dir.path().join("node_modules");

        let (store, tree_cache, bundle_cache) = setup_test_store(dir.path(), &[]);
        let materializer = Materializer::new(&store, &tree_cache, &bundle_cache);

        let result = materializer
            .materialize(
                &[ResolvedPackage {
                    name: "unknown".into(),
                    version: "0.0.0".into(),
                }],
                &nm,
            )
            .unwrap();
        assert!(!result, "cache miss should return false");
    }

    #[test]
    fn test_dep_hash_in_bundle_cache() {
        let dir = tempdir().unwrap();
        let bundle_cache = BundleCache::new(dir.path());
        let hash = BundleCache::compute_dep_set_hash(&[("test", "1.0.0")]);
        assert!(!bundle_cache.has_bundle(&hash));

        bundle_cache.create_bundle(&hash).unwrap();
        assert!(bundle_cache.has_bundle(&hash));
    }
}
