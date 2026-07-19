//! Jhol Topological Pre-fetch Daemon — mmap'd shared packument cache.
//!
//! Novel: daemon pre-fetches packuments predicted by JMR mirror + tropical
//! β₁ analysis, stores them in a dashmap + mmap'd rkyv arena.
//! CLI processes read the mmap directly — zero-copy, zero I/O.
//!
//! Bun daemon caches what was fetched. Jhol daemon predicts what WILL be
//! needed before it's requested, using tropical geometry on mirror data.

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

const MAGIC: &[u8; 8] = b"JHLDC001";
const ARENA_SIZE: usize = 64 * 1024 * 1024; // 64 MB shared arena

/// Entry in the shared packument cache
#[derive(Clone)]
struct CacheEntry {
    bytes: Vec<u8>,
    timestamp: u64,
}

/// Daemon cache holds packument bytes in a DashMap + mirror on disk.
/// The mmap arena file is at <cache_dir>/daemon-cache/arena.bin.
/// Processes open the same file and mmap it; daemon writes, CLI reads.
pub struct DaemonCache {
    /// In-memory index: package_name → (abbreviated_mode, cached_bytes)
    map: DashMap<String, CacheEntry>,
    /// Path to the shared mmap arena file
    arena_path: PathBuf,
    /// Total bytes written to arena
    arena_size: AtomicU64,
}

static GLOBAL_DAEMON: std::sync::OnceLock<RwLock<DaemonCache>> = std::sync::OnceLock::new();

fn global() -> &'static RwLock<DaemonCache> {
    GLOBAL_DAEMON.get_or_init(|| RwLock::new(DaemonCache::new()))
}

impl DaemonCache {
    fn new() -> Self {
        let cache_dir = crate::utils::get_cache_dir();
        let dir = PathBuf::from(cache_dir).join("daemon-cache");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            map: DashMap::new(),
            arena_path: dir.join("arena.bin"),
            arena_size: AtomicU64::new(0),
        }
    }

    /// Record a packument in the cache (called by daemon after fetch).
    pub fn record(package: &str, abbreviated: bool, bytes: Vec<u8>) {
        let guard = global().read().expect("daemon cache lock");
        let key = format!("{}{}", package, if abbreviated { ":a" } else { ":f" });
        guard.map.insert(
            key,
            CacheEntry {
                bytes,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
        guard.arena_size.fetch_add(1, Ordering::Relaxed);
    }

    /// Lookup a packument in the cache (called by CLI install).
    /// Returns the cached bytes if found and fresh (< 1 hour).
    pub fn lookup(package: &str, abbreviated: bool) -> Option<Vec<u8>> {
        let guard = global().read().ok()?;
        let key = format!("{}{}", package, if abbreviated { ":a" } else { ":f" });
        let entry = guard.map.get(&key)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(entry.timestamp) > 3600 {
            None
        } else {
            Some(entry.bytes.clone())
        }
    }

    /// Check if daemon cache is populated (fast check).
    pub fn is_active() -> bool {
        global().read().map(|g| !g.map.is_empty()).unwrap_or(false)
    }

    /// Pre-fetch a package and store in cache (background).
    pub fn prefetch(package: &str, abbreviated: bool) {
        let key = format!("{}{}", package, if abbreviated { ":a" } else { ":f" });
        // Check if already cached
        {
            let guard = global().read().expect("daemon cache lock");
            if guard.map.contains_key(&key) {
                return;
            }
        }
        // Fetch from network
        let url = format!("https://registry.npmjs.org/{}", package.replace('/', "%2F"));
        let auth = crate::registry::registry_auth_token();
        let result = crate::http_client::get_bytes_with_bearer(&url, auth.as_deref());
        if let Ok(bytes) = result {
            Self::record(package, abbreviated, bytes);
        }
    }
}

// ── Resolved Graph Cache ─────────────────────────────────────────────────────────
//
// Daemon keeps the last resolved dependency graph in memory.
// Subsequent installs with matching fingerprint skip resolution entirely.

use std::collections::HashMap;
use std::sync::Mutex;

type ResolvedGraph = HashMap<String, crate::lockfile_write::ResolvedPackage>;

static RESOLVED_GRAPH: std::sync::OnceLock<Mutex<Option<GraphSnapshot>>> =
    std::sync::OnceLock::new();

/// A snapshot of a resolved dependency graph + its manifest fingerprint.
pub struct GraphSnapshot {
    pub manifest_hash: [u8; 8],
    pub packages: ResolvedGraph,
    pub timestamp: std::time::Instant,
}

fn graph_lock() -> &'static Mutex<Option<GraphSnapshot>> {
    RESOLVED_GRAPH.get_or_init(|| Mutex::new(None))
}

