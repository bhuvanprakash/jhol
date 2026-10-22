//! Distributed GRM — P2P resolution network foundation (was `distributed_grm.rs`).

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct P2PResolutionRecord {
    pub package_spec: String,
    pub resolved_version: String,
    pub content_hash: String,
    pub platform: String,
    pub node_version: Option<String>,
    pub peer_count: u64,
}

pub struct DistributedGRM {
    local_cache: HashMap<String, P2PResolutionRecord>,
}

impl Default for DistributedGRM {
    fn default() -> Self {
        Self::new()
    }
}

impl DistributedGRM {
    pub fn new() -> Self {
        Self {
            local_cache: HashMap::new(),
        }
    }

    pub fn query_dht(&self, package_spec: &str) -> Option<P2PResolutionRecord> {
        self.local_cache.get(package_spec).cloned()
    }

    pub fn publish_to_dht(&mut self, record: P2PResolutionRecord) {
        self.local_cache.insert(record.package_spec.clone(), record);
    }
}

use parking_lot::RwLock;
use std::sync::OnceLock;

fn global_dgrm() -> &'static RwLock<DistributedGRM> {
    static INSTANCE: OnceLock<RwLock<DistributedGRM>> = OnceLock::new();
    INSTANCE.get_or_init(|| RwLock::new(DistributedGRM::new()))
}

pub fn query_distributed(package_spec: &str) -> Option<P2PResolutionRecord> {
    global_dgrm().read().query_dht(package_spec)
}

pub fn publish_resolution(record: P2PResolutionRecord) {
    global_dgrm().write().publish_to_dht(record);
}
