//! JHOL Global Shared Cache - Memory-mapped package store
//! 
//! Like pnpm's store but faster - uses memory-mapped files
//! for zero-copy reads across all projects

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};
use std::sync::{Arc, RwLock};

/// Global shared cache instance
pub struct GlobalCache {
    /// Cache directory (shared across all projects)
    cache_dir: PathBuf,
    /// Package index: package@version -> cache entry
    index: Arc<RwLock<HashMap<String, CacheEntry>>>,
    /// Memory-mapped file handles for zero-copy reads
    mmap_files: Arc<RwLock<HashMap<String, memmap2::Mmap>>>,
}

/// Cache entry metadata
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CacheEntry {
    pub package: String,
    pub version: String,
    pub hash: String,
    pub size: u64,
    pub essential_size: u64,
    pub savings_percent: f64,
    pub cached_at: u64,
    pub accessed_count: u64,
}

impl GlobalCache {
    /// Create or open global cache
    pub fn new() -> Result<Self, String> {
        // Use system-wide cache directory
        let cache_dir = if cfg!(target_os = "macos") {
            dirs::cache_dir()
                .ok_or("No cache dir")?
                .join("jhol")
                .join("global-store")
        } else if cfg!(target_os = "linux") {
            std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| dirs::cache_dir().unwrap_or_else(|| PathBuf::from("~/.cache")))
                .join("jhol")
                .join("global-store")
        } else {
            // Windows or fallback
            dirs::cache_dir()
                .ok_or("No cache dir")?
                .join("jhol")
                .join("global-store")
        };
        
        fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;
        
        // Load index
        let index_path = cache_dir.join("index.json");
        let index = if index_path.exists() {
            let content = fs::read_to_string(&index_path)
                .map_err(|e| format!("Failed to read index: {}", e))?;
            serde_json::from_str(&content)
                .unwrap_or_else(|_| HashMap::new())
        } else {
            HashMap::new()
        };
        
        Ok(Self {
            cache_dir,
            index: Arc::new(RwLock::new(index)),
            mmap_files: Arc::new(RwLock::new(HashMap::new())),
        })
    }
    
    /// Check if package is in global cache
    pub fn has(&self, package: &str, version: &str) -> bool {
        let key = format!("{}@{}", package, version);
        self.index.read().unwrap().contains_key(&key)
    }
    
    /// Get package from cache (zero-copy read via mmap)
    pub fn get(&self, package: &str, version: &str) -> Option<Vec<u8>> {
        let key = format!("{}@{}", package, version);
        
        // Update access count
        if let Some(entry) = self.index.write().unwrap().get_mut(&key) {
            entry.accessed_count += 1;
        }
        
        // Try memory-mapped read
        if let Some(mmap) = self.mmap_files.read().unwrap().get(&key) {
            return Some(mmap.to_vec());
        }
        
        // Fall back to file read
        let entry = self.index.read().unwrap().get(&key)?.clone();
        let file_path = self.cache_dir.join(format!("{}.pkg", entry.hash));
        
        if file_path.exists() {
            let mut file = File::open(&file_path).ok()?;
            let mut data = Vec::new();
            file.read_to_end(&mut data).ok()?;
            
            // Memory-map for future access
            if let Ok(mmap) = unsafe { memmap2::Mmap::map(&file) } {
                self.mmap_files.write().unwrap().insert(key, mmap);
            }
            
            Some(data)
        } else {
            None
        }
    }
    
    /// Add package to global cache
    pub fn add(&self, package: &str, version: &str, data: &[u8]) -> Result<CacheEntry, String> {
        use sha2::{Digest, Sha256};
        
        let hash = format!("{:x}", Sha256::digest(data));
        let key = format!("{}@{}", package, version);
        
        // Check if already cached
        if self.has(package, version) {
            return Ok(self.index.read().unwrap().get(&key).unwrap().clone());
        }
        
        // Write to cache file
        let file_path = self.cache_dir.join(format!("{}.pkg", hash));
        File::create(&file_path)
            .map_err(|e| format!("Failed to create cache file: {}", e))?
            .write_all(data)
            .map_err(|e| format!("Failed to write cache: {}", e))?;
        
        // Memory-map for future access
        let file = OpenOptions::new()
            .read(true)
            .open(&file_path)
            .map_err(|e| format!("Failed to open for mmap: {}", e))?;
        
        if let Ok(mmap) = unsafe { memmap2::Mmap::map(&file) } {
            self.mmap_files.write().unwrap().insert(key.clone(), mmap);
        }
        
        // Create entry
        let entry = CacheEntry {
            package: package.to_string(),
            version: version.to_string(),
            hash: hash.clone(),
            size: data.len() as u64,
            essential_size: data.len() as u64,  // Will be updated by selective extract
            savings_percent: 0.0,
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            accessed_count: 0,
        };
        
        // Update index
        self.index.write().unwrap().insert(key, entry.clone());
        
        // Save index
        self.save_index()?;
        
        Ok(entry)
    }
    
    /// Save index to disk
    fn save_index(&self) -> Result<(), String> {
        let index_path = self.cache_dir.join("index.json");
        let content = serde_json::to_string_pretty(&*self.index.read().unwrap())
            .map_err(|e| format!("Failed to serialize index: {}", e))?;
        
        fs::write(&index_path, content)
            .map_err(|e| format!("Failed to write index: {}", e))?;
        
        Ok(())
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let index = self.index.read().unwrap();
        
        let total_size: u64 = index.values().map(|e| e.size).sum();
        let total_packages = index.len();
        let total_accesses: u64 = index.values().map(|e| e.accessed_count).sum();
        
        CacheStats {
            total_packages,
            total_size,
            total_accesses,
            avg_accesses: if total_packages > 0 {
                total_accesses as f64 / total_packages as f64
            } else {
                0.0
            },
        }
    }
    
    /// Clear cache
    pub fn clear(&self) -> Result<(), String> {
        // Clear memory maps
        self.mmap_files.write().unwrap().clear();
        
        // Remove cache files
        for entry in self.index.read().unwrap().values() {
            let file_path = self.cache_dir.join(format!("{}.pkg", entry.hash));
            let _ = fs::remove_file(&file_path);
        }
        
        // Clear index
        self.index.write().unwrap().clear();
        
        // Save empty index
        self.save_index()?;
        
        Ok(())
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_packages: usize,
    pub total_size: u64,
    pub total_accesses: u64,
    pub avg_accesses: f64,
}

impl Default for GlobalCache {
    fn default() -> Self {
        Self::new().expect("Failed to create global cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_cache() {
        let cache = GlobalCache::new().unwrap();
        
        let data = b"test package data";
        cache.add("test-pkg", "1.0.0", data).unwrap();
        
        assert!(cache.has("test-pkg", "1.0.0"));
        
        let retrieved = cache.get("test-pkg", "1.0.0").unwrap();
        assert_eq!(retrieved, data);
        
        let stats = cache.stats();
        assert_eq!(stats.total_packages, 1);
    }
}
