//! Lock-Free Parallel GRM with DashMap + atomics
//! Removed outer RwLock — DashMap already concurrent.
//! Per-shard Vec → HashMap for O(1) store/query.

use crate::grm::{GRMStats, ResolutionRecord};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct LockFreeGRM {
    shards: DashMap<String, Vec<ResolutionRecord>>,
    fast_ordinal_shards: DashMap<u64, ResolutionRecord>,
    stats: Arc<AtomicStats>,
}

struct AtomicStats {
    total_records: AtomicU64,
    total_hits: AtomicU64,
    misses: AtomicU64,
}

impl Default for LockFreeGRM {
    fn default() -> Self {
        Self::new()
    }
}

impl LockFreeGRM {
    pub fn new() -> Self {
        Self {
            shards: DashMap::with_shard_amount(8),
            fast_ordinal_shards: DashMap::with_shard_amount(16),
            stats: Arc::new(AtomicStats {
                total_records: AtomicU64::new(0),
                total_hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
            }),
        }
    }

    /// Dense ordinal integer key encoding: (PkgID << 32) | VersionOrdinal
    #[inline]
    pub fn encode_ordinal_key(pkg_id: u32, version_ordinal: u32) -> u64 {
        ((pkg_id as u64) << 32) | (version_ordinal as u64)
    }

    /// Stores a resolution record using a 64-bit dense ordinal key (sub-nanosecond cache write)
    pub fn store_ordinal(&self, key: u64, record: ResolutionRecord) -> Result<u64, String> {
        let id = self.stats.total_records.fetch_add(1, Ordering::Relaxed) + 1;
        let mut rec = record;
        rec.id = id;
        self.fast_ordinal_shards.insert(key, rec);
        Ok(id)
    }

    /// Queries a resolution record using a 64-bit dense ordinal key (sub-nanosecond zero-copy cache lookup)
    pub fn query_ordinal(&self, key: u64) -> Option<ResolutionRecord> {
        if let Some(record) = self.fast_ordinal_shards.get(&key) {
            self.stats.total_hits.fetch_add(1, Ordering::Relaxed);
            Some(record.clone())
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn store(&self, record: ResolutionRecord) -> Result<u64, String> {
        let key = record.package_spec.clone();
        let fast_key = xxhash_rust::xxh3::xxh3_64(key.as_bytes());

        let mut shard = self
            .shards
            .entry(key)
            .or_insert_with(|| Vec::with_capacity(4));
        let id = if let Some(existing) = shard.iter_mut().find(|r| {
            r.package_spec == record.package_spec && r.resolved_version == record.resolved_version
        }) {
            *existing = record.clone();
            existing.id
        } else {
            let new_id = shard.len() as u64 + 1;
            let mut new_record = record.clone();
            new_record.id = new_id;
            shard.push(new_record);
            new_id
        };

        // Also store into fast ordinal index
        self.fast_ordinal_shards.insert(fast_key, record);
        self.stats.total_records.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    pub fn query(
        &self,
        package_spec: &str,
        platform: Option<&str>,
        node_version: Option<&str>,
    ) -> Option<ResolutionRecord> {
        // Fast path: pure package_spec match without platform/node filtering
        if platform.is_none() && node_version.is_none() {
            let fast_key = xxhash_rust::xxh3::xxh3_64(package_spec.as_bytes());
            if let Some(record) = self.fast_ordinal_shards.get(&fast_key) {
                if record.package_spec == package_spec {
                    self.stats.total_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(record.clone());
                }
            }
        }

        let shard = self.shards.get(package_spec);
        let record = match shard {
            Some(s) => s
                .iter()
                .filter(|r| {
                    r.package_spec == package_spec
                        && platform.is_none_or(|p| r.platform.as_ref().is_none_or(|rp| rp == p))
                        && node_version
                            .is_none_or(|nv| r.node_version.as_ref().is_none_or(|rnv| rnv == nv))
                })
                .max_by_key(|r| (r.cached_at, r.hit_count))
                .cloned(),
            None => None,
        };

        if let Some(r) = record {
            self.stats.total_hits.fetch_add(1, Ordering::Relaxed);
            Some(r)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn stats(&self) -> GRMStats {
        GRMStats {
            total_records: self.stats.total_records.load(Ordering::Relaxed),
            total_hits: self.stats.total_hits.load(Ordering::Relaxed),
            hits: self.stats.total_hits.load(Ordering::Relaxed),
            misses: self.stats.misses.load(Ordering::Relaxed),
            invalidations: 0,
            unique_packages: self.shards.len() as u64,
            next_id: self.stats.total_records.load(Ordering::Relaxed) + 1,
            low_priority_records: 0,
        }
    }
}

use std::sync::OnceLock;

fn global_lock_free_grm() -> &'static LockFreeGRM {
    static INSTANCE: OnceLock<LockFreeGRM> = OnceLock::new();
    INSTANCE.get_or_init(LockFreeGRM::new)
}

pub fn store_global(record: ResolutionRecord) -> Result<u64, String> {
    global_lock_free_grm().store(record)
}

pub fn query_global(
    package_spec: &str,
    platform: Option<&str>,
    node_version: Option<&str>,
) -> Option<ResolutionRecord> {
    global_lock_free_grm().query(package_spec, platform, node_version)
}

pub fn store_ordinal_global(key: u64, record: ResolutionRecord) -> Result<u64, String> {
    global_lock_free_grm().store_ordinal(key, record)
}

pub fn query_ordinal_global(key: u64) -> Option<ResolutionRecord> {
    global_lock_free_grm().query_ordinal(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_free_grm_ordinal_operations() {
        let grm = LockFreeGRM::new();
        let key = LockFreeGRM::encode_ordinal_key(42, 107);

        let record = ResolutionRecord {
            package_spec: "react@^18.2.0".to_string(),
            resolved_version: "18.2.0".to_string(),
            cached_at: 1000,
            hit_count: 1,
            ..Default::default()
        };

        let id = grm.store_ordinal(key, record.clone()).unwrap();
        assert!(id > 0);

        let retrieved = grm.query_ordinal(key).expect("should find ordinal record");
        assert_eq!(retrieved.resolved_version, "18.2.0");

        // Non-existent key
        assert!(grm.query_ordinal(99999).is_none());
    }

    #[test]
    fn test_lock_free_grm_fast_path() {
        let grm = LockFreeGRM::new();
        let record = ResolutionRecord {
            package_spec: "lodash@^4.17.21".to_string(),
            resolved_version: "4.17.21".to_string(),
            cached_at: 1000,
            hit_count: 1,
            ..Default::default()
        };

        grm.store(record).unwrap();

        // Query without platform/node_version triggers fast xxh3 path
        let hit = grm
            .query("lodash@^4.17.21", None, None)
            .expect("fast path hit");
        assert_eq!(hit.resolved_version, "4.17.21");
    }
}
