use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::jgrid::ingester::materialize_tree_from_store;
use crate::jgrid::materialize::Cloner;
use crate::jgrid::store::{BundleCache, GridStore, TreeCache};

/// A resolved package reference (from jhol-core's resolution)
#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
}

/// Resolved package at its exact lockfile path relative to `node_modules`.
#[derive(Clone, Debug)]
pub struct LayoutPackage {
    pub name: String,
    pub version: String,
    pub install_path: String,
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
        let mut names = std::collections::HashSet::new();
        let layout: Vec<LayoutPackage> = packages
            .iter()
            .map(|pkg| {
                if !names.insert(pkg.name.clone()) {
                    return Err(format!("flat JGrid layout is ambiguous for {}", pkg.name));
                }
                Ok(LayoutPackage {
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    install_path: pkg.name.clone(),
                })
            })
            .collect::<Result<_, _>>()?;
        self.materialize_layout(&layout, node_modules)
    }

    pub fn materialize_layout(
        &self,
        packages: &[LayoutPackage],
        node_modules: &Path,
    ) -> Result<bool, String> {
        if packages.is_empty() {
            return Ok(true);
        }
        let mut paths = std::collections::HashSet::new();
        if packages.iter().any(|pkg| {
            !safe_install_path(&pkg.install_path) || !paths.insert(pkg.install_path.as_str())
        }) {
            return Err("invalid or duplicate JGrid install path".into());
        }

        // Step 0: Light L0 — graph-keyed bundle keyed only by name@version
        // (published by the store_unpacked bridge / post-install L0 publish).
        // Matches LPM/pnpm "virtual store by dep graph" without needing manifests.
        let light_refs: Vec<(&str, &str)> = packages
            .iter()
            .map(|p| (p.name.as_str(), p.version.as_str()))
            .collect();
        let light_hash = BundleCache::compute_dep_set_hash(&light_refs);
        if self.bundle_cache.has_bundle(&light_hash) {
            let bundle_nm = self.bundle_cache.bundle_node_modules_path(&light_hash);
            Cloner::clone_directory(&bundle_nm, node_modules)?;
            super::emit_route("L0");
            return Ok(true);
        }

        // Step 0b: CAS store_unpacked → parallel clonefile (world-class L1).
        // Does not require a populated store.jgrid — only complete unpacked trees.
        if super::store_bridge::all_have_unpacked(packages)
            && super::materialize_from_store_unpacked(packages, node_modules, self.bundle_cache)?
        {
            return Ok(true);
        }

        // Step 1: Verified L0 — Clone-Ready Bundle (content-hash bound)
        let mut identity_rows = Vec::with_capacity(packages.len());
        for package in packages {
            let Some(manifest) = self
                .grid_store
                .find_manifest(&package.name, &package.version)
            else {
                // No manifests and store_unpacked already failed → miss.
                return Ok(false);
            };
            let tree_hash = manifest_tree_hash(manifest, self.grid_store.string_table());
            identity_rows.push((
                package.name.clone(),
                package.version.clone(),
                package.install_path.clone(),
                hex::encode(tree_hash),
            ));
        }
        let dep_refs: Vec<(&str, &str, &str, &str)> = identity_rows
            .iter()
            .map(|(name, version, path, hash)| {
                (
                    name.as_str(),
                    version.as_str(),
                    path.as_str(),
                    hash.as_str(),
                )
            })
            .collect();
        let dep_hash = BundleCache::compute_verified_layout_hash(&dep_refs);

        if self.bundle_cache.has_bundle(&dep_hash) {
            let bundle_nm = self.bundle_cache.bundle_node_modules_path(&dep_hash);
            Cloner::clone_directory(&bundle_nm, node_modules)?;
            super::emit_route("L0-verified");
            return Ok(true);
        }

        // Step 2: Try L1 — Per-Package Trees
        if self.try_materialize_from_trees(packages, node_modules, &dep_hash)? {
            super::emit_route("L1");
            return Ok(true);
        }

        // Step 3: Try L2 — Build trees from JGrid content blocks
        if self.try_materialize_from_grid(packages, node_modules, &dep_hash)? {
            super::emit_route("L2");
            return Ok(true);
        }

        // Cache miss — caller should fall back
        Ok(false)
    }

    /// L1: Clone pre-assembled trees for each package
    fn try_materialize_from_trees(
        &self,
        packages: &[LayoutPackage],
        node_modules: &Path,
        dep_hash: &str,
    ) -> Result<bool, String> {
        let mut all_have_trees = true;
        let mut tree_info: Vec<(&LayoutPackage, [u8; 32])> = Vec::new();

        for pkg in packages {
            let manifest = self.grid_store.find_manifest(&pkg.name, &pkg.version);
            match manifest {
                Some(m) => {
                    let tree_hash = manifest_tree_hash(m, self.grid_store.string_table());
                    if self.tree_cache.has_tree(&tree_hash) {
                        tree_info.push((pkg, tree_hash));
                    } else {
                        all_have_trees = false;
                        break;
                    }
                }
                None => {
                    all_have_trees = false;
                    break;
                }
            }
        }

        if !all_have_trees {
            return Ok(false);
        }

        // All trees exist — clone into staging directory
        self.build_staging_and_clone(node_modules, dep_hash, |staging_nm| {
            for (pkg, tree_hash) in &tree_info {
                let tree_path = self.tree_cache.tree_path(tree_hash);
                let pkg_dir = staging_nm.join(&pkg.install_path);
                Cloner::clone_directory(&tree_path, &pkg_dir)?;
            }
            Ok(())
        })?;

        Ok(true)
    }

    /// L2: Build trees from JGrid content blocks
    fn try_materialize_from_grid(
        &self,
        packages: &[LayoutPackage],
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
            // A missing manifest (corrupt/GC'd store, version mismatch) must be a
            // normal cache miss, not a panic: release builds use `panic = "abort"`.
            let manifest = self
                .grid_store
                .find_manifest(&pkg.name, &pkg.version)
                .ok_or_else(|| {
                    format!("jgrid: manifest not found for {}@{}", pkg.name, pkg.version)
                })?;
            let tree_hash = manifest_tree_hash(manifest, self.grid_store.string_table());
            let tree_path = self.tree_cache.tree_path(&tree_hash);

            if !self.tree_cache.has_tree(&tree_hash) {
                let candidate = create_unique_dir(self.tree_cache.base_dir(), "tree")?;
                let built_hash = match materialize_tree_from_store(
                    self.grid_store,
                    manifest,
                    self.grid_store.string_table(),
                    &candidate,
                ) {
                    Ok(hash) => hash,
                    Err(error) => {
                        remove_path(&candidate);
                        return Err(error);
                    }
                };
                if built_hash != tree_hash {
                    remove_path(&candidate);
                    return Err(format!(
                        "JGrid tree hash mismatch for {}@{}",
                        pkg.name, pkg.version
                    ));
                }
                publish_directory(&candidate, &tree_path)?;
                self.tree_cache.mark_complete(&tree_hash)?;
            }

            tree_info.push((pkg.install_path.clone(), tree_hash));
        }

        // Clone into staging
        self.build_staging_and_clone(node_modules, dep_hash, |staging_nm| {
            for (install_path, tree_hash) in &tree_info {
                let tree_path = self.tree_cache.tree_path(tree_hash);
                let pkg_dir = staging_nm.join(install_path);
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
        if let Some(parent) = node_modules.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create parent {parent:?}: {e}"))?;
        }
        let _install_lock = crate::file_lock::FileLock::lock(node_modules)
            .map_err(|e| format!("lock node_modules publication: {e}"))?;

        let bundle_parent = self.bundle_cache.base_dir();
        std::fs::create_dir_all(bundle_parent).map_err(|e| format!("create bundle cache: {e}"))?;
        let candidate = create_unique_dir(bundle_parent, "bundle")?;
        let candidate_nm = candidate.join("node_modules");
        let result = (|| {
            std::fs::create_dir(&candidate_nm)
                .map_err(|e| format!("create candidate node_modules: {e}"))?;
            populate(&candidate_nm)?;
            crate::bin_links::rebuild_bin_links(&candidate_nm)
                .map_err(|e| format!("build bundle .bin links: {e}"))?;
            crate::file_lock::AtomicWrite::write_to_file(
                &candidate.join(".jhol-complete"),
                b"jhol-jgrid-bundle-v1\n",
            )
            .map_err(|e| format!("write bundle proof: {e}"))?;

            let bundle_path = self.bundle_cache.bundle_path(dep_hash);
            publish_directory(&candidate, &bundle_path)?;

            let project_parent = node_modules.parent().unwrap_or(Path::new("."));
            let project_candidate = unique_path(project_parent, "node_modules")?;
            if let Err(error) = Cloner::clone_directory(
                &self.bundle_cache.bundle_node_modules_path(dep_hash),
                &project_candidate,
            ) {
                remove_path(&project_candidate);
                return Err(error);
            }
            if let Err(error) = publish_directory(&project_candidate, node_modules) {
                remove_path(&project_candidate);
                return Err(error);
            }
            Ok(())
        })();

        if result.is_err() {
            remove_path(&candidate);
        }
        result
    }
}

