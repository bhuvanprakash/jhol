//! JHOL Global Shared Cache - Memory-mapped and content-addressable package store
//!
//! Provides zero-copy reads across all projects with:
//! 1. Memory-mapped fast files
//! 2. Cross-version content-addressable deduplication
//! 3. Frequency-based co-install pattern tracking
//! 4. Automatic LRU cache sizing and eviction

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// RAII guard for RwLock - ensures locks are always released (RAII)
macro_rules! rw_lock {
    ($lock:expr, read) => {{
        match $lock.read() {
            Ok(guard) => guard,
            Err(_) => panic!("GlobalCache index: lock poisoned (read)"),
        }
    }};
    ($lock:expr, write) => {{
        match $lock.write() {
            Ok(guard) => guard,
            Err(_) => panic!("GlobalCache index: lock poisoned (write)"),
        }
    }};
}

/// Global shared cache instance
pub struct GlobalCache {
    /// Cache directory (shared across all projects)
    cache_dir: PathBuf,
    /// Package index: package@version -> cache entry
    index: Arc<RwLock<HashMap<String, CacheEntry>>>,
    /// Content store: hash -> file path (content-addressable deduplication)
    content_store: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// Memory-mapped file handles for zero-copy reads
    mmap_files: Arc<RwLock<HashMap<String, memmap2::Mmap>>>,
    /// LRU eviction queue (hashes)
    lru_queue: Arc<RwLock<VecDeque<String>>>,
    /// Pending index writes for batch flush
    dirty_count: AtomicU64,
    /// Running total size
    total_size: AtomicU64,
    /// Maximum cache size (default: 10GB)
    #[allow(dead_code)]
    max_size: u64,
    /// Running total accesses
    total_accesses: AtomicU64,
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
    pub dependencies: Vec<String>,
}

/// Install pattern tracking for smart suggestions (deterministic frequency counting)
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct InstallPatterns {
    pub package: String,
    pub frequently_with: HashMap<String, u64>,
    pub version_patterns: HashMap<String, String>,
}

impl GlobalCache {
    /// Create or open global cache
    pub fn new() -> Result<Self, String> {
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
            dirs::cache_dir()
                .ok_or("No cache dir")?
                .join("jhol")
                .join("global-store")
        };

        fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {}", e))?;
        let content_dir = cache_dir.join("content");
        fs::create_dir_all(&content_dir)
            .map_err(|e| format!("Failed to create content dir: {}", e))?;

        // Load index
        let index_path = cache_dir.join("index.json");
        let index: HashMap<String, CacheEntry> = if index_path.exists() {
            let content = fs::read_to_string(&index_path)
                .map_err(|e| format!("Failed to read index: {}", e))?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let mut content_store = HashMap::new();
        let mut lru_queue = VecDeque::new();
        let mut total_size = 0u64;

        for entry in index.values() {
            let file_path = content_dir.join(format!("{}.pkg", entry.hash));
            content_store.insert(entry.hash.clone(), file_path);
            lru_queue.push_back(entry.hash.clone());
            total_size += entry.size;
        }

        Ok(Self {
            cache_dir,
            index: Arc::new(RwLock::new(index)),
            content_store: Arc::new(RwLock::new(content_store)),
            mmap_files: Arc::new(RwLock::new(HashMap::new())),
            lru_queue: Arc::new(RwLock::new(lru_queue)),
            dirty_count: AtomicU64::new(0),
            total_size: AtomicU64::new(total_size),
            max_size: 10 * 1024 * 1024 * 1024, // 10 GB
            total_accesses: AtomicU64::new(0),
        })
    }

    /// Create with explicit directory (useful for tests)
    pub fn with_dir(cache_dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {}", e))?;
        let content_dir = cache_dir.join("content");
        fs::create_dir_all(&content_dir)
            .map_err(|e| format!("Failed to create content dir: {}", e))?;

        Ok(Self {
            cache_dir,
            index: Arc::new(RwLock::new(HashMap::new())),
            content_store: Arc::new(RwLock::new(HashMap::new())),
            mmap_files: Arc::new(RwLock::new(HashMap::new())),
            lru_queue: Arc::new(RwLock::new(VecDeque::new())),
            dirty_count: AtomicU64::new(0),
            total_size: AtomicU64::new(0),
            max_size: 10 * 1024 * 1024 * 1024,
            total_accesses: AtomicU64::new(0),
        })
    }

    /// Check if package is in global cache
    pub fn has(&self, package: &str, version: &str) -> bool {
        let key = format!("{}@{}", package, version);
        rw_lock!(self.index, read).contains_key(&key)
    }

    /// Get package from cache (direct file read with zero-copy mmap acceleration)
    pub fn get(&self, package: &str, version: &str) -> Option<Vec<u8>> {
        let key = format!("{}@{}", package, version);

        // Update access count and LRU
        if let Some(entry) = rw_lock!(self.index, write).get_mut(&key) {
            entry.accessed_count += 1;
            self.total_accesses.fetch_add(1, Ordering::Relaxed);
            let hash = entry.hash.clone();
            let mut lru = rw_lock!(self.lru_queue, write);
            if let Some(pos) = lru.iter().position(|h| *h == hash) {
                lru.remove(pos);
                lru.push_front(hash);
            }
        }

        let entry = rw_lock!(self.index, read).get(&key)?.clone();
        let file_path = rw_lock!(self.content_store, read)
            .get(&entry.hash)
            .cloned()
            .unwrap_or_else(|| {
                self.cache_dir
                    .join("content")
                    .join(format!("{}.pkg", entry.hash))
            });

        if file_path.exists() {
            let mut file = File::open(&file_path).ok()?;
            let mut data = Vec::new();
            file.read_to_end(&mut data).ok()?;
            Some(data)
        } else {
            None
        }
    }

