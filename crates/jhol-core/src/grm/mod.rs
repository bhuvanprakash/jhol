//! Global Resolution Memory — stub for compatibility.

pub mod store;
pub mod query;
pub mod sync;

use std::time::SystemTime;
use serde::{Serialize, Deserialize};
use crate::mpt_store::ContentHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ResolutionSource {
    #[default]
    GRM,
    SoulPath,
    Cache,
    Registry,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolutionRecord {
    pub package_spec: String,
    pub resolved_version: String,
    pub id: u64,
    pub timestamp: u64,
    pub ttl_secs: u64,
    pub dependencies: std::collections::HashMap<String, String>,
    pub platform: Option<String>,
    pub node_version: Option<String>,
    pub cached_at: u64,
    pub hit_count: u64,
    pub content_hash: ContentHash,
    pub dependency_hashes: Vec<ContentHash>,
    pub resolution_hash: ContentHash,
    pub provenance_sequence: Option<u64>,
    pub source: ResolutionSource,
    pub cache_priority: u32,
}

#[derive(Debug, Clone, Default)]
pub struct GRMStats {
    pub total_records: u64,
    pub total_hits: u64,
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub unique_packages: u64,
    pub next_id: u64,
    pub low_priority_records: u64,
}

pub fn current_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub struct GrmHandle;
impl GrmHandle {
    pub fn new() -> Self { Self }
    pub fn current_timestamp() -> u64 { current_timestamp() }
}

pub fn lookup(spec: &str) -> Option<ResolutionRecord> {
    crate::lock_free_grm::query_global(spec, None, None)
}

pub fn insert(record: ResolutionRecord) {
    let _ = crate::lock_free_grm::store_global(record);
}

pub fn garbage_collect() {}
