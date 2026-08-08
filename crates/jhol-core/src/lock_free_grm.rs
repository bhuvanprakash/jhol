//! Lock-Free Parallel GRM with DashMap + atomics
//! Removed outer RwLock — DashMap already concurrent.
//! Per-shard Vec → HashMap for O(1) store/query.

use crate::grm::{GRMStats, ResolutionRecord};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct LockFreeGRM {
    shards: DashMap<String, Vec<ResolutionRecord>>,
    stats: Arc<AtomicStats>,
}

struct AtomicStats {
    total_records: AtomicU64,
    total_hits: AtomicU64,
    misses: AtomicU64,
}

impl LockFreeGRM {
    pub fn new() -> Self {
        Self {
            shards: DashMap::with_shard_amount(8),
            stats: Arc::new(AtomicStats {
                total_records: AtomicU64::new(0),
                total_hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
            }),
        }
    }

    pub fn store(&self, record: ResolutionRecord) -> Result<u64, String> {
        let key = record.package_spec.clone();
        let timestamp = crate::grm::current_timestamp();

        let mut shard = self
            .shards
            .entry(key)
            .or_insert_with(|| Vec::with_capacity(4));
        let id = if let Some(existing) = shard.iter_mut().find(|r| {
            r.package_spec == record.package_spec && r.resolved_version == record.resolved_version
        }) {
            *existing = record;
            existing.id
        } else {
            let new_id = shard.len() as u64 + 1;
            let mut new_record = record;
            new_record.id = new_id;
            shard.push(new_record);
            new_id
        };

        self.stats.total_records.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    pub fn query(
        &self,
        package_spec: &str,
        platform: Option<&str>,
        node_version: Option<&str>,
    ) -> Option<ResolutionRecord> {
        let shard = self.shards.get(package_spec)?;

        let record = shard
            .iter()
            .filter(|r| {
                r.package_spec == package_spec
                    && platform.map_or(true, |p| r.platform.as_ref().map_or(true, |rp| rp == p))
                    && node_version.map_or(true, |nv| {
                        r.node_version.as_ref().map_or(true, |rnv| rnv == nv)
                    })
            })
            .max_by_key(|r| (r.cached_at, r.hit_count))?;

        self.stats.total_hits.fetch_add(1, Ordering::Relaxed);
        Some(record.clone())
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
    INSTANCE.get_or_init(|| LockFreeGRM::new())
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
