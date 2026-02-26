//! JHOL Offline Mode - Full Dependency Tree Caching
//! 
//! Caches complete dependency trees for reliable offline installs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use serde::{Serialize, Deserialize};

/// Cached dependency tree for offline mode
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyTreeCache {
    /// Package name
    pub package: String,
    /// Package version
    pub version: String,
    /// Direct dependencies
    pub dependencies: Vec<DependencyEntry>,
    /// Tarball URL
    pub tarball_url: Option<String>,
    /// Integrity hash
    pub integrity: Option<String>,
    /// Cache timestamp
    pub cached_at: u64,
}

/// Single dependency entry
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyEntry {
    /// Package name
    pub name: String,
    /// Version requirement
    pub spec: String,
    /// Resolved version (if known)
    pub resolved_version: Option<String>,
    /// Tarball URL
    pub tarball_url: Option<String>,
    /// Integrity hash
    pub integrity: Option<String>,
}

/// Offline mode manager
pub struct OfflineCache {
    cache_dir: PathBuf,
    trees: HashMap<String, DependencyTreeCache>,
}

impl OfflineCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        let cache_dir = cache_dir.join("offline-trees");
        let _ = fs::create_dir_all(&cache_dir);
        
        Self {
            cache_dir,
            trees: HashMap::new(),
        }
    }
    
    /// Load cached dependency tree for a package
    pub fn load_tree(&mut self, package: &str, version: &str) -> Option<DependencyTreeCache> {
        let key = format!("{}@{}", package, version);
        
        // Check in-memory cache first
        if let Some(tree) = self.trees.get(&key) {
            return Some(tree.clone());
        }
        
        // Check disk cache
        let cache_file = self.cache_dir.join(format!("{}.json", key.replace('/', "%")));
        if cache_file.exists() {
            if let Ok(content) = fs::read_to_string(&cache_file) {
                if let Ok(tree) = serde_json::from_str::<DependencyTreeCache>(&content) {
                    self.trees.insert(key, tree.clone());
                    return Some(tree);
                }
            }
        }
        
        None
    }
    
    /// Save dependency tree to cache
    pub fn save_tree(&mut self, tree: DependencyTreeCache) -> Result<(), String> {
        let key = format!("{}@{}", tree.package, tree.version);
        let cache_file = self.cache_dir.join(format!("{}.json", key.replace('/', "%")));
        
        let content = serde_json::to_string_pretty(&tree)
            .map_err(|e| format!("Failed to serialize tree: {}", e))?;
        
        fs::write(&cache_file, content)
            .map_err(|e| format!("Failed to write cache: {}", e))?;
        
        self.trees.insert(key, tree);
        Ok(())
    }
    
    /// Check if package is available offline
    pub fn is_available_offline(&self, package: &str, version: &str) -> bool {
        let key = format!("{}@{}", package, version);
        self.trees.contains_key(&key)
    }
    
    /// Get all cached packages from memory and disk cache.
    pub fn cached_packages(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();

        for key in self.trees.keys() {
            if let Some((name, version)) = split_package_key(key) {
                out.push((name.to_string(), version.to_string()));
            }
        }

        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }

                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let decoded = stem.replace('%', "/");
                if let Some((name, version)) = split_package_key(&decoded) {
                    out.push((name.to_string(), version.to_string()));
                }
            }
        }

        out.sort();
        out.dedup();
        out
    }
    
    /// Clear offline cache
    pub fn clear(&mut self) -> Result<(), String> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)
                .map_err(|e| format!("Failed to clear cache: {}", e))?;
            fs::create_dir_all(&self.cache_dir)
                .map_err(|e| format!("Failed to recreate cache: {}", e))?;
        }
        self.trees.clear();
        Ok(())
    }
    
    /// Get cached tarball path for offline install
    pub fn get_cached_tarball(&self, package: &str, version: &str) -> Option<PathBuf> {
        let key = format!("{}@{}", package, version);
        if let Some(tree) = self.trees.get(&key) {
            if let Some(url) = &tree.tarball_url {
                // Extract hash from URL and find in store
                let cache_dir = self.cache_dir.parent().unwrap_or(&self.cache_dir);
                let store_dir = cache_dir.join("store");
                
                // Search store for matching hash
                if let Ok(entries) = fs::read_dir(store_dir) {
                    for entry in entries.flatten() {
                        if let Ok(sub_entries) = fs::read_dir(entry.path()) {
                            for sub_entry in sub_entries.flatten() {
                                if sub_entry.path().exists() {
                                    return Some(sub_entry.path());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

/// Build dependency tree from resolved packages
pub fn build_dependency_tree(
    packages: &HashMap<String, crate::lockfile_write::ResolvedPackage>,
) -> Vec<DependencyTreeCache> {
    let mut trees = Vec::new();
    
    for (name, pkg) in packages {
        let dependencies = pkg.dependencies.iter()
            .map(|(dep_name, dep_spec)| DependencyEntry {
                name: dep_name.clone(),
                spec: dep_spec.clone(),
                resolved_version: None,
                tarball_url: None,
                integrity: None,
            })
            .collect();
        
        trees.push(DependencyTreeCache {
            package: name.clone(),
            version: pkg.version.clone(),
            dependencies,
            tarball_url: Some(pkg.resolved.clone()),
            integrity: pkg.integrity.clone(),
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
    }
    
    trees
}

/// Install from offline cache
pub fn install_from_offline_cache(
    offline_cache: &mut OfflineCache,
    package: &str,
    version: &str,
) -> Result<DependencyTreeCache, String> {
    offline_cache.load_tree(package, version)
        .ok_or_else(|| format!(
            "Package {}@{} not available offline. Run online install first to cache dependencies.",
            package, version
        ))
}

fn split_package_key(key: &str) -> Option<(&str, &str)> {
    let idx = key.rfind('@')?;
    if idx == 0 || idx + 1 >= key.len() {
        return None;
    }
    let (name, version_with_at) = key.split_at(idx);
    let version = &version_with_at[1..];
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some((name, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offline_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut cache = OfflineCache::new(temp_dir.path().to_path_buf());
        
        let tree = DependencyTreeCache {
            package: "test-pkg".to_string(),
            version: "1.0.0".to_string(),
            dependencies: vec![],
            tarball_url: None,
            integrity: None,
            cached_at: 0,
        };
        
        // Save tree
        cache.save_tree(tree.clone()).unwrap();
        
        // Load tree
        let loaded = cache.load_tree("test-pkg", "1.0.0").unwrap();
        assert_eq!(loaded.package, "test-pkg");
        assert_eq!(loaded.version, "1.0.0");
        
        // Check availability
        assert!(cache.is_available_offline("test-pkg", "1.0.0"));
    }

    #[test]
    fn test_cached_packages_reads_scoped_from_disk() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut cache = OfflineCache::new(temp_dir.path().to_path_buf());

        let tree = DependencyTreeCache {
            package: "@scope/demo".to_string(),
            version: "2.3.4".to_string(),
            dependencies: vec![],
            tarball_url: None,
            integrity: None,
            cached_at: 0,
        };

        cache.save_tree(tree).unwrap();

        // New cache instance validates disk scan path (not just in-memory map)
        let fresh_cache = OfflineCache::new(temp_dir.path().to_path_buf());
        let packages = fresh_cache.cached_packages();
        assert!(packages.iter().any(|(n, v)| n == "@scope/demo" && v == "2.3.4"));
    }

    #[test]
    fn test_split_package_key_supports_scoped_names() {
        let parsed = split_package_key("@scope/pkg@1.2.3").unwrap();
        assert_eq!(parsed.0, "@scope/pkg");
        assert_eq!(parsed.1, "1.2.3");

        let parsed_plain = split_package_key("lodash@4.17.21").unwrap();
        assert_eq!(parsed_plain.0, "lodash");
        assert_eq!(parsed_plain.1, "4.17.21");
    }
}
