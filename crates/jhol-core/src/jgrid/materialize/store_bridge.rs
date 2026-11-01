//! Store-unpacked → JGrid L1 bridge (world-class CAS materialization).
//!
//! Latest package-manager practice (pnpm CAFS, LPM v2/v3, NativeLink/Bazel,
//! conda APFS clonefile) keeps **immutable content-addressed trees** and
//! materializes projects with **whole-directory COW clones** (`clonefile` /
//! reflink), not per-file walks from a giant mmap blob.
//!
//! Jhol already pays for `store_unpacked/<shard>/<hash>/`. This module treats
//! those dirs as L1 trees: if every requested package has a complete unpacked
//! CAS dir, we clonefile them into a staging `node_modules`, publish a durable
//! L0 graph-keyed bundle, then clone once into the project. No tarball re-extract
//! and no JGrid block rebuild on the hot path.

use super::cloner::Cloner;
use super::installer::{
    create_unique_dir, publish_directory, remove_path, unique_path, LayoutPackage,
};
use crate::jgrid::store::BundleCache;
use std::path::{Path, PathBuf};

/// Env-gated route marker so paper-matrix / profilers can prove which tier fired.
pub(crate) fn emit_route(tier: &str) {
    // Always print a stable token when the arm is under measurement; otherwise
    // only under the debug flag so normal installs stay quiet.
    let force = std::env::var_os("JHOL_JGRID_ROUTE").is_some()
        || std::env::var_os("JHOL_DEBUG_JGRID").is_some()
        || std::env::var_os("JHOL_PROFILE_INSTALL").is_some();
    if force {
        eprintln!("[JGRID] {tier}");
    }
}

/// Look up a complete `store_unpacked` tree for `name@version`.
pub(crate) fn unpacked_source_for(name: &str, version: &str) -> Option<PathBuf> {
    let spec = format!("{name}@{version}");
    let dir = crate::utils::with_store_index(|index| {
        index
            .get(&spec)
            .map(|h| crate::utils::sharded_unpacked_path(h))
    })?;
    if crate::registry::store_extract_complete(&dir) {
        Some(dir)
    } else {
        None
    }
}

/// True when every layout package has a complete CAS unpacked tree.
pub(crate) fn all_have_unpacked(packages: &[LayoutPackage]) -> bool {
    packages
        .iter()
        .all(|pkg| unpacked_source_for(&pkg.name, &pkg.version).is_some())
}

/// Materialize from `store_unpacked` (L1-store), publish L0 under the light
/// dep-set hash, and clone into `node_modules`.
pub(crate) fn materialize_from_store_unpacked(
    packages: &[LayoutPackage],
    node_modules: &Path,
    bundle_cache: &BundleCache,
) -> Result<bool, String> {
    if packages.is_empty() {
        return Ok(true);
    }

    let light_refs: Vec<(&str, &str)> = packages
        .iter()
        .map(|p| (p.name.as_str(), p.version.as_str()))
        .collect();
    let light_hash = BundleCache::compute_dep_set_hash(&light_refs);

    // Fast path: L0 bundle hit (O(1) APFS directory clone)
    if bundle_cache.has_bundle(&light_hash) {
        let bundle_nm = bundle_cache.bundle_node_modules_path(&light_hash);
        if !node_modules.exists() {
            if let Some(parent) = node_modules.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if Cloner::clone_directory(&bundle_nm, node_modules).is_ok() {
                emit_route("L0");
                return Ok(true);
            }
        }
        if let Some(parent) = node_modules.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create parent {parent:?}: {e}"))?;
        }
        let _lock = crate::file_lock::FileLock::lock(node_modules)
            .map_err(|e| format!("lock node_modules publication: {e}"))?;
        let project_parent = node_modules.parent().unwrap_or(Path::new("."));
        let project_candidate = unique_path(project_parent, "node_modules")?;
        if let Err(error) = Cloner::clone_directory(&bundle_nm, &project_candidate) {
            remove_path(&project_candidate);
            return Err(error);
        }
        if let Err(error) = publish_directory(&project_candidate, node_modules) {
            remove_path(&project_candidate);
            return Err(error);
        }
        emit_route("L0");
        return Ok(true);
    }

    // Verify all packages are available in store_unpacked
    let mut sources: Vec<(&LayoutPackage, PathBuf)> = Vec::with_capacity(packages.len());
    for pkg in packages {
        let Some(dir) = unpacked_source_for(&pkg.name, &pkg.version) else {
            return Ok(false);
        };
        sources.push((pkg, dir));
    }

    if let Some(parent) = node_modules.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create parent {parent:?}: {e}"))?;
    }
    let _lock = crate::file_lock::FileLock::lock(node_modules)
        .map_err(|e| format!("lock node_modules publication: {e}"))?;

    let project_parent = node_modules.parent().unwrap_or(Path::new("."));
    let project_candidate = unique_path(project_parent, "node_modules")?;

    let result = (|| {
        std::fs::create_dir(&project_candidate)
            .map_err(|e| format!("create candidate node_modules: {e}"))?;

        // Parallel COW clones — NativeLink measured ~4× wins from clonefile over
        // per-file hardlink walks; cloning whole package trees is the same shape.
        let errors: Vec<String> = {
            use rayon::prelude::*;
            sources
                .par_iter()
                .filter_map(|(pkg, src)| {
                    let dst = project_candidate.join(&pkg.install_path);
                    if let Some(parent) = dst.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    Cloner::clone_directory(src, &dst).err()
                })
                .collect()
        };
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }

        crate::bin_links::rebuild_bin_links(&project_candidate)
            .map_err(|e| format!("build project .bin links: {e}"))?;

        publish_directory(&project_candidate, node_modules)?;

        // Durable L0 publish: single atomic directory CoW clone from node_modules into bundle cache
        let bundle_parent = bundle_cache.base_dir();
        if std::fs::create_dir_all(bundle_parent).is_ok() {
            if let Ok(bundle_cand) = create_unique_dir(bundle_parent, "bundle") {
                let bundle_cand_nm = bundle_cand.join("node_modules");
                if Cloner::clone_directory(node_modules, &bundle_cand_nm).is_ok() {
                    let _ = crate::file_lock::AtomicWrite::write_to_file(
                        &bundle_cand.join(".jhol-complete"),
                        b"jhol-jgrid-bundle-v1\n",
                    );
                    let bundle_path = bundle_cache.bundle_path(&light_hash);
                    let _ = publish_directory(&bundle_cand, &bundle_path);
                } else {
                    remove_path(&bundle_cand);
                }
            }
        }
        Ok(())
    })();

    if result.is_err() {
        remove_path(&project_candidate);
    }
    result?;
    emit_route("L1-store");
    Ok(true)
}

