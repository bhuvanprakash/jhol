//! Content-Addressable Store implementation
//! 
//! Stores packages by content hash (SHA256) for deduplication.
//! Supports hard links, reflinks, and regular copies.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::io::{Read, Write};
use std::fs::{self, File};

use dashmap::DashMap;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as TokioMutex;

use crate::utils;

/// Configuration for Content-Addressable Store
#[derive(Clone, Debug)]
pub struct CASConfig {
    /// Maximum cache size in bytes (0 = unlimited)
    pub max_size: u64,
    /// Maximum age for cache entries (0 = unlimited)
    pub max_age: Duration,
    /// Enable integrity verification
    pub verify_integrity: bool,
    /// Prefer hard links over copies
    pub prefer_hardlinks: bool,
    /// Enable reflink (copy-on-write) on supported filesystems
    pub enable_reflink: bool,
}

impl Default for CASConfig {
    fn default() -> Self {
        Self {
            max_size: 10 * 1024 * 1024 * 1024, // 10 GB default
            max_age: Duration::from_secs(30 * 24 * 60 * 60), // 30 days
            verify_integrity: true,
            prefer_hardlinks: true,
            enable_reflink: true,
        }
    }
}

/// Entry in the content-addressable store
#[derive(Clone, Debug)]
pub struct StoreEntry {
    /// SHA256 hash of content
    pub hash: String,
    /// Size in bytes
    pub size: u64,
    /// Integrity hash (SRI format)
    pub integrity: String,
    /// Path to stored file
    pub store_path: PathBuf,
    /// Last access time
    pub last_accessed: Instant,
    /// Access count (for LRU)
    pub access_count: u64,
    /// Package references (package@version -> count)
    pub references: HashMap<String, u64>,
}

/// Content-Addressable Store
/// 
/// Stores packages by content hash for deduplication.
/// Directory structure: `~/.jhol-store/v1/files/XX/XXXX/full_hash`
pub struct ContentAddressableStore {
    /// Base store path
    store_path: PathBuf,
    /// Index: package@version -> hash
    index: Arc<DashMap<String, String>>,
    /// Hash -> store entry
    entries: Arc<DashMap<String, StoreEntry>>,
    /// Configuration
    config: CASConfig,
    /// Write lock to prevent concurrent writes to same hash
    write_locks: Arc<DashMap<String, Arc<TokioMutex<()>>>>,
    /// Total size tracking
    total_size: Arc<TokioMutex<u64>>,
}

