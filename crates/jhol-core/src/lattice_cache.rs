//! Lattice Resolution Cache — (package, constraint) → version.
//! Soul Path memory plane: constraint lattice, not dumb KV.
//!
//! If we know C₂ → v and C₁ is satisfied by v (and C₁ is "tighter or equal"
//! in the sense that v ∈ C₁), answer without network.

use parking_lot::RwLock;
use semver::VersionReq;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

// Include build-time resolution seed from binary-packages index.
#[cfg(not(test))]
include!(concat!(env!("OUT_DIR"), "/resolution_seed_data.rs"));

#[derive(Clone, Debug)]
struct LatticeEntry {
    constraint: String,
    version: String,
    parsed_version: semver::Version,
    hits: u64,
}

struct LatticeStore {
    /// package → list of (constraint, version)
    by_pkg: HashMap<String, Vec<LatticeEntry>>,
}

impl LatticeStore {
    fn new() -> Self {
        Self {
            by_pkg: HashMap::new(),
        }
    }
}

fn global() -> &'static RwLock<LatticeStore> {
    static G: OnceLock<RwLock<LatticeStore>> = OnceLock::new();
    G.get_or_init(|| {
        // `mut` is required by the non-test configuration below (`load_disk` /
        // `load_seed_entries` take `&mut`); clippy --fix, run without those cfgs
        // active, strips it and breaks the release build.
        #[allow(unused_mut)]
        let mut store = LatticeStore::new();
        #[cfg(not(test))]
        {
            load_disk(&mut store);
            // Load build-time resolution seed so the truly-first cold install
            // avoids packument fetches for common packages.
            load_seed_entries(&mut store);
        }
        RwLock::new(store)
    })
}

fn disk_path() -> PathBuf {
    PathBuf::from(crate::utils::get_cache_dir()).join("lattice_cache.json")
}

#[allow(dead_code)]
fn load_disk(store: &mut LatticeStore) {
    let path = disk_path();
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Ok(map) = serde_json::from_str::<HashMap<String, Vec<(String, String)>>>(&raw) else {
        return;
    };
    for (pkg, entries) in map {
        let mut parsed: Vec<LatticeEntry> = entries
            .into_iter()
            .filter_map(|(constraint, version)| {
                let parsed_version = semver::Version::parse(&version).ok()?;
                Some(LatticeEntry {
                    constraint,
                    version,
                    parsed_version,
                    hits: 0,
                })
            })
            .collect();
        parsed.sort_by(|a, b| b.parsed_version.cmp(&a.parsed_version));
        store.by_pkg.insert(pkg, parsed);
    }
}

fn save_disk(store: &LatticeStore) {
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (pkg, entries) in &store.by_pkg {
        out.insert(
            pkg.clone(),
            entries
                .iter()
                .map(|e| (e.constraint.clone(), e.version.clone()))
                .collect(),
        );
    }
    if let Ok(s) = serde_json::to_string(&out) {
        let path = disk_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, s);
    }
}

pub fn cache_key(name: &str, constraint: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    constraint.hash(&mut h);
    h.finish()
}

static PENDING: OnceLock<Mutex<Vec<(String, String, String)>>> = OnceLock::new();

fn pending() -> &'static Mutex<Vec<(String, String, String)>> {
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Flush buffered remembers to disk in a single write.
pub fn flush_disk() {
    let _batch = match pending().lock() {
        Ok(mut buf) if !buf.is_empty() => std::mem::take(&mut *buf),
        _ => return,
    };
    let g = global().read();
    save_disk(&g);
}

