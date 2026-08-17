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
}
