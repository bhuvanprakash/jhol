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
    /// Registry identity → list of (constraint, version).
    by_pkg: HashMap<String, Vec<LatticeEntry>>,
    /// Pre-origin cache entries stay on disk, but can never satisfy a lookup.
    legacy_entries: HashMap<String, Vec<(String, String)>>,
}

impl LatticeStore {
    fn new() -> Self {
        Self {
            by_pkg: HashMap::new(),
            legacy_entries: HashMap::new(),
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
        if !pkg.starts_with("metadata-v2:") {
            store.legacy_entries.insert(pkg, entries);
            continue;
        }
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
    let mut out = store.legacy_entries.clone();
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

/// Retain an originless legacy entry for migration diagnostics.
/// It is never eligible to resolve a package; use an identity-bound writer.
pub fn remember(package: &str, constraint: &str, version: &str) {
    remember_legacy(package, constraint, version);
}

fn remember_legacy(package: &str, constraint: &str, version: &str) {
    let mut g = global().write();
    let entries = g.legacy_entries.entry(package.to_string()).or_default();
    if let Some(entry) = entries.iter_mut().find(|entry| entry.0 == constraint) {
        entry.1 = version.to_string();
    } else {
        entries.push((constraint.to_string(), version.to_string()));
    }
    drop(g);

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
}

/// Remember a version under the captured registry and credential identity.
/// Remember a version using an already-computed metadata identity key.
pub(crate) fn remember_for_key(package: &str, constraint: &str, version: &str, key: &str) {
    remember_scoped_key(package, constraint, version, key.to_string());
}

fn remember_scoped_key(_package: &str, constraint: &str, version: &str, key: String) {
    let mut g = global().write();
    let list = g.by_pkg.entry(key.clone()).or_default();
    if let Some(entry) = list.iter_mut().find(|entry| entry.constraint == constraint) {
        entry.version = version.to_string();
        if let Ok(parsed) = semver::Version::parse(version) {
            entry.parsed_version = parsed;
        }
        entry.hits = entry.hits.saturating_add(1);
    } else if let Ok(parsed_version) = semver::Version::parse(version) {
        list.push(LatticeEntry {
            constraint: constraint.to_string(),
            version: version.to_string(),
            parsed_version,
            hits: 1,
        });
        list.sort_by(|a, b| b.parsed_version.cmp(&a.parsed_version));
    }
    list.sort_by(|a, b| b.parsed_version.cmp(&a.parsed_version));
    if list.len() > 64 {
        list.sort_by(|a, b| b.hits.cmp(&a.hits));
        list.truncate(64);
        list.sort_by(|a, b| b.parsed_version.cmp(&a.parsed_version));
    }
    drop(g);
    if let Ok(mut buf) = pending().lock() {
        buf.push((key, constraint.to_string(), version.to_string()));
        if buf.len() >= 64 {
            drop(buf);
            flush_disk();
        }
    }
}

/// Originless compatibility lookup. Always misses because the source is unknown.
pub fn lookup_exact(package: &str, constraint: &str) -> Option<String> {
    let _ = (package, constraint);
    None
}

pub(crate) fn lookup_lattice_for_key(key: &str, constraint: &str) -> Option<String> {
    if let Some(version) = lookup_exact_for_key(key, constraint) {
        return Some(version);
    }
    let req = VersionReq::parse(constraint.trim()).ok();
    let g = global().read();
    let list = g.by_pkg.get(key)?;
    list.iter().find_map(|entry| {
        let matches = if let Some(ref requirement) = req {
            requirement.matches(&entry.parsed_version)
        } else {
            constraint == "latest"
                || constraint == "*"
                || constraint.is_empty()
                || entry.version == constraint
                || crate::registry::version_satisfies(constraint, &entry.version)
        };
        matches.then(|| entry.version.clone())
    })
}

pub(crate) fn lookup_exact_for_key(key: &str, constraint: &str) -> Option<String> {
    global()
        .read()
        .by_pkg
        .get(key)?
        .iter()
        .find(|entry| entry.constraint == constraint)
        .map(|entry| entry.version.clone())
}

/// Originless compatibility lookup. Always misses because the source is unknown.
pub fn lookup_lattice(package: &str, constraint: &str) -> Option<String> {
    let _ = (package, constraint);
    None
}

/// Originless compatibility batch lookup. Always empty because sources are unknown.
pub fn lookup_lattice_batch(deps: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    let _ = deps;
    std::collections::HashMap::new()
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
        let key = "metadata-v2:test-registry-lodash";
        remember_for_key("lodash", "^4.0.0", "4.17.21", key);
        let hit = lookup_lattice_for_key(key, "^4.17.0");
        assert_eq!(hit.as_deref(), Some("4.17.21"));
    }

    #[test]
    fn legacy_lattice_entries_are_preserved_but_never_used_for_resolution() {
        let package = "legacy-cache-boundary-probe";
        remember(package, "*", "9.9.9");
        assert_eq!(lookup_lattice(package, "*"), None);
        assert!(global().read().legacy_entries.contains_key(package));
    }

    #[test]
    fn scoped_lattice_entries_do_not_cross_registry_keys() {
        let a = "metadata-v2:registry-a-token-a";
        let b = "metadata-v2:registry-b-token-b";
        remember_for_key("private-pkg", "*", "1.0.0", a);
        assert_eq!(lookup_lattice_for_key(a, "*"), Some("1.0.0".into()));
        assert_eq!(lookup_lattice_for_key(b, "*"), None);
    }
}