impl ContentAddressableStore {
    /// Create a new CAS instance
    pub fn new(store_path: PathBuf) -> Self {
        Self::with_config(store_path, CASConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(store_path: PathBuf, config: CASConfig) -> Self {
        // Ensure store directories exist
        let files_dir = store_path.join("files");
        let _ = fs::create_dir_all(&files_dir);
        
        // Create subdirectories for hash sharding (00-ff)
        for i in 0..256 {
            let subdir = format!("{:02x}", i);
            let _ = fs::create_dir_all(files_dir.join(&subdir));
        }
        
        Self {
            store_path,
            index: Arc::new(DashMap::new()),
            entries: Arc::new(DashMap::new()),
            config,
            write_locks: Arc::new(DashMap::new()),
            total_size: Arc::new(TokioMutex::new(0)),
        }
    }

    /// Get the store path for a hash
    fn hash_to_path(&self, hash: &str) -> PathBuf {
        if hash.len() < 4 {
            return self.store_path.join("files").join("invalid");
        }
        
        let subdir1 = &hash[0..2];
        let subdir2 = &hash[2..4];
        self.store_path
            .join("files")
            .join(subdir1)
            .join(subdir2)
            .join(hash)
    }

    /// Store content and return its hash
    pub async fn store(&self, content: &[u8]) -> Result<String, String> {
        // Compute hash
        let hash = compute_hash(content);
        
        // Get write lock for this hash
        let lock = self.write_locks
            .entry(hash.clone())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone();
        
        let _guard = lock.lock().await;
        
        // Check if already exists
        if let Some(entry) = self.entries.get(&hash) {
            // Update access time
            let mut entry = entry.value().clone();
            entry.last_accessed = Instant::now();
            entry.access_count += 1;
            self.entries.insert(hash.clone(), entry);
            return Ok(hash);
        }
        
        // Store the content
        let store_path = self.hash_to_path(&hash);
        
        // Atomic write: write to temp file, then rename
        let temp_path = store_path.with_extension("tmp");
        {
            let mut file = File::create(&temp_path)
                .map_err(|e| format!("Failed to create temp file: {}", e))?;
            file.write_all(content)
                .map_err(|e| format!("Failed to write content: {}", e))?;
            file.sync_all()
                .map_err(|e| format!("Failed to sync file: {}", e))?;
        }
        
        // Rename to final location
        fs::rename(&temp_path, &store_path)
            .map_err(|e| format!("Failed to finalize store: {}", e))?;
        
        // Compute integrity hash
        let integrity = format!("sha256-{}", base64::encode(Sha256::digest(content)));
        
        // Create entry
        let entry = StoreEntry {
            hash: hash.clone(),
            size: content.len() as u64,
            integrity,
            store_path: store_path.clone(),
            last_accessed: Instant::now(),
            access_count: 1,
            references: HashMap::new(),
        };
        
        // Update tracking
        self.entries.insert(hash.clone(), entry);
        
        // Update total size
        {
            let mut total = self.total_size.lock().await;
            *total += content.len() as u64;
        }
        
        // Check size limits and evict if needed
        self.evict_if_needed().await;
        
        Ok(hash)
    }

    /// Store a package with metadata
    pub async fn store_package(
        &self,
        package: &str,
        version: &str,
        content: &[u8],
        expected_integrity: Option<&str>,
    ) -> Result<String, String> {
        // Verify integrity if provided
        if let Some(expected) = expected_integrity {
            if !verify_integrity(content, expected) {
                return Err(format!("Integrity verification failed for {}@{}", package, version));
            }
        }
        
        // Store content
        let hash = self.store(content).await?;
        
        // Update index
        let key = format!("{}@{}", package, version);
        self.index.insert(key.clone(), hash.clone());
        
        // Update entry references
        if let Some(mut entry) = self.entries.get_mut(&hash) {
            *entry.references.entry(key).or_insert(0) += 1;
        }
        
        Ok(hash)
    }

    /// Get content by hash
    pub async fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
        let entry = self.entries.get(hash)
            .ok_or_else(|| format!("Hash {} not found in store", hash))?;
        
        // Update access time
        let mut entry_value = entry.value().clone();
        entry_value.last_accessed = Instant::now();
        entry_value.access_count += 1;
        self.entries.insert(hash.to_string(), entry_value);
        
        // Read content
        let content = tokio::fs::read(&entry.store_path)
            .await
            .map_err(|e| format!("Failed to read store entry: {}", e))?;
        
        Ok(content)
    }

    /// Get content by package@version
    pub async fn get_package(&self, package: &str, version: &str) -> Result<Vec<u8>, String> {
        let key = format!("{}@{}", package, version);
        let hash = self.index.get(&key)
            .ok_or_else(|| format!("Package {}@{} not found in store", package, version))?
            .clone();
        
        self.get(&hash).await
    }

    /// Check if content exists in store
    pub fn has(&self, hash: &str) -> bool {
        self.entries.contains_key(hash)
    }

    /// Check if package exists in store
    pub fn has_package(&self, package: &str, version: &str) -> bool {
        let key = format!("{}@{}", package, version);
        self.index.contains_key(&key)
    }

    /// Link package to destination (hard link, reflink, or copy)
    pub async fn link_to(
        &self,
        hash: &str,
        dest: &Path,
        link_type: Option<LinkType>,
    ) -> Result<(), String> {
        let entry = self.entries.get(hash)
            .ok_or_else(|| format!("Hash {} not found in store", hash))?;
        
        let store_path = entry.store_path.clone();
        drop(entry); // Release lock
        
        // Determine link type
        let link_type = link_type.unwrap_or_else(|| self.default_link_type());
        
        // Attempt to link
        match link_type {
            LinkType::HardLink => {
                if self.try_hardlink(&store_path, dest).await {
                    return Ok(());
                }
                // Fall through to next type if hardlink fails
            }
            LinkType::Reflink => {
                if self.try_reflink(&store_path, dest).await {
                    return Ok(());
                }
                // Fall through to next type if reflink fails
            }
            LinkType::Copy => {
                return self.copy_file(&store_path, dest).await;
            }
        }
        
        // Try alternative link types
        if self.config.enable_reflink && self.try_reflink(&store_path, dest).await {
            return Ok(());
        }
        
        if self.config.prefer_hardlinks && self.try_hardlink(&store_path, dest).await {
            return Ok(());
        }
        
        // Final fallback: copy
        self.copy_file(&store_path, dest).await
    }

    /// Try to create a hard link
    async fn try_hardlink(&self, src: &Path, dest: &Path) -> bool {
        // Ensure destination directory exists
        if let Some(parent) = dest.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        
        // Remove existing destination
        let _ = tokio::fs::remove_file(dest).await;
        
        // Try hard link
        match tokio::fs::hard_link(src, dest).await {
            Ok(()) => true,
            Err(_) => false,
        }
    }

    /// Try to create a reflink (copy-on-write)
    async fn try_reflink(&self, src: &Path, dest: &Path) -> bool {
        // Ensure destination directory exists
        if let Some(parent) = dest.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        
        // Remove existing destination
        let _ = tokio::fs::remove_file(dest).await;
        
        // Try reflink (platform-specific)
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            match reflink_copy::reflink(src, dest) {
                Ok(()) => return true,
                Err(_) => {}
            }
        }
        
        // Try reflink_or_copy (copy if reflink not supported)
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            if reflink_copy::reflink_or_copy(src, dest).is_ok() {
                return true;
            }
        }