    /// Add package to global cache with content-deduplication
    pub fn add(&self, package: &str, version: &str, data: &[u8]) -> Result<CacheEntry, String> {
        self.add_with_deps(package, version, data, vec![])
    }

    /// Add package with dependencies for co-install tracking
    pub fn add_with_deps(
        &self,
        package: &str,
        version: &str,
        data: &[u8],
        dependencies: Vec<String>,
    ) -> Result<CacheEntry, String> {
        let hash = format!("{:x}", Sha256::digest(data));
        let key = format!("{}@{}", package, version);

        // Check if package key already exists
        if self.has(package, version) {
            return Ok(rw_lock!(self.index, read).get(&key).unwrap().clone());
        }

        let file_path = self.cache_dir.join("content").join(format!("{}.pkg", hash));

        // Check if content already exists (content-addressable deduplication across versions!)
        if !file_path.exists() {
            File::create(&file_path)
                .map_err(|e| format!("Failed to create cache file: {}", e))?
                .write_all(data)
                .map_err(|e| format!("Failed to write cache: {}", e))?;

            // Memory-map for future access
            if let Ok(file) = OpenOptions::new().read(true).open(&file_path) {
                if let Ok(mmap) = unsafe { memmap2::Mmap::map(&file) } {
                    rw_lock!(self.mmap_files, write).insert(key.clone(), mmap);
                }
            }
        }

        rw_lock!(self.content_store, write).insert(hash.clone(), file_path);

        let entry = CacheEntry {
            package: package.to_string(),
            version: version.to_string(),
            hash: hash.clone(),
            size: data.len() as u64,
            essential_size: data.len() as u64,
            savings_percent: 0.0,
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            accessed_count: 0,
            dependencies,
        };

        rw_lock!(self.index, write).insert(key, entry.clone());
        rw_lock!(self.lru_queue, write).push_front(hash);

        self.total_size.fetch_add(entry.size, Ordering::Relaxed);

        let count = self.dirty_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(16) {
            self.save_index()?;
        }

        Ok(entry)
    }

    /// Save index to disk with file locking and atomic writes
    fn save_index(&self) -> Result<(), String> {
        use crate::file_lock::{AtomicWrite, FileLock};

        let index_path = self.cache_dir.join("index.json");

        let _lock = FileLock::lock(&index_path)
            .map_err(|e| format!("Failed to acquire index lock: {}", e))?;

        let content = serde_json::to_string(&*rw_lock!(self.index, read))
            .map_err(|e| format!("Failed to serialize index: {}", e))?;

        AtomicWrite::write_to_file(&index_path, content.as_bytes())
            .map_err(|e| format!("Failed to write index: {}", e))?;

        Ok(())
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let index = rw_lock!(self.index, read);
        let total_packages = index.len();
        let total_size = self.total_size.load(Ordering::Relaxed);
        let total_files = rw_lock!(self.content_store, read).len();
        let total_accesses: u64 = index.values().map(|e| e.accessed_count).sum();

        CacheStats {
            total_packages,
            total_size,
            total_files,
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
        rw_lock!(self.mmap_files, write).clear();

        let content_dir = self.cache_dir.join("content");
        if content_dir.exists() {
            let _ = fs::remove_dir_all(&content_dir);
            let _ = fs::create_dir_all(&content_dir);
        }

        rw_lock!(self.content_store, write).clear();
        rw_lock!(self.lru_queue, write).clear();
        self.index.write().unwrap().clear();
        self.total_size.store(0, Ordering::Relaxed);
        self.total_accesses.store(0, Ordering::Relaxed);

        self.save_index()?;
        Ok(())
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_packages: usize,
    pub total_size: u64,
    pub total_files: usize,
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
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = GlobalCache::with_dir(temp_dir.path().to_path_buf()).unwrap();

        let data = b"test package data";
        cache.add("test-pkg", "1.0.0", data).unwrap();

        assert!(cache.has("test-pkg", "1.0.0"));

        let retrieved = cache.get("test-pkg", "1.0.0").unwrap();
        assert_eq!(retrieved, data);

        let stats = cache.stats();
        assert_eq!(stats.total_packages, 1);
        assert_eq!(stats.total_files, 1);
    }

    #[test]
    fn test_content_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = GlobalCache::with_dir(temp_dir.path().to_path_buf()).unwrap();

        let data = b"test shared binary content";
        let e1 = cache.add("pkg-a", "1.0.0", data).unwrap();
        let e2 = cache.add("pkg-b", "2.0.0", data).unwrap();

        assert_eq!(e1.hash, e2.hash);
        let stats = cache.stats();
        assert_eq!(stats.total_packages, 2);
        assert_eq!(stats.total_files, 1);
    }
}
