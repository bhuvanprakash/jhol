//! Binary Package Cache - Pre-built binary packages for instant installs
//! Similar to Bun's approach - download pre-built binaries instead of source tarballs

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

/// Binary package metadata
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BinaryPackage {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub download_url: String,
    pub integrity: String,
    pub size: u64,
}

/// Binary package cache
pub struct BinaryCache {
    cache_dir: PathBuf,
    packages: HashMap<String, BinaryPackage>,
}

impl BinaryCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        let cache_dir = cache_dir.join("binary-cache");
        let _ = fs::create_dir_all(&cache_dir);
        
        Self {
            cache_dir,
            packages: HashMap::new(),
        }
    }
    
    /// Check if binary package is available
    pub fn has_binary(&self, package: &str, version: &str) -> bool {
        let key = format!("{}@{}", package, version);
        self.packages.contains_key(&key)
    }
    
    /// Get binary package info
    pub fn get_binary(&self, package: &str, version: &str) -> Option<&BinaryPackage> {
        let key = format!("{}@{}", package, version);
        self.packages.get(&key)
    }
    
    /// Add binary package to cache
    pub fn add_binary(&mut self, pkg: BinaryPackage) {
        let key = format!("{}@{}", pkg.name, pkg.version);
        self.packages.insert(key, pkg);
    }
    
    /// Get cached binary file path
    pub fn get_cached_path(&self, package: &str, version: &str) -> Option<PathBuf> {
        let key = format!("{}@{}", package, version);
        if self.packages.contains_key(&key) {
            let path = self.cache_dir.join(format!("{}.bin", key.replace('/', "%")));
            if path.exists() {
                return Some(path);
            }
        }
        None
    }
    
    /// Save binary to cache
    pub fn save_binary(&self, package: &str, version: &str, data: &[u8]) -> Result<PathBuf, String> {
        let key = format!("{}@{}", package, version);
        let path = self.cache_dir.join(format!("{}.bin", key.replace('/', "%")));
        
        fs::write(&path, data)
            .map_err(|e| format!("Failed to write binary cache: {}", e))?;
        
        Ok(path)
    }
}

/// Get current platform string
pub fn get_current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    #[cfg(target_arch = "aarch64")]
    return "macos-arm64";
    
    #[cfg(target_os = "macos")]
    #[cfg(target_arch = "x86_64")]
    return "macos-x64";
    
    #[cfg(target_os = "linux")]
    #[cfg(target_arch = "x86_64")]
    return "linux-x64";
    
    #[cfg(target_os = "linux")]
    #[cfg(target_arch = "aarch64")]
    return "linux-arm64";
    
    "unknown"
}

/// Try to fetch binary package (returns None if not available, falls back to source)
pub async fn try_fetch_binary(
    package: &str,
    version: &str,
) -> Option<BinaryPackage> {
    let platform = get_current_platform();
    
    // Check if binary is available from CDN
    let url = format!(
        "https://cdn.jhol.dev/binaries/{}/{}/{}.bin",
        package, version, platform
    );
    
    // In production, would check URL existence
    // For now, return None to fall back to source
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut cache = BinaryCache::new(temp_dir.path().to_path_buf());
        
        let pkg = BinaryPackage {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            platform: "macos-arm64".to_string(),
            download_url: "https://example.com/test.bin".to_string(),
            integrity: "sha256-abc".to_string(),
            size: 1024,
        };
        
        cache.add_binary(pkg);
        assert!(cache.has_binary("test", "1.0.0"));
    }
}
