//! Lattice Resolution Cache — (package, constraint) → version.
//! Soul Path memory plane: constraint lattice, not dumb KV.
//!
//! If we know C₂ → v and C₁ is satisfied by v (and C₁ is "tighter or equal"
//! in the sense that v ∈ C₁), answer without network.

use parking_lot::RwLock;
use semver::{Version, VersionReq};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

// Include build-time resolution seed from binary-packages index.
#[cfg(not(test))]
include!(concat!(env!("OUT_DIR"), "/resolution_seed_data.rs"));

#[derive(Clone, Debug)]
struct LatticeEntry {
    constraint: String,
    version: String,
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

fn load_disk(store: &mut LatticeStore) {
    let path = disk_path();
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Ok(map) = serde_json::from_str::<HashMap<String, Vec<(String, String)>>>(&raw) else {
        return;
    };
    for (pkg, entries) in map {
        store.by_pkg.insert(
            pkg,
            entries
                .into_iter()
                .map(|(constraint, version)| LatticeEntry {
                    constraint,
                    version,
                    hits: 0,
                })
                .collect(),
        );
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

/// Remember that `constraint` resolved to `version` for `package`.
/// Also stores in GRM shards for cross-project closed-world memory.
pub fn remember(package: &str, constraint: &str, version: &str) {
    let mut g = global().write();
    let list = g.by_pkg.entry(package.to_string()).or_default();
    if let Some(e) = list.iter_mut().find(|e| e.constraint == constraint) {
        e.version = version.to_string();
        e.hits = e.hits.saturating_add(1);
    } else {
        list.push(LatticeEntry {
            constraint: constraint.to_string(),
            version: version.to_string(),
            hits: 1,
        });
    }
    // Cap per package
    if list.len() > 64 {
        list.sort_by(|a, b| b.hits.cmp(&a.hits));
        list.truncate(64);
    }
    save_disk(&g);

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
    let mut best: Option<Version> = None;
    for e in list {
        let Ok(ver) = Version::parse(&e.version) else {
            continue;
        };
        let ok = if let Some(ref req) = req {
            req.matches(&ver)
        } else {
            // bare / latest / *
            constraint == "latest"
                || constraint == "*"
                || constraint.is_empty()
                || e.version == constraint
                || crate::registry::version_satisfies(constraint, &e.version)
        };
        if ok {
            if best.as_ref().map(|b| &ver > b).unwrap_or(true) {
                best = Some(ver);
            }
        }
    }
    best.map(|v| v.to_string())
}

/// Batch lattice lookup: returns resolved versions for multiple packages at once.
/// Returns a map of package → version for every package that has a lattice hit.
/// Uses GRM first, then lattice, then falls through (caller handles network miss).
pub fn lookup_lattice_batch(
    deps: &[(&str, &str)],
) -> std::collections::HashMap<String, String> {
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
        let mut best: Option<semver::Version> = None;
        for e in list {
            let Ok(ver) = semver::Version::parse(&e.version) else {
                continue;
            };
            let ok = if let Some(ref req) = req {
                req.matches(&ver)
            } else {
                constraint == "latest" || constraint == "*" || constraint.is_empty()
            };
            if ok {
                if best.as_ref().map(|b| &ver > b).unwrap_or(true) {
                    best = Some(ver);
                }
            }
        }
        if let Some(v) = best {
            results.insert(package.to_string(), v.to_string());
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
