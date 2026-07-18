//! GRM Store - Resolution Memory Storage

use crate::grm::{GRMStats, ResolutionRecord};
use crate::mpt_store::ContentHash;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Platform constraints
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformConstraints {
    pub os: Option<String>,
    pub arch: Option<String>,
    pub libc: Option<String>,
}

impl Default for PlatformConstraints {
    fn default() -> Self {
        Self {
            os: Some(std::env::consts::OS.to_string()),
            arch: Some(std::env::consts::ARCH.to_string()),
            libc: None,
        }
    }
}

/// GRM Store
pub struct GRMStore {
    records: Arc<DashMap<String, Vec<ResolutionRecord>>>,
    next_id: RwLock<u64>,
    object_path: PathBuf,
    hit_counts: Arc<DashMap<u64, u64>>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    invalidations: Arc<AtomicU64>,
}

impl GRMStore {
    // =========================================================================
    // Creation
    // =========================================================================

    /// Create new store at path
    pub fn new(base_path: impl AsRef<Path>) -> Result<Self, GRMStoreError> {
        let object_path = base_path.as_ref().to_path_buf();
        fs::create_dir_all(&object_path).map_err(|e| GRMStoreError::IoError(e.to_string()))?;

        let mut store = Self {
            records: Arc::new(DashMap::new()),
            next_id: RwLock::new(1),
            object_path,
            hit_counts: Arc::new(DashMap::new()),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            invalidations: Arc::new(AtomicU64::new(0)),
        };

        // Log but don't fail - we can operate with empty state
        if let Err(e) = store.load() {
            crate::safe_file::log_warning(&format!(
                "GRM load failed: {}. Starting with empty state.",
                e
            ));
        }

        Ok(store)
    }

    /// Create in-memory store
    #[allow(dead_code)]
    pub fn memory() -> Self {
        Self {
            records: Arc::new(DashMap::new()),
            next_id: RwLock::new(1),
            object_path: PathBuf::new(),
            hit_counts: Arc::new(DashMap::new()),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            invalidations: Arc::new(AtomicU64::new(0)),
        }
    }

    // =========================================================================
    // Core Operations
    // =========================================================================

    /// Store a new resolution
    pub fn store(&self, record: ResolutionRecord) -> Result<u64, GRMStoreError> {
        let mut next_id = self.next_id.write();
        let id = *next_id;
        *next_id += 1;

        let resolved_version = record.resolved_version.clone();
        let mut record_with_id = record;
        record_with_id.id = id;

        // Compute resolution hash if not set
        if record_with_id.resolution_hash.is_empty() {
            record_with_id.resolution_hash = compute_resolution_hash(&record_with_id);
        }

        // Store in memory
        let key = self.make_key(
            &record_with_id.package_spec,
            record_with_id.platform.as_ref().map(|s| s.as_str()),
            record_with_id.node_version.as_ref().map(|s| s.as_str()),
        );
        let mut entries = self.records.entry(key).or_insert_with(Vec::new);

        // Replace existing entry for same version, or append
        if let Some(existing) = entries
            .iter_mut()
            .find(|r| r.resolved_version == resolved_version)
        {
            *existing = record_with_id.clone();
        } else {
            entries.push(record_with_id.clone());
        }

        // Save - log error but don't fail since in-memory is updated
        if let Err(e) = self.save() {
            crate::safe_file::log_error(&format!("GRM save failed: {}. Data may not persist.", e));
        }

        Ok(id)
    }

    /// Query for cached resolution
    pub fn query(
        &self,
        package_spec: &str,
        platform: Option<&str>,
        node_version: Option<&str>,
    ) -> Option<ResolutionRecord> {
        let key = self.make_key(package_spec, platform, node_version);

        let Some(entries) = self.records.get(&key) else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };

        // Return most recent with highest hit count
        let found = entries
            .iter()
            .max_by_key(|r| (r.cached_at, r.hit_count))
            .cloned();
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// Query for cached resolution and conservatively invalidate stale entries.
    ///
    /// If a record is older than `max_age`, `latest_resolver` is asked for the
    /// latest valid registry version for the original package spec. When the
    /// registry lookup succeeds and differs from the cached version, the record
    /// is invalidated. Lookup failures keep the record (offline-safe false
    /// negatives are preferred over false positives).
    pub fn query_validated<F>(
        &self,
        package_spec: &str,
        platform: Option<&str>,
        node_version: Option<&str>,
        max_age: Duration,
        latest_resolver: F,
    ) -> Option<ResolutionRecord>
    where
        F: FnOnce(&str) -> Result<Option<String>, String>,
    {
        let record = self.query(package_spec, platform, node_version)?;
        if !self.is_older_than(&record, max_age) {
            return Some(record);
        }

        match latest_resolver(package_spec) {
            Ok(Some(latest)) if latest != record.resolved_version => {
                if self.invalidate_id(record.id).unwrap_or(false) {
                    crate::safe_file::log_warning(&format!(
                        "GRM invalidated stale {}: cached {}, latest {}",
                        package_spec, record.resolved_version, latest
                    ));
                }
                None
            }
            _ => Some(record),
        }
    }

