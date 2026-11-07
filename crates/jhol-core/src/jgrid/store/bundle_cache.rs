use std::fs;
use std::path::{Path, PathBuf};

/// Manages clone-ready bundles at ~/.jhol-cache/bundles/<dep_set_hash>/
pub struct BundleCache {
    base_dir: PathBuf,
}

impl BundleCache {
    pub fn new(cache_dir: &Path) -> Self {
        let base_dir = cache_dir.join("bundles");
        fs::create_dir_all(&base_dir).ok();
        Self { base_dir }
    }

    /// Fresh-project replay key: byte-identical `package.json` on THIS platform.
    /// Single definition — the publish side and the replay lane both derive the
    /// alias name from this, so a bundle published by one install is reachable by
    /// the next identical install even before a lockfile exists.
    pub fn deps_manifest_key(pj_bytes: &[u8]) -> String {
        use xxhash_rust::xxh3::xxh3_64;
        let mut seed = Vec::with_capacity(pj_bytes.len() + 32);
        seed.extend_from_slice(b"jhol-bundle-deps-v2");
        seed.push(0);
        seed.extend_from_slice(std::env::consts::OS.as_bytes());
        seed.push(0);
        seed.extend_from_slice(std::env::consts::ARCH.as_bytes());
        seed.push(0);
        seed.extend_from_slice(pj_bytes);
        format!("{:016x}", xxh3_64(&seed))
    }

    /// Compute the hash for a dependency set
    pub fn compute_dep_set_hash(packages: &[(&str, &str)]) -> String {
        let mut sorted: Vec<(&str, &str)> = packages.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));

        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        for (name, version) in &sorted {
            hasher.update(name.as_bytes());
            hasher.update(b"\0");
            hasher.update(version.as_bytes());
            hasher.update(b"\0");
        }
        let hash = hasher.digest();
        format!("{:016x}", hash)
    }

    /// Hash package identities and exact install paths for nested layouts.
    pub fn compute_layout_hash(packages: &[(&str, &str, &str)]) -> String {
        let mut sorted = packages.to_vec();
        sorted.sort_by(|a, b| {
            a.2.cmp(b.2)
                .then_with(|| a.0.cmp(b.0))
                .then_with(|| a.1.cmp(b.1))
        });
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(b"jhol-jgrid-layout-v1\0");
        for (name, version, install_path) in sorted {
            for value in [name, version, install_path] {
                hasher.update(&(value.len() as u64).to_le_bytes());
                hasher.update(value.as_bytes());
            }
        }
        format!("{:016x}", hasher.digest())
    }

    /// Bind a cached layout to the canonical content hash for every package.
    pub fn compute_verified_layout_hash(packages: &[(&str, &str, &str, &str)]) -> String {
        let mut sorted = packages.to_vec();
        sorted.sort_by(|a, b| {
            a.2.cmp(b.2)
                .then_with(|| a.0.cmp(b.0))
                .then_with(|| a.1.cmp(b.1))
        });
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(b"jhol-jgrid-verified-layout-v2\0");
        for (name, version, install_path, tree_hash) in sorted {
            for value in [name, version, install_path, tree_hash] {
                hasher.update(&(value.len() as u64).to_le_bytes());
                hasher.update(value.as_bytes());
            }
        }
        format!("{:016x}", hasher.digest())
    }

    /// Path to a specific bundle's node_modules directory
    pub fn bundle_path(&self, dep_hash: &str) -> PathBuf {
        self.base_dir.join(dep_hash)
    }

    pub fn bundle_node_modules_path(&self, dep_hash: &str) -> PathBuf {
        self.bundle_path(dep_hash).join("node_modules")
    }

    /// Check if a bundle exists
    pub fn has_bundle(&self, dep_hash: &str) -> bool {
        self.bundle_node_modules_path(dep_hash).is_dir()
            && self.bundle_path(dep_hash).join(".jhol-complete").is_file()
    }

    /// Mark a bundle ready after its package tree and `.bin` links are complete.
    pub fn mark_complete(&self, dep_hash: &str) -> Result<(), String> {
        let marker = self.bundle_path(dep_hash).join(".jhol-complete");
        crate::file_lock::AtomicWrite::write_to_file(&marker, b"jhol-jgrid-bundle-v1\n")
            .map_err(|e| format!("write bundle completion proof: {e}"))
    }

    /// Create a lightweight alias pointing from `alias_hash` to `target_hash`.
    pub fn alias_bundle(&self, target_hash: &str, alias_hash: &str) -> Result<(), String> {
        if target_hash == alias_hash || alias_hash.is_empty() {
            return Ok(());
        }
        let target = self.bundle_path(target_hash);
        let alias = self.bundle_path(alias_hash);
        if !target.exists() || alias.exists() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(&target, &alias).is_ok() {
                return Ok(());
            }
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(&target, &alias).is_ok() {
                return Ok(());
            }
        }
        crate::jgrid::materialize::Cloner::clone_directory(&target, &alias).ok();
        Ok(())
    }

    /// Create the bundle directory structure
    pub fn create_bundle(&self, dep_hash: &str) -> Result<PathBuf, String> {
        let nm_path = self.bundle_node_modules_path(dep_hash);
        fs::create_dir_all(&nm_path).map_err(|e| format!("create bundle: {e}"))?;
        Ok(nm_path)
    }

    /// Remove a specific bundle
    pub fn remove_bundle(&self, dep_hash: &str) -> Result<(), String> {
        let path = self.bundle_path(dep_hash);
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|e| format!("remove bundle: {e}"))?;
        }
        Ok(())
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dep_hash_stability() {
        let pkgs1 = vec![("lodash", "4.17.21"), ("chalk", "5.3.0")];
        let pkgs2 = vec![("chalk", "5.3.0"), ("lodash", "4.17.21")];
        assert_eq!(
            BundleCache::compute_dep_set_hash(&pkgs1),
            BundleCache::compute_dep_set_hash(&pkgs2),
            "hash should be order-independent"
        );
    }

    #[test]
    fn test_dep_hash_different() {
        let pkgs1 = vec![("lodash", "4.17.21")];
        let pkgs2 = vec![("lodash", "4.17.22")];
        assert_ne!(
            BundleCache::compute_dep_set_hash(&pkgs1),
            BundleCache::compute_dep_set_hash(&pkgs2)
        );
    }

    #[test]
    fn partial_bundle_directory_is_not_a_cache_hit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = BundleCache::new(dir.path());
        cache.create_bundle("test").unwrap();
        assert!(!cache.has_bundle("test"));
        cache.mark_complete("test").unwrap();
        assert!(cache.has_bundle("test"));
    }
}
