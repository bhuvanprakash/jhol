//! JMR: JAGR Mirror Resolution — glued sheaf sections + lattice hits.
//! Soul Path memory: last successful resolution sections keyed by fingerprint.

use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct MirrorEntry {
    pub package: String,
    pub version: String,
    pub resolved_version: String,
    pub integrity: Option<String>,
    pub resolved_url: Option<String>,
    pub tarball_url: Option<String>,
    pub deps: HashMap<String, String>,
    pub dependencies: HashMap<String, String>,
    pub dep_signature: String,
}

#[derive(Clone, Debug, Default)]
pub struct MirrorStats {
    pub hits: u64,
    pub misses: u64,
    pub stores: u64,
    pub total_entries: u64,
    pub fresh_entries: u64,
    pub expired_entries: u64,
}

#[derive(Clone, Debug)]
pub struct Prediction {
    pub package: String,
    pub confidence: f64,
}

struct Section {
    entries: HashMap<String, MirrorEntry>,
    stored_at: Instant,
}

pub struct Mirror {
    /// fingerprint (manifest/hash) → glued section
    sections: DashMap<u64, Section>,
    /// package → latest entry (cross-project)
    by_pkg: DashMap<String, MirrorEntry>,
    stats: RwLock<MirrorStats>,
}

impl Mirror {
    fn new() -> Self {
        Self {
            sections: DashMap::new(),
            by_pkg: DashMap::new(),
            stats: RwLock::new(MirrorStats::default()),
        }
    }

    pub fn store_section(&self, fingerprint: u64, entries: HashMap<String, MirrorEntry>) {
        for (k, v) in &entries {
            self.by_pkg.insert(k.clone(), v.clone());
        }
        self.sections.insert(
            fingerprint,
            Section {
                entries,
                stored_at: Instant::now(),
            },
        );
        let mut s = self.stats.write();
        s.stores = s.stores.saturating_add(1);
        s.total_entries = self.by_pkg.len() as u64;
        s.fresh_entries = s.total_entries;
    }

    pub fn get_section(&self, fingerprint: u64) -> Option<HashMap<String, MirrorEntry>> {
        let sec = self.sections.get(&fingerprint)?;
        if sec.stored_at.elapsed() > Duration::from_secs(3600) {
            return None;
        }
        self.stats.write().hits = self.stats.write().hits.saturating_add(1);
        Some(sec.entries.clone())
    }

    pub fn lookup(&self, name: &str, range: &str) -> Option<MirrorEntry> {
        if let Some(e) = self.by_pkg.get(name) {
            let version = &e.version;
            let ok = if range == "*" || range == "latest" {
                true
            } else {
                crate::registry::version_satisfies(range, version)
            };
            if ok {
                self.stats.write().hits = self.stats.write().hits.saturating_add(1);
                return Some(e.clone());
            }
        }
        // Lattice fallback
        if let Some(v) = crate::lattice_cache::lookup_lattice(name, range) {
            self.stats.write().hits = self.stats.write().hits.saturating_add(1);
            use std::hash::Hasher;
            let mut h2 = std::collections::hash_map::DefaultHasher::new();
            let hash = format!("{:x}", Hasher::finish(&h2));
            return Some(MirrorEntry {
                package: name.to_string(),
                version: v.clone(),
                resolved_version: v.clone(),
                integrity: None,
                resolved_url: None,
                tarball_url: None,
                deps: HashMap::new(),
                dependencies: HashMap::new(),
                dep_signature: hash,
            });
        }
        self.stats.write().misses = self.stats.write().misses.saturating_add(1);
        None
    }

    pub fn lookup_pkg(&self, package: &str) -> Option<MirrorEntry> {
        self.lookup(package, "*")
    }

    pub fn prune(&self) -> Result<usize, String> {
        let before = self.by_pkg.len();
        // Remove expired sections
        self.sections.retain(|_, sec| sec.stored_at.elapsed() <= Duration::from_secs(3600));
        // Rebuild by_pkg from remaining sections
        self.by_pkg.clear();
        for ref_multi in self.sections.iter() {
            for (k, v) in &ref_multi.entries {
                self.by_pkg.insert(k.clone(), v.clone());
            }
        }
        let after = self.by_pkg.len();
        let mut s = self.stats.write();
        s.total_entries = after as u64;
        s.expired_entries = (before.saturating_sub(after)) as u64;
        s.fresh_entries = after as u64;
        Ok(before.saturating_sub(after))
    }

    pub fn clear(&self) -> Result<(), String> {
        self.sections.clear();
        self.by_pkg.clear();
        let mut s = self.stats.write();
        s.total_entries = 0;
        s.fresh_entries = 0;
        s.expired_entries = 0;
        Ok(())
    }