    pub fn get_by_id(&self, id: u64) -> Option<ResolutionRecord> {
        self.records
            .iter()
            .flat_map(|entry| entry.value().clone())
            .find(|record| record.id == id)
    }

    pub fn invalidate_id(&self, id: u64) -> Result<bool, GRMStoreError> {
        let mut removed = false;
        let keys: Vec<String> = self.records.iter().map(|r| r.key().clone()).collect();
        for key in keys {
            if let Some(mut records) = self.records.get_mut(&key) {
                let before = records.len();
                records.retain(|r| r.id != id);
                removed |= records.len() != before;
            }
        }
        if removed {
            self.invalidations.fetch_add(1, Ordering::Relaxed);
            self.save()?;
        }
        Ok(removed)
    }

    /// Query with full query object
    pub fn query_full(&self, query: &crate::grm::GRMQuery) -> Vec<ResolutionRecord> {
        let key = self.make_key(
            &query.package_spec,
            query.platform.as_deref(),
            query.node_version.as_deref(),
        );

        let entries = match self.records.get(&key) {
            Some(e) => e,
            None => return Vec::new(),
        };

        entries
            .iter()
            .filter(|r| r.matches_query(query))
            .cloned()
            .collect()
    }

    /// Increment hit count for a record
    #[allow(dead_code)]
    pub fn record_hit(&self, id: u64) {
        *self.hit_counts.entry(id).or_insert(0) += 1;
    }

    /// Invalidate stale entries
    #[allow(dead_code)]
    pub fn prune(&self, max_age: Duration) -> Result<u64, GRMStoreError> {
        let cutoff = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
            - max_age.as_millis() as u64;

        let mut removed = 0u64;

        // Collect entries to modify, then modify
        let keys_to_check: Vec<String> = self.records.iter().map(|r| r.key().clone()).collect();

        for key in keys_to_check {
            if let Some(mut records) = self.records.get_mut(&key) {
                let before = records.len();
                records.retain(|r| r.cached_at >= cutoff);
                removed += (before - records.len()) as u64;
            }
        }

        if removed > 0 {
            if let Err(e) = self.save() {
                crate::safe_file::log_error(&format!(
                    "GRM save after prune failed: {}. Data may not persist.",
                    e
                ));
            }
        }

        Ok(removed)
    }

    /// Get statistics
    #[allow(dead_code)]
    #[must_use]
    pub fn stats(&self) -> GRMStats {
        let mut total_records = 0u64;
        let mut total_hits = 0u64;
        let mut low_priority_records = 0u64;

        for entry in self.records.iter() {
            total_records += entry.len() as u64;
            total_hits += entry.iter().map(|r| r.hit_count).sum::<u64>();
            low_priority_records += entry.iter().filter(|r| r.cache_priority < 50).count() as u64;
        }

        GRMStats {
            total_records,
            total_hits,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
            low_priority_records,
            unique_packages: self.records.len() as u64,
            next_id: *self.next_id.read(),
        }
    }