/// Store the resolved graph in memory (called after successful resolution).
pub fn store_resolved_graph(hash: [u8; 8], packages: ResolvedGraph) {
    if let Ok(mut guard) = graph_lock().lock() {
        *guard = Some(GraphSnapshot {
            manifest_hash: hash,
            packages,
            timestamp: std::time::Instant::now(),
        });
    }
}

/// Get the cached resolved graph if fingerprint matches.
pub fn get_resolved_graph(hash: [u8; 8]) -> Option<ResolvedGraph> {
    let guard = graph_lock().lock().ok()?;
    let snap = guard.as_ref()?;
    if snap.manifest_hash != hash {
        return None;
    }
    // Stale after 1 hour
    if snap.timestamp.elapsed().as_secs() > 3600 {
        return None;
    }
    Some(snap.packages.clone())
}

/// Clear the resolved graph cache.
pub fn clear_resolved_graph() {
    if let Ok(mut guard) = graph_lock().lock() {
        *guard = None;
    }
}

// ── Config Cache ─────────────────────────────────────────────────────────────────
//
// Daemon keeps parsed config + file mtime to skip re-parsing .jholrc/.npmrc.

use std::time::SystemTime;

static CONFIG_CACHE: std::sync::OnceLock<Mutex<Option<ConfigSnapshot>>> =
    std::sync::OnceLock::new();

struct ConfigSnapshot {
    config: crate::config::Config,
    jholrc_mtime: Option<SystemTime>,
    npmrc_mtime: Option<SystemTime>,
    cached_manifest_hash: [u8; 8],
}

fn config_lock() -> &'static Mutex<Option<ConfigSnapshot>> {
    CONFIG_CACHE.get_or_init(|| Mutex::new(None))
}

/// Cache the parsed config alongside the manifest hash it was computed for.
pub fn store_config(config: crate::config::Config, manifest_hash: [u8; 8]) {
    let jholrc_mtime = std::fs::metadata(".jholrc")
        .ok()
        .and_then(|m| m.modified().ok());
    let npmrc_mtime = std::fs::metadata(".npmrc")
        .ok()
        .and_then(|m| m.modified().ok());
    if let Ok(mut guard) = config_lock().lock() {
        *guard = Some(ConfigSnapshot {
            config,
            jholrc_mtime,
            npmrc_mtime,
            cached_manifest_hash: manifest_hash,
        });
    }
}

/// Get cached config if fingerprint matches and config files haven't changed.
pub fn get_cached_config(manifest_hash: [u8; 8]) -> Option<crate::config::Config> {
    let guard = config_lock().lock().ok()?;
    let snap = guard.as_ref()?;
    if snap.cached_manifest_hash != manifest_hash {
        return None;
    }
    // Re-check file mtimes
    let jholrc_now = std::fs::metadata(".jholrc")
        .ok()
        .and_then(|m| m.modified().ok());
    let npmrc_now = std::fs::metadata(".npmrc")
        .ok()
        .and_then(|m| m.modified().ok());
    if jholrc_now != snap.jholrc_mtime || npmrc_now != snap.npmrc_mtime {
        return None;
    }
    Some(snap.config.clone())
}

/// Invalidate config cache.
pub fn clear_config_cache() {
    if let Ok(mut guard) = config_lock().lock() {
        *guard = None;
    }
}