        false
    }

    /// Copy file
    async fn copy_file(&self, src: &Path, dest: &Path) -> Result<(), String> {
        // Ensure destination directory exists
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create destination directory: {}", e))?;
        }
        
        // Copy file
        tokio::fs::copy(src, dest)
            .await
            .map_err(|e| format!("Failed to copy file: {}", e))?;
        
        Ok(())
    }

    /// Get default link type based on config
    fn default_link_type(&self) -> LinkType {
        if self.config.enable_reflink {
            LinkType::Reflink
        } else if self.config.prefer_hardlinks {
            LinkType::HardLink
        } else {
            LinkType::Copy
        }
    }

    /// Evict entries if cache is too large
    async fn evict_if_needed(&self) {
        let total_size = *self.total_size.lock().await;
        
        if total_size <= self.config.max_size || self.config.max_size == 0 {
            return;
        }
        
        // Find oldest entries (LRU eviction)
        let mut entries_to_evict: Vec<(String, Instant)> = self.entries
            .iter()
            .map(|e| (e.key().clone(), e.value().last_accessed))
            .collect();
        
        // Sort by last access time (oldest first)
        entries_to_evict.sort_by(|a, b| a.1.cmp(&b.1));
        
        // Evict until under limit
        let mut evicted_size = 0u64;
        let target_size = self.config.max_size / 2; // Evict to 50% of max
        
        for (hash, _) in entries_to_evict {
            if total_size - evicted_size <= target_size {
                break;
            }
            
            if let Some(entry) = self.entries.remove(&hash) {
                evicted_size += entry.1.size;
                
                // Delete file
                let _ = tokio::fs::remove_file(&entry.1.store_path).await;
                
                // Remove from index
                for key in entry.1.references.keys() {
                    self.index.remove(key);
                }
            }
        }
        
        // Update total size
        {
            let mut total = self.total_size.lock().await;
            *total = total.saturating_sub(evicted_size);
        }
    }

    /// Get store statistics
    pub async fn stats(&self) -> CASStats {
        let mut total_size = 0u64;
        let mut entry_count = 0u64;
        let mut total_references = 0u64;
        
        for entry in self.entries.iter() {
            total_size += entry.value().size;
            entry_count += 1;
            total_references += entry.value().references.len() as u64;
        }
        
        CASStats {
            entry_count,
            total_size,
            total_references,
            index_size: self.index.len() as u64,
        }
    }

    /// Clear all entries
    pub async fn clear(&self) -> Result<(), String> {
        // Delete all files
        let files_dir = self.store_path.join("files");
        if files_dir.exists() {
            tokio::fs::remove_dir_all(&files_dir)
                .await
                .map_err(|e| format!("Failed to clear store: {}", e))?;
            
            // Recreate directories
            let _ = fs::create_dir_all(&files_dir);
            for i in 0..256 {
                let subdir = format!("{:02x}", i);
                let _ = fs::create_dir_all(files_dir.join(&subdir));
            }
        }
        
        // Clear maps
        self.index.clear();
        self.entries.clear();
        
        // Reset size
        {
            let mut total = self.total_size.lock().await;
            *total = 0;
        }
        
        Ok(())
    }

    /// Prune old entries
    pub async fn prune(&self) -> Result<PruneResult, String> {
        let now = Instant::now();
        let mut pruned_count = 0u64;
        let mut pruned_size = 0u64;
        
        let mut to_remove: Vec<String> = Vec::new();
        
        for entry in self.entries.iter() {
            if now.duration_since(entry.value().last_accessed) > self.config.max_age {
                to_remove.push(entry.key().clone());
            }
        }
        
        for hash in to_remove {
            if let Some(entry) = self.entries.remove(&hash) {
                pruned_size += entry.1.size;
                pruned_count += 1;
                
                // Delete file
                let _ = tokio::fs::remove_file(&entry.1.store_path).await;
                
                // Remove from index
                for key in entry.1.references.keys() {
                    self.index.remove(key);
                }
            }
        }
        
        // Update total size
        {
            let mut total = self.total_size.lock().await;
            *total = total.saturating_sub(pruned_size);
        }
        
        Ok(PruneResult {
            pruned_count,
            pruned_size,
        })
    }
}