fn manifest_tree_hash(
    manifest: &crate::jgrid::Manifest,
    string_table: &crate::jgrid::FormatStringTable,
) -> [u8; 32] {
    let entries: Vec<(&str, u16, [u8; 32])> = manifest
        .files
        .iter()
        .map(|file| {
            (
                string_table.get(file.path_offset),
                file.mode,
                file.content_hash,
            )
        })
        .collect();
    crate::jgrid::Manifest::compute_tree_hash(&entries)
}

fn safe_install_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

pub(crate) fn unique_path(parent: &Path, prefix: &str) -> Result<std::path::PathBuf, String> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    for _ in 0..100 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{prefix}.{}.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            id
        ));
        if std::fs::symlink_metadata(&path).is_err() {
            return Ok(path);
        }
    }
    Err(format!("could not allocate unique path in {parent:?}"))
}

pub(crate) fn create_unique_dir(parent: &Path, prefix: &str) -> Result<std::path::PathBuf, String> {
    for _ in 0..100 {
        let path = unique_path(parent, prefix)?;
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create staging directory {path:?}: {error}")),
        }
    }
    Err(format!(
        "could not allocate staging directory in {parent:?}"
    ))
}

pub(crate) fn publish_directory(candidate: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("create parent {parent:?}: {e}"))?;
    let backup = unique_path(parent, "previous")?;
    let had_destination = std::fs::symlink_metadata(destination).is_ok();
    if had_destination {
        std::fs::rename(destination, &backup)
            .map_err(|e| format!("preserve existing output {destination:?}: {e}"))?;
    }
    if let Err(error) = std::fs::rename(candidate, destination) {
        if had_destination {
            let _ = std::fs::rename(&backup, destination);
        }
        return Err(format!("publish output {destination:?}: {error}"));
    }
    if had_destination {
        remove_path(&backup);
    }
    Ok(())
}