    pub fn stats(&self) -> MirrorStats {
        let mut s = self.stats.read().clone();
        let now = std::time::Instant::now();
        let mut fresh = 0u64;
        for ref_multi in self.sections.iter() {
            if ref_multi.stored_at.elapsed() <= Duration::from_secs(3600) {
                fresh = fresh.saturating_add(ref_multi.entries.len() as u64);
            }
        }
        s.total_entries = self.by_pkg.len() as u64;
        s.fresh_entries = fresh;
        s.expired_entries = s.total_entries.saturating_sub(fresh);
        s
    }

    pub fn predict(&self, context: &[String], n: usize) -> Vec<Prediction> {
        if context.len() < 2 {
            return Vec::new();
        }
        let prev = &context[context.len() - 2];
        let curr = &context[context.len() - 1];
        crate::second_order_markov::predict_with_context(prev, curr, n)
            .into_iter()
            .enumerate()
            .map(|(i, package)| Prediction {
                package,
                confidence: 1.0 - (i as f64 * 0.15),
            })
            .collect()
    }
}

static MIRROR: OnceLock<Mirror> = OnceLock::new();

pub fn global_mirror() -> &'static Mirror {
    MIRROR.get_or_init(Mirror::new)
}

/// Fingerprint package.json deps map.
pub fn fingerprint_deps(deps: &HashMap<String, String>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut pairs: Vec<_> = deps.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (k, v) in pairs {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    Hasher::finish(&h)
}

/// Store a successful resolve into JMR + lattice + GRM.
pub fn record_resolution(
    fingerprint: u64,
    packages: &HashMap<String, crate::lockfile_write::ResolvedPackage>,
) {
    let mut entries = HashMap::new();
    let mut lattice_batch = Vec::new();
    let platform = Some(std::env::consts::OS.to_string());
    let node = std::env::var("NODE_VERSION").ok();

    for (name, pkg) in packages {
        let dep_sig = dep_signature(&pkg.dependencies);
        entries.insert(
            name.clone(),
            MirrorEntry {
                package: name.clone(),
                version: pkg.version.clone(),
                resolved_version: pkg.version.clone(),
                integrity: pkg.integrity.clone(),
                resolved_url: if pkg.resolved.is_empty() {
                    None
                } else {
                    Some(pkg.resolved.clone())
                },
                tarball_url: if pkg.resolved.is_empty() {
                    None
                } else {
                    Some(pkg.resolved.clone())
                },
                deps: pkg.dependencies.clone(),
                dependencies: pkg.dependencies.clone(),
                dep_signature: dep_sig,
            },
        );
        lattice_batch.push((name.clone(), "*".to_string(), pkg.version.clone()));
        let _ = crate::lock_free_grm::store_global(crate::grm::ResolutionRecord {
            package_spec: name.clone(),
            resolved_version: pkg.version.clone(),
            id: 0,
            timestamp: crate::grm::current_timestamp(),
            ttl_secs: 86400,
            dependencies: pkg.dependencies.clone(),
            platform: platform.clone(),
            node_version: node.clone(),
            cached_at: crate::grm::current_timestamp(),
            hit_count: 0,
            ..Default::default()
        });
    }
    crate::lattice_cache::remember_many(&lattice_batch);
    global_mirror().store_section(fingerprint, entries);
}

/// SNR: store a speculative neighbor resolution.
/// Keyed by (parent_fingerprint, edit_descriptor).
/// edit_descriptor describes the change (e.g., "add:react@^18", "update:lodash@^5").
pub fn snr_put(
    parent_fp: u64,
    edit: &str,
    entries: HashMap<String, MirrorEntry>,
) {
    // Store under a composite key: hash(parent_fp || edit)
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parent_fp.hash(&mut hasher);
    edit.hash(&mut hasher);
    let composite_fp = Hasher::finish(&hasher);
    global_mirror().store_section(composite_fp, entries);
}

/// SNR: retrieve a speculative neighbor resolution.
pub fn snr_get(
    parent_fp: u64,
    edit: &str,
) -> Option<HashMap<String, MirrorEntry>> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parent_fp.hash(&mut hasher);
    edit.hash(&mut hasher);
    let composite_fp = Hasher::finish(&hasher);
    global_mirror().get_section(composite_fp)
}

fn dep_signature(deps: &HashMap<String, String>) -> String {
    use std::hash::{Hash, Hasher};
    let mut pairs: Vec<_> = deps.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (k, v) in pairs {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    format!("{:x}", Hasher::finish(&h))
}