/// Statistics for the content-addressable store
#[derive(Clone, Debug, Default)]
pub struct CASStats {
    /// Number of unique content entries
    pub entry_count: u64,
    /// Total size in bytes
    pub total_size: u64,
    /// Total package references
    pub total_references: u64,
    /// Number of package@version -> hash mappings
    pub index_size: u64,
}

/// Result of pruning operation
#[derive(Clone, Debug, Default)]
pub struct PruneResult {
    /// Number of entries pruned
    pub pruned_count: u64,
    /// Bytes freed
    pub pruned_size: u64,
}

/// Link type for package installation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkType {
    /// Hard link (fastest, no extra disk space)
    HardLink,
    /// Reflink/copy-on-write (fast, minimal disk space)
    Reflink,
    /// Full copy (slowest, uses most disk space)
    Copy,
}

/// Compute SHA256 hash of content
fn compute_hash(content: &[u8]) -> String {
    let hash = Sha256::digest(content);
    format!("{:x}", hash)
}

/// Verify content integrity
fn verify_integrity(content: &[u8], expected_integrity: &str) -> bool {
    let hash = Sha256::digest(content);
    let computed = format!("sha256-{}", base64::encode(hash));
    computed == expected_integrity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_get() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas = ContentAddressableStore::new(temp_dir.path().to_path_buf());
        
        let content = b"test content";
        let hash = cas.store(content).await.unwrap();
        
        assert_eq!(hash.len(), 64); // SHA256 hex length
        
        let retrieved = cas.get(&hash).await.unwrap();
        assert_eq!(retrieved, content);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas = ContentAddressableStore::new(temp_dir.path().to_path_buf());
        
        let content = b"test content";
        let hash1 = cas.store(content).await.unwrap();
        let hash2 = cas.store(content).await.unwrap();
        
        // Same content should produce same hash
        assert_eq!(hash1, hash2);
        
        // Should only have one entry
        let stats = cas.stats().await;
        assert_eq!(stats.entry_count, 1);
    }

    #[tokio::test]
    async fn test_store_package() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cas = ContentAddressableStore::new(temp_dir.path().to_path_buf());
        
        let content = b"package content";
        let hash = cas.store_package("test-pkg", "1.0.0", content, None).await.unwrap();
        
        assert!(cas.has_package("test-pkg", "1.0.0"));
        
        let retrieved = cas.get_package("test-pkg", "1.0.0").await.unwrap();
        assert_eq!(retrieved, content);
    }
}