pub(crate) fn remove_path(path: &Path) {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jgrid::ingester::ingest_raw_files;
    use crate::jgrid::store::{BundleCache, GridBuilder, TreeCache};
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
    fn exact_layout_materializes_nested_versions_and_isolates_projects() {
        let dir = tempdir().unwrap();
        let (store, tree_cache, bundle_cache) = setup_test_store(
            dir.path(),
            &[
                (
                    "dep",
                    "1.0.0",
                    &[("package.json", 0o644, b"{\"version\":\"1.0.0\"}" as &[u8])],
                ),
                (
                    "dep",
                    "2.0.0",
                    &[("package.json", 0o644, b"{\"version\":\"2.0.0\"}")],
                ),
            ],
        );
        let materializer = Materializer::new(&store, &tree_cache, &bundle_cache);
        let layout = vec![
            LayoutPackage {
                name: "dep".into(),
                version: "1.0.0".into(),
                install_path: "dep".into(),
            },
            LayoutPackage {
                name: "dep".into(),
                version: "2.0.0".into(),
                install_path: "parent/node_modules/dep".into(),
            },
        ];
        let first = dir.path().join("first/node_modules");
        let second = dir.path().join("second/node_modules");
        assert!(materializer.materialize_layout(&layout, &first).unwrap());
        assert!(materializer.materialize_layout(&layout, &second).unwrap());
        assert!(std::fs::read_to_string(first.join("dep/package.json"))
            .unwrap()
            .contains("1.0.0"));
        assert!(
            std::fs::read_to_string(first.join("parent/node_modules/dep/package.json"))
                .unwrap()
                .contains("2.0.0")
        );

        std::fs::write(first.join("dep/package.json"), "modified").unwrap();
        assert!(std::fs::read_to_string(second.join("dep/package.json"))
            .unwrap()
            .contains("1.0.0"));
        let dep_v1 = store.find_manifest("dep", "1.0.0").unwrap();
        let dep_v2 = store.find_manifest("dep", "2.0.0").unwrap();
        let hash_v1 = hex::encode(manifest_tree_hash(dep_v1, store.string_table()));
        let hash_v2 = hex::encode(manifest_tree_hash(dep_v2, store.string_table()));
        assert!(std::fs::read_to_string(
            bundle_cache
                .bundle_node_modules_path(&BundleCache::compute_verified_layout_hash(&[
                    ("dep", "1.0.0", "dep", &hash_v1),
                    ("dep", "2.0.0", "parent/node_modules/dep", &hash_v2),
                ]))
                .join("dep/package.json")
        )
        .unwrap()
        .contains("1.0.0"));
    }

    #[test]
    fn failed_publish_keeps_previous_project_tree() {
        let dir = tempdir().unwrap();
        let existing = dir.path().join("node_modules");
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("sentinel"), b"old").unwrap();
        let missing_candidate = dir.path().join("candidate");
        assert!(publish_directory(&missing_candidate, &existing).is_err());
        assert_eq!(std::fs::read(existing.join("sentinel")).unwrap(), b"old");
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
        bundle_cache.mark_complete(&hash).unwrap();
        assert!(bundle_cache.has_bundle(&hash));
    }
}