    /// Get all records for a package
    #[allow(dead_code)]
    pub fn get_all_versions(&self, package_name: &str) -> Vec<ResolutionRecord> {
        // Try exact match first
        let key = self.make_key(package_name, None, None);
        if let Some(records) = self.records.get(&key) {
            return records.clone();
        }

        // Try partial match
        let mut results = Vec::new();
        for entry in self.records.iter() {
            if entry.key().starts_with(package_name) {
                results.extend(entry.value().clone());
            }
        }
        results
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    fn make_key(
        &self,
        package_spec: &str,
        platform: Option<&str>,
        node_version: Option<&str>,
    ) -> String {
        let base = platform
            .map(|p| format!("{}:{}", package_spec, p))
            .unwrap_or_else(|| package_spec.to_string());

        if let Some(nv) = node_version {
            format!("{}@{}", base, nv)
        } else {
            base
        }
    }

    fn is_older_than(&self, record: &ResolutionRecord, max_age: Duration) -> bool {
        let now = current_timestamp();
        now.saturating_sub(record.cached_at) > max_age.as_millis() as u64
    }

    // =========================================================================
    // Persistence
    // =========================================================================

    fn data_path(&self) -> PathBuf {
        self.object_path.join("grm.json")
    }

    fn save(&self) -> Result<(), GRMStoreError> {
        if self.object_path.as_os_str().is_empty() {
            return Ok(());
        }

        let data_path = self.data_path();
        if let Some(parent) = data_path.parent() {
            if let Err(e) = crate::safe_file::ensure_dir(parent) {
                return Err(GRMStoreError::IoError(e.to_string()));
            }
        }

        let data = GRMStoreData {
            records: self
                .records
                .iter()
                .flat_map(|e| e.value().clone())
                .collect(),
            next_id: *self.next_id.read(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
        };

        // Use atomic write to prevent data corruption
        if let Err(e) = crate::safe_file::atomic_write_json(&data_path, &data) {
            return Err(GRMStoreError::Serialization(e.to_string()));
        }

        Ok(())
    }

    fn load(&mut self) -> Result<(), GRMStoreError> {
        if self.object_path.as_os_str().is_empty() {
            return Ok(());
        }

        let data_path = self.data_path();
        if !data_path.exists() {
            return Ok(());
        }

        let bytes = fs::read(&data_path).map_err(|e| GRMStoreError::IoError(e.to_string()))?;
        let data: GRMStoreData = serde_json::from_slice(&bytes)
            .map_err(|e| GRMStoreError::Deserialization(e.to_string()))?;

        *self.next_id.write() = data.next_id;
        self.hits.store(data.hits, Ordering::Relaxed);
        self.misses.store(data.misses, Ordering::Relaxed);
        self.invalidations
            .store(data.invalidations, Ordering::Relaxed);

        for record in data.records {
            let key = self.make_key(
                &record.package_spec,
                record.platform.as_deref(),
                record.node_version.as_deref(),
            );
            let mut entries = self.records.entry(key).or_insert_with(Vec::new);
            entries.push(record);
        }

        Ok(())
    }
}

/// Store data format
#[derive(Serialize, Deserialize)]
struct GRMStoreData {
    records: Vec<ResolutionRecord>,
    next_id: u64,
    #[serde(default)]
    hits: u64,
    #[serde(default)]
    misses: u64,
    #[serde(default)]
    invalidations: u64,
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Compute hash for a resolution record
fn compute_resolution_hash(record: &ResolutionRecord) -> ContentHash {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(record.package_spec.as_bytes());
    hasher.update(record.resolved_version.as_bytes());
    hasher.update(record.content_hash.as_str().as_bytes());
    for dep in &record.dependency_hashes {
        hasher.update(dep.as_str().as_bytes());
    }
    if let Some(ref p) = record.platform {
        hasher.update(p.as_bytes());
    }
    if let Some(ref n) = record.node_version {
        hasher.update(n.as_bytes());
    }

    ContentHash::new(&hasher.finalize())
}

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum GRMStoreError {
    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grm::{ResolutionRecord, ResolutionSource};

    fn record(spec: &str, version: &str) -> ResolutionRecord {
        ResolutionRecord {
            id: 0,
            package_spec: spec.to_string(),
            resolved_version: version.to_string(),
            content_hash: ContentHash::new(format!("{spec}-{version}").as_bytes()),
            dependency_hashes: vec![],
            platform: Some("darwin".to_string()),
            node_version: Some("20".to_string()),
            resolution_hash: ContentHash::default(),
            provenance_sequence: None,
            cached_at: current_timestamp(),
            hit_count: 0,
            source: ResolutionSource::GRM,
            cache_priority: 100,
        }
    }

    #[test]
    fn freshness_check_invalidates_cached_version_when_registry_has_newer_match() {
        let mut store = GRMStore::memory();
        let mut rec = record("left-pad@^1", "1.0.0");
        rec.cached_at = 0; // epoch — guarantees the record is stale
        let id = store.store(rec).unwrap();

        let found = store.query_validated(
            "left-pad@^1",
            Some("darwin"),
            Some("20"),
            Duration::from_secs(7 * 24 * 60 * 60),
            |_| Ok(Some("1.1.0".to_string())),
        );

        assert!(found.is_none());
        assert!(store.get_by_id(id).is_none());
        assert_eq!(store.stats().invalidations, 1);
    }

    #[test]
    fn conservative_freshness_keeps_record_when_latest_lookup_fails() {
        let mut store = GRMStore::memory();
        let mut rec = record("left-pad@^1", "1.0.0");
        rec.cached_at = 0;
        store.store(rec).unwrap();

        let found = store.query_validated(
            "left-pad@^1",
            Some("darwin"),
            Some("20"),
            Duration::from_secs(7 * 24 * 60 * 60),
            |_| Err("offline".to_string()),
        );

        assert_eq!(found.unwrap().resolved_version, "1.0.0");
        assert_eq!(store.stats().invalidations, 0);
    }
}