/// Remember that `constraint` resolved to `version` for `package`.
/// Updates in-memory state immediately; disk write is buffered.
pub fn remember(package: &str, constraint: &str, version: &str) {
    let mut g = global().write();
    let list = g.by_pkg.entry(package.to_string()).or_default();
    if let Some(e) = list.iter_mut().find(|e| e.constraint == constraint) {
        e.version = version.to_string();
        if let Ok(pv) = semver::Version::parse(version) {
            e.parsed_version = pv;
        }
        e.hits = e.hits.saturating_add(1);
    } else if let Ok(parsed_version) = semver::Version::parse(version) {
        list.push(LatticeEntry {
            constraint: constraint.to_string(),
            version: version.to_string(),
            parsed_version,
            hits: 1,
        });
        // Keep sorted by version descending for binary search
        list.sort_by(|a, b| b.parsed_version.cmp(&a.parsed_version));
    }
    // Cap per package
    if list.len() > 64 {
        list.sort_by(|a, b| b.hits.cmp(&a.hits));
        list.truncate(64);
        list.sort_by(|a, b| b.parsed_version.cmp(&a.parsed_version));
    }
    drop(g);

    // Buffer disk write; auto-flush when buffer is large
    if let Ok(mut buf) = pending().lock() {
        buf.push((
            package.to_string(),
            constraint.to_string(),
            version.to_string(),
        ));
        if buf.len() >= 64 {
            drop(buf);
            flush_disk();
        }
    }

    // Cross-project: also store in GRM shards so other projects or daemon
    // queries hit without needing this JSON file loaded.
    let _ = crate::lock_free_grm::store_global(crate::grm::ResolutionRecord {
        package_spec: format!("{}@{}", package, version),
        resolved_version: version.to_string(),
        id: 0,
        timestamp: crate::grm::current_timestamp(),
        ttl_secs: 7200,
        dependencies: std::collections::HashMap::new(),
        platform: Some(std::env::consts::OS.to_string()),
        node_version: std::env::var("NODE_VERSION").ok(),
        cached_at: crate::grm::current_timestamp(),
        hit_count: 1,
        ..Default::default()
    });
}

/// Lookup exact constraint hit.
pub fn lookup_exact(package: &str, constraint: &str) -> Option<String> {
    let g = global().read();
    g.by_pkg
        .get(package)?
        .iter()
        .find(|e| e.constraint == constraint)
        .map(|e| e.version.clone())
}

/// Lattice lookup: any remembered version that satisfies the new constraint.
pub fn lookup_lattice(package: &str, constraint: &str) -> Option<String> {
    if let Some(v) = lookup_exact(package, constraint) {
        return Some(v);
    }
    let req = VersionReq::parse(constraint.trim()).ok();
    let g = global().read();
    let list = g.by_pkg.get(package)?;
    // Entries are sorted by version descending. First match is the highest.
    for e in list {
        let ok = if let Some(ref r) = req {
            r.matches(&e.parsed_version)
        } else {
            constraint == "latest"
                || constraint == "*"
                || constraint.is_empty()
                || e.version == constraint
                || crate::registry::version_satisfies(constraint, &e.version)
        };
        if ok {
            return Some(e.version.clone());
        }
    }
    None
}

/// Batch lattice lookup: returns resolved versions for multiple packages at once.
/// Returns a map of package → version for every package that has a lattice hit.
/// Uses GRM first, then lattice, then falls through (caller handles network miss).
pub fn lookup_lattice_batch(deps: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    let g = global().read();
    let mut results = std::collections::HashMap::new();
    for &(package, constraint) in deps {
        // Try GRM first (cross-project memory) — query by package@constraint
        let pkg_spec = format!("{}@{}", package, constraint);
        if let Some(rec) = crate::lock_free_grm::query_global(
            &pkg_spec,
            Some(std::env::consts::OS),
            std::env::var("NODE_VERSION").ok().as_deref(),
        ) {
            if crate::registry::version_satisfies(constraint, &rec.resolved_version) {
                results.insert(package.to_string(), rec.resolved_version.clone());
                continue;
            }
        }
        // Try exact match
        let list = match g.by_pkg.get(package) {
            Some(l) => l,
            None => continue,
        };
        let req = semver::VersionReq::parse(constraint.trim()).ok();
        // Entries sorted by version desc; first match is highest.
        let matched = list.iter().find(|e| {
            if let Some(ref r) = req {
                r.matches(&e.parsed_version)
            } else {
                constraint == "latest" || constraint == "*" || constraint.is_empty()
            }
        });
        if let Some(e) = matched {
            results.insert(package.to_string(), e.version.clone());
        }
    }
    results
}

/// Batch remember after a full resolve.
pub fn remember_many(resolved: &[(String, String, String)]) {
    // (package, constraint, version)
    for (pkg, constraint, version) in resolved {
        remember(pkg, constraint, version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_satisfies_tighter() {
        remember("lodash", "^4.0.0", "4.17.21");
        let hit = lookup_lattice("lodash", "^4.17.0");
        assert_eq!(hit.as_deref(), Some("4.17.21"));
    }
}
