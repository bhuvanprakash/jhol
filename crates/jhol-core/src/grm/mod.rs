//! Global Resolution Memory (GRM)
//!
//! Persistent, local-first knowledge base of resolution results.
//! Works 100% offline. Optional sync to any cloud storage you provide.
//!
//! Key features:
//! - Local-first (no network dependency)
//! - User-owned cloud (you provide storage)
//! - Learning (each successful resolution improves future hits)
//! - Privacy (no data sent anywhere by default)
//! - Cross-platform (sync across machines)

pub mod query;
pub mod store;
pub mod sync;

pub use query::{FreshnessRequirement, GRMQuery};
pub use store::GRMStore;
pub use sync::{SyncConfig, SyncProvider, SyncStatus};

use crate::mpt_store::ContentHash;
use dashmap::DashMap;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Process-global GRM stores, keyed by cache directory.
///
/// The outer RwLock protects lazy insertion/removal of stores while DashMap shard
/// locks allow concurrent per-store access by resolver threads.
pub type GlobalGRMStores = DashMap<String, GRMStore>;

/// GRM Statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GRMStats {
    pub total_records: u64,
    pub total_hits: u64,
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub low_priority_records: u64,
    pub unique_packages: u64,
    pub next_id: u64,
}

/// Resolution source
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResolutionSource {
    Network,
    Cache,
    GRM,
    Fallback,
}

/// Resolution record - stored in GRM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionRecord {
    pub id: u64,
    pub package_spec: String,
    pub resolved_version: String,
    pub content_hash: ContentHash,
    pub dependency_hashes: Vec<ContentHash>,
    pub platform: Option<String>,
    pub node_version: Option<String>,
    pub resolution_hash: ContentHash,
    #[serde(default)]
    pub provenance_sequence: Option<u64>,
    pub cached_at: u64,
    pub hit_count: u64,
    pub source: ResolutionSource,
    #[serde(default = "default_cache_priority")]
    pub cache_priority: u8,
}

fn default_cache_priority() -> u8 {
    100
}

impl ResolutionRecord {
    /// Check if this record matches a query
    #[must_use]
    pub fn matches_query(&self, query: &GRMQuery) -> bool {
        // Check package spec
        if !query.package_spec.is_empty() && query.package_spec != self.package_spec {
            // Also try matching just the package name
            let query_name = query
                .package_spec
                .split('@')
                .next()
                .unwrap_or(&query.package_spec);
            let spec_name = self
                .package_spec
                .split('@')
                .next()
                .unwrap_or(&self.package_spec);
            if query_name != spec_name {
                return false;
            }
        }

        // Check platform
        if let Some(ref query_platform) = query.platform {
            if let Some(ref record_platform) = self.platform {
                if query_platform != record_platform {
                    return false;
                }
            }
        }

        // Check node version
        if let Some(ref query_node) = query.node_version {
            if let Some(ref record_node) = self.node_version {
                if query_node != record_node {
                    return false;
                }
            }
        }

        // Check freshness
        if let Some(freshness) = query.freshness {
            let age = current_timestamp() - self.cached_at;
            if !freshness.is_fresh(age) {
                return false;
            }
        }

        true
    }
}

/// Return the key used for the global GRM store for the current cache directory.
pub fn global_store_key() -> String {
    crate::utils::get_cache_dir()
}

fn global_store_path() -> PathBuf {
    PathBuf::from(global_store_key()).join("grm")
}

fn global_store_cell() -> &'static RwLock<GlobalGRMStores> {
    static GLOBAL_STORES: OnceLock<RwLock<GlobalGRMStores>> = OnceLock::new();
    GLOBAL_STORES.get_or_init(|| RwLock::new(DashMap::new()))
}

fn ensure_current_global_store() {
    let key = global_store_key();
    {
        let stores = global_store_cell().read();
        if stores.contains_key(&key) {
            return;
        }
    }

    let stores = global_store_cell().write();
    if stores.contains_key(&key) {
        return;
    }

    match GRMStore::new(global_store_path()) {
        Ok(store) => {
            stores.insert(key, store);
        }
        Err(e) => {
            crate::safe_file::log_error(&format!(
                "GRM global store initialization failed: {}. Falling back to in-memory GRM.",
                e
            ));
            stores.insert(key, GRMStore::memory());
        }
    }
}

/// Get a read guard for the process-global GRM store map.
///
/// The guard always contains an entry for [`global_store_key()`], initialized
/// lazily from `JHOL_CACHE_DIR`/`utils::get_cache_dir()`.
pub fn get_global_store() -> RwLockReadGuard<'static, GlobalGRMStores> {
    ensure_current_global_store();
    global_store_cell().read()
}

/// Get a write guard for the process-global GRM store map.
///
/// This is intended for tests and rare administrative operations; normal query
/// and store paths should use [`get_global_store()`] plus DashMap entry guards.
pub fn get_global_store_mut() -> RwLockWriteGuard<'static, GlobalGRMStores> {
    ensure_current_global_store();
    global_store_cell().write()
}

/// Current timestamp in milliseconds
pub(crate) fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