/// Publish a durable L0 bundle by cloning an existing project `node_modules`.
/// Cheap on APFS (one directory clonefile); this is the Instant-Lock equivalent
/// that lives in the durable cache instead of `/tmp`.
pub(crate) fn publish_l0_from_node_modules(
    packages: &[(String, String)],
    node_modules: &Path,
    bundle_cache: &BundleCache,
    prepared_identity: Option<&str>,
) -> Result<bool, String> {
    if packages.is_empty() || !node_modules.is_dir() {
        return Ok(false);
    }
    let refs: Vec<(&str, &str)> = packages
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let hash = prepared_identity
        .map(str::to_owned)
        .unwrap_or_else(|| BundleCache::compute_dep_set_hash(&refs));
    if bundle_cache.has_bundle(&hash) {
        return Ok(false);
    }

    let bundle_parent = bundle_cache.base_dir();
    std::fs::create_dir_all(bundle_parent).map_err(|e| format!("create bundle cache: {e}"))?;
    let candidate = create_unique_dir(bundle_parent, "bundle")?;
    let candidate_nm = candidate.join("node_modules");
    let result: Result<(), String> = (|| {
        Cloner::clone_directory(node_modules, &candidate_nm)?;
        crate::file_lock::AtomicWrite::write_to_file(
            &candidate.join(".jhol-complete"),
            b"jhol-jgrid-bundle-v1\n",
        )
        .map_err(|e| format!("write bundle proof: {e}"))?;
        publish_directory(&candidate, &bundle_cache.bundle_path(&hash))?;
        Ok(())
    })();
    if result.is_err() {
        remove_path(&candidate);
    }
    result?;
    emit_route("L0-publish");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jgrid::materialize::LayoutPackage;
    use tempfile::tempdir;

    #[test]
    fn light_l0_roundtrip_from_synthetic_unpacked() {
        let dir = tempdir().unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        // Isolate store index + unpacked under this cache.
        std::env::set_var("JHOL_CACHE_DIR", &cache);

        let hash = "aa".repeat(32);
        let unpacked = crate::utils::sharded_unpacked_path(&hash);
        std::fs::create_dir_all(&unpacked).unwrap();
        std::fs::write(
            unpacked.join("package.json"),
            r#"{"name":"ms","version":"2.1.3"}"#,
        )
        .unwrap();
        std::fs::write(unpacked.join("index.js"), "module.exports=1").unwrap();
        crate::registry::mark_store_extract_complete_pub(&unpacked);

        let mut index = std::collections::HashMap::new();
        index.insert("ms@2.1.3".into(), hash.clone());
        let _ = crate::utils::write_store_index(&index);

        let bundles = BundleCache::new(&cache.join("grid"));
        let packages = vec![LayoutPackage {
            name: "ms".into(),
            version: "2.1.3".into(),
            install_path: "ms".into(),
        }];
        let nm = dir.path().join("node_modules");
        assert!(materialize_from_store_unpacked(&packages, &nm, &bundles).unwrap());
        assert!(nm.join("ms").join("index.js").is_file());
        assert!(bundles.has_bundle(&BundleCache::compute_dep_set_hash(&[("ms", "2.1.3")])));

        // Second call hits L0.
        let nm2 = dir.path().join("nm2");
        assert!(materialize_from_store_unpacked(&packages, &nm2, &bundles).unwrap());
        assert!(nm2.join("ms").join("index.js").is_file());

        std::env::remove_var("JHOL_CACHE_DIR");
    }
}
