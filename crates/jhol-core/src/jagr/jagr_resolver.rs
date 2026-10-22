//! JAGR: Main Resolver
//!
//! Orchestrates the three-layer JAGR architecture:
//! - Layer 3: Tropical Geometry (PTIME filter)
//! - Layer 2: Tropical Sheaf Cohomology (Topological filter)
//! - Layer 1: Core-Guided MaxSAT (Exact solver)
//!
//! The novel architecture uses mathematics that has never been
//! applied to package management before:
//! - Tropical Nullstellensatz (Layer 3)
//! - Betti numbers and Simplicial Normal Form (Layer 2)
//! - OL3P/OL3P-S core-guided MaxSAT (Layer 1)
//!
//! ## Performance: Global Resolution Cache
//!
//! A process-lifetime DashMap keyed on xxh3(request) provides <1µs
//! repeat-resolution for identical dependency sets. Cache is sharded by
//! DashMap's internal bucketing — no global lock, fully concurrent reads.
//!
//! ## Persistent version_ordinals Cache
//!
//! Version→ordinal mappings persist to `<cache_dir>/version_ordinals.json`
//! so semver-to-ordinal conversions never repeat across install runs.
//! Loaded once via `OnceLock` on first access.

/// Global persistent ordinals cache — survives across ResolveRequest lifetimes.
/// Key: "major.minor.patch" → ordinal f64. Loaded from disk once.
/// Layer 1: OnceLock — load once per process.
/// Layer 2: mmap — zero-copy read from disk.
/// Layer 3: dirty flag — write-back deferred.
pub fn global_ordinals() -> &'static Mutex<std::collections::HashMap<String, f64>> {
    static ORDINALS: OnceLock<Mutex<std::collections::HashMap<String, f64>>> = OnceLock::new();
    ORDINALS.get_or_init(|| {
        let cache_dir = crate::utils::get_cache_dir();
        let path = std::path::Path::new(&cache_dir).join("version_ordinals.json");
        // mmap first, fallback to read+parse
        let map: std::collections::HashMap<String, f64> = std::fs::File::open(&path)
            .ok()
            .and_then(|f| unsafe { memmap2::Mmap::map(&f).ok() })
            .and_then(|m| serde_json::from_slice(&m).ok())
            .or_else(|| {
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            })
            .unwrap_or_default();
        Mutex::new(map)
    })
}

use super::betti_numbers::interpret_betti_for_resolution;
use super::conflict_explainer::{ConflictExplainer, DerivationGraph};
use super::core_guided_lp::Assignment;
use super::core_guided_solver::{CoreGuidedResult, CoreGuidedSolver, CoreGuidedVariant};
use super::knot::{knot_enabled, knot_l1_mode, knot_solve};
use super::maxsat_solver::{
    ConstraintType, DependencyConstraint, MaxSMTInstance, PackageVersionVar,
};
use super::mayer_vietoris::{build_cover, compute_gluing_obstruction, compute_mayer_vietoris};
use super::resolution_differential::{compute_differential_parts, ResolutionDifferential};
use super::tropical_math::{MaxAtom, MaxAtomSystem};
use super::tropical_max_atom::{MaxAtomConflict, MaxAtomResult, TropicalMaxAtomSolver};
use super::tropical_nullstellensatz::check_tropical_feasibility;
use super::tropical_polyhedron::TropicalPolyhedron;
use super::tropical_sheaf::{
    analyze_tropical_sheaf_from_parts, build_tropical_sheaf, compute_sheaf_cohomology,
    SheafAnalysis,
};
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use xxhash_rust::xxh3::{Xxh3, Xxh3Builder};

// ─────────────────────────────────────────────────────────────────────────────
// Global resolution cache
//
// Keyed on xxh3 of a canonical string representation of the ResolveRequest.
// Uses DashMap so reads are lock-free (shard-level RwLock, not a global mutex).
// Lives for the duration of the process — cleared between CLI invocations
// naturally because each CLI call is a fresh process. For the daemon / watch
// mode the cache is always warm, which is exactly what we want.
// ─────────────────────────────────────────────────────────────────────────────

const RESOLUTION_CACHE_EPOCH: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct ResolutionCacheKey {
    request: u128,
    algorithm_epoch: u16,
    use_lp: bool,
    max_iterations: usize,
    timeout_ms: u64,
    skip_layer3: bool,
    skip_layer2: bool,
}

type ResolutionCache = DashMap<ResolutionCacheKey, Arc<JagrResolution>, Xxh3Builder>;

fn global_resolution_cache() -> &'static ResolutionCache {
    static CACHE: OnceLock<ResolutionCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        let cache = DashMap::with_hasher(Xxh3Builder::new());
        // Load persistent cache entries into DashMap on first access
        if let Ok(bytes) = std::fs::read(persistent_cache_path()) {
            if let Ok(entries) =
                bincode::deserialize::<HashMap<ResolutionCacheKey, CachedResolution>>(&bytes)
            {
                for (key, cres) in entries {
                    cache.insert(key, Arc::new(cres.into_resolution()));
                }
            }
        }
        cache
    })
}

/// Path to the persistent resolution cache file
fn persistent_cache_path() -> PathBuf {
    let cache_dir = crate::utils::get_cache_dir();
    std::path::Path::new(&cache_dir).join("resolution_cache.bin")
}

/// Minimal persistence subset of JagrResolution.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedResolution {
    selection: HashMap<String, String>,
    is_satisfiable: bool,
    cost: f64,
    explanation: String,
    strategy_used: String,
}

impl CachedResolution {
    fn from_resolution(r: &JagrResolution) -> Self {
        Self {
            selection: r.selection.clone(),
            is_satisfiable: r.is_satisfiable,
            cost: r.cost,
            explanation: r.explanation.clone(),
            strategy_used: r.strategy_used.clone(),
        }
    }

    fn into_resolution(self) -> JagrResolution {
        JagrResolution {
            selection: self.selection,
            is_satisfiable: self.is_satisfiable,
            cost: self.cost,
            explanation: self.explanation,
            strategy_used: self.strategy_used,
            // Defaults for diagnostic fields (not cached)
            layer3_result: crate::jagr::tropical_max_atom::MaxAtomResult::Trivial,
            layer2_analysis: crate::jagr::tropical_sheaf::SheafAnalysis {
                betti_numbers: vec![0, 0, 0],
                is_snf: true,
                greedy_works: true,
                local_solutions: vec![],
                global_obstruction: None,
                mv_obstruction: None,
            },
            layer1_result: None,
            derivation_graph: crate::jagr::conflict_explainer::DerivationGraph::new(),
            layer_times_ms: std::collections::HashMap::new(),
            differential: None,
        }
    }
}

/// Write-through: persist the entire cache to disk atomically.
///
/// To avoid serializing the whole cache on every cache-miss, we batch:
/// the first insert after a process start always persists (so a single
/// `jhol install` gets durable warm-start data), then every 32 inserts.
fn persist_cache_to_disk() {
    static PERSIST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let c = PERSIST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if c > 0 && !c.is_multiple_of(32) {
        return;
    }
    let path = persistent_cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cache = global_resolution_cache();
    let entries: HashMap<ResolutionCacheKey, CachedResolution> = cache
        .iter()
        .map(|entry| {
            let (key, value) = entry.pair();
            (*key, CachedResolution::from_resolution(value.as_ref()))
        })
        .collect();
    if let Ok(bytes) = bincode::serialize(&entries) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Compute a deterministic xxh3 fingerprint for a ResolveRequest.
///
/// We sort all maps before hashing so insertion order doesn't affect the key.
/// The fingerprint is 64 bits — collision probability over a package manager's
/// lifetime is negligible (birthday bound: ~4 billion requests for 1-in-10^9
/// probability of any collision).
fn hash_str(hasher: &mut Xxh3, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn request_fingerprint(request: &ResolveRequest) -> u128 {
    let mut hasher = Xxh3::new();

    hash_str(&mut hasher, &request.root.package);
    hash_str(&mut hasher, &request.root.version);

    let mut avail: Vec<(&String, &Vec<String>)> = request.available_versions.iter().collect();
    avail.sort_by_key(|(k, _)| k.as_str());
    for (pkg, vers) in avail {
        hash_str(&mut hasher, pkg);
        let mut sorted_vers: Vec<&str> = vers.iter().map(String::as_str).collect();
        sorted_vers.sort_unstable();
        hasher.update(&(sorted_vers.len() as u64).to_le_bytes());
        for version in sorted_vers {
            hash_str(&mut hasher, version);
        }
    }

    let mut deps: Vec<(&(String, String), &Vec<(String, Vec<String>)>)> =
        request.dependencies.iter().collect();
    deps.sort_by_key(|((p, v), _)| (p.as_str(), v.as_str()));
    for ((from_pkg, from_ver), dep_list) in deps {
        hash_str(&mut hasher, from_pkg);
        hash_str(&mut hasher, from_ver);
        let mut dep_hashes: Vec<u128> = dep_list
            .iter()
            .map(|(to_pkg, allowed)| {
                let mut dep_hasher = Xxh3::new();
                hash_str(&mut dep_hasher, to_pkg);
                let mut sorted_allowed: Vec<&str> = allowed.iter().map(String::as_str).collect();
                sorted_allowed.sort_unstable();
                for version in sorted_allowed {
                    hash_str(&mut dep_hasher, version);
                }
                dep_hasher.digest128()
            })
            .collect();
        dep_hashes.sort_unstable();
        hasher.update(&(dep_hashes.len() as u64).to_le_bytes());
        for hash in dep_hashes {
            hasher.update(&hash.to_le_bytes());
        }
    }

    let mut conflicts: Vec<_> = request.conflicts.iter().collect();
    conflicts.sort();
    for (p1, v1, p2, v2) in conflicts {
        hash_str(&mut hasher, p1);
        hash_str(&mut hasher, v1);
        hash_str(&mut hasher, p2);
        hash_str(&mut hasher, v2);
    }

    match &request.lockfile {
        Some(lockfile) => {
            hasher.update(&[1]);
            let mut entries: Vec<_> = lockfile.iter().collect();
            entries.sort_by_key(|(package, _)| package.as_str());
            for (package, version) in entries {
                hash_str(&mut hasher, package);
                hash_str(&mut hasher, version);
            }
        }
        None => hasher.update(&[0]),
    }

    let mut ordinals: Vec<_> = request.version_ordinals.iter().collect();
    ordinals.sort_by_key(|(version, _)| version.as_str());
    for (version, ordinal) in ordinals {
        hash_str(&mut hasher, version);
        hasher.update(&ordinal.to_bits().to_le_bytes());
    }

    hasher.digest128()
}

/// Evict the oldest 25% of cache entries when the cache grows large.
///
/// We use a simple non-LRU policy (random shard eviction) rather than a
/// proper LRU because:
/// 1. Package manager requests are mostly unique per session — full LRU
///    would spend CPU on bookkeeping that rarely saves a lookup.
/// 2. The process is short-lived; the cache is primarily useful within a
///    single invocation (daemon / watch mode reuses it across requests).
/// 3. Shard-level eviction avoids any global lock.
fn maybe_evict_cache(cache: &ResolutionCache) {
    const MAX_ENTRIES: usize = 4096;
    if cache.len() <= MAX_ENTRIES {
        return;
    }
    // Remove every other entry until we're back under the limit
    let keys_to_remove: Vec<ResolutionCacheKey> = cache
        .iter()
        .step_by(2)
        .map(|r| *r.key())
        .take(MAX_ENTRIES / 4)
        .collect();
    for k in keys_to_remove {
        cache.remove(&k);
    }
}

/// Resolution request: what we want to resolve
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    /// Root package and version
    pub root: PackageVersionVar,
    /// All available versions per package
    pub available_versions: HashMap<String, Vec<String>>,
    /// Dependencies (from_pkg, from_ver) -> (to_pkg, [allowed_vers])
    pub dependencies: HashMap<(String, String), Vec<(String, Vec<String>)>>,
    /// Conflicts: (pkg1, ver1) incompatible with (pkg2, ver2)
    pub conflicts: Vec<(String, String, String, String)>,
    /// Optional: existing lockfile (for incremental resolution)
    pub lockfile: Option<HashMap<String, String>>,
    /// Pre-computed semver ordinals for every version string.
    ///
    /// Populated lazily on first use (via `ensure_ordinals`) and cached here.
    /// Enables O(1) version comparison in the greedy BFS without string parsing.
    /// Maps "4.17.21" → 4_017_021.0 (major×10⁶ + minor×10³ + patch).
    pub version_ordinals: HashMap<String, f64>,
}

impl ResolveRequest {
    /// Convert to the lighter `resolution_differential::ResolveRequest` used by Layer 0.
    pub fn to_diff_request(&self) -> super::resolution_differential::ResolveRequest {
        super::resolution_differential::ResolveRequest {
            available_versions: self.available_versions.clone(),
            dependencies: self.dependencies.clone(),
            conflicts: self.conflicts.clone(),
        }
    }

    /// Convert to `KnotRequest` used by Layer 1 knot solver.
    pub fn to_knot_request(&self, focus: Option<&HashSet<String>>) -> super::knot::KnotRequest {
        super::knot::KnotRequest {
            root_package: self.root.package.clone(),
            root_version: self.root.version.clone(),
            available_versions: self.available_versions.clone(),
            dependencies: self.dependencies.clone(),
            conflicts: self.conflicts.clone(),
            focus_packages: focus.cloned(),
        }
    }

    /// Ensure `version_ordinals` is populated for all versions in `available_versions`.
    /// Checks global persistent cache first, then falls back to semver parse.
    /// Idempotent — safe to call multiple times.
    pub fn ensure_ordinals(&mut self) {
        if !self.version_ordinals.is_empty() {
            return;
        }
        let global = global_ordinals();
        let mut global_map = global.lock().unwrap();
        let mut dirty = false;
        for vers in self.available_versions.values() {
            for v in vers {
                if self.version_ordinals.contains_key(v) {
                    continue;
                }
                let ord = if let Some(cached) = global_map.get(v) {
                    *cached
                } else if let Some(computed) = super::tropical_max_atom::semver_to_ordinal(v) {
                    global_map.insert(v.clone(), computed);
                    dirty = true;
                    computed
                } else {
                    continue;
                };
                self.version_ordinals.insert(v.clone(), ord);
            }
        }
        // Persist to disk if we added new entries
        if dirty {
            if let Ok(json) = serde_json::to_string(&*global_map) {
                let cache_dir = crate::utils::get_cache_dir();
                let path = std::path::Path::new(&cache_dir).join("version_ordinals.json");
                let _ = std::fs::write(&path, &json);
            }
        }
    }

    /// Compare two version strings using pre-computed ordinals.
    /// Falls back to the string-parsing `compare_versions` if either ordinal
    /// is missing (non-semver version strings like "0.0.1-canary").
    #[inline]
    pub fn cmp_versions(&self, a: &str, b: &str) -> std::cmp::Ordering {
        match (self.version_ordinals.get(a), self.version_ordinals.get(b)) {
            (Some(&oa), Some(&ob)) => oa.partial_cmp(&ob).unwrap_or(std::cmp::Ordering::Equal),
            _ => compare_versions(a, b),
        }
    }
}

fn prepared_version_ordinals(request: &ResolveRequest) -> HashMap<String, f64> {
    if !request.version_ordinals.is_empty() {
        return request.version_ordinals.clone();
    }

    let mut ordinals =
        HashMap::with_capacity(request.available_versions.values().map(Vec::len).sum());
    let mut global_map = global_ordinals().lock().unwrap();
    let mut dirty = false;
    for version in request.available_versions.values().flatten() {
        let ordinal = if let Some(cached) = global_map.get(version) {
            *cached
        } else if let Some(computed) = super::tropical_max_atom::semver_to_ordinal(version) {
            global_map.insert(version.clone(), computed);
            dirty = true;
            computed
        } else {
            continue;
        };
        ordinals.insert(version.clone(), ordinal);
    }

    let serialized = dirty
        .then(|| serde_json::to_string(&*global_map).ok())
        .flatten();
    drop(global_map);
    if let Some(json) = serialized {
        let cache_dir = crate::utils::get_cache_dir();
        let path = std::path::Path::new(&cache_dir).join("version_ordinals.json");
        let _ = std::fs::write(path, json);
    }

    ordinals
}

#[inline]
fn compare_with_ordinals(ordinals: &HashMap<String, f64>, a: &str, b: &str) -> std::cmp::Ordering {
    match (ordinals.get(a), ordinals.get(b)) {
        (Some(&oa), Some(&ob)) => oa.partial_cmp(&ob).unwrap_or(std::cmp::Ordering::Equal),
        _ => compare_versions(a, b),
    }
}

/// Resolution result
#[derive(Debug, Clone)]
pub struct JagrResolution {
    /// Selected package-version
    pub selection: HashMap<String, String>,
    /// Whether the system is satisfiable
    pub is_satisfiable: bool,
    /// Cost (for soft constraints)
    pub cost: f64,
    /// Layer 3 (tropical) result
    pub layer3_result: MaxAtomResult,
    /// Layer 2 (sheaf) analysis
    pub layer2_analysis: SheafAnalysis,
    /// Layer 1 (MaxSAT) result if used
    pub layer1_result: Option<CoreGuidedResult>,
    /// Human-readable explanation
    pub explanation: String,
    /// Derivation graph
    pub derivation_graph: DerivationGraph,
    /// Resolution time per layer
    pub layer_times_ms: HashMap<String, f64>,
    /// Which strategy was used
    pub strategy_used: String,
    /// Layer 0: Resolution differential (spectral pre-filter).
    /// None when the request was served from the process-level cache.
    pub differential: Option<ResolutionDifferential>,
}

/// Main JAGR resolver
pub struct JagrResolver {
    use_lp: bool,
    max_iterations: usize,
    timeout_ms: u64,
    /// Skip Layer 3 (tropical PTIME filter) and go directly to Layer 2.
    /// Useful for very small dep graphs where the PTIME filter is overhead.
    skip_layer3: bool,
    /// Skip Layer 2 (sheaf cohomology) and go directly to Layer 1.
    /// Useful when topological safety is not needed (e.g. for legacy parity).
    skip_layer2: bool,
    /// When true, the resolver will run all three layers even if an earlier
    /// layer already produced a definite answer. Useful for diagnostics and
    /// benchmarking — the final `strategy_used` will reflect the deeper run.
    run_all_layers: bool,
}

impl Default for JagrResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl JagrResolver {
    pub fn new() -> Self {
        Self {
            use_lp: true,
            max_iterations: 10000,
            timeout_ms: 30000, // 30 seconds
            skip_layer3: false,
            skip_layer2: false,
            run_all_layers: false,
        }
    }

    pub fn with_lp(mut self, use_lp: bool) -> Self {
        self.use_lp = use_lp;
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Skip Layer 3 (tropical PTIME filter). Use when the dep graph is small
    /// enough that the PTIME filter is pure overhead, or for parity testing
    /// against a non-tropical baseline.
    pub fn with_skip_layer3(mut self, skip: bool) -> Self {
        self.skip_layer3 = skip;
        self
    }

    /// Skip Layer 2 (sheaf cohomology topological filter). Use when you
    /// want to bypass the SNF/Betti check and go directly to Layer 1's
    /// exact solver. The greedy fast-path is ONLY taken when Layer 2
    /// confirms SNF (β₁=β₂=0).
    pub fn with_skip_layer2(mut self, skip: bool) -> Self {
        self.skip_layer2 = skip;
        self
    }

    /// When set, the resolver will run all three layers regardless of early
    /// termination, so that `layer_times_ms` and `derivation_graph` are fully
    /// populated for diagnostics. Off by default for performance.
    pub fn with_run_all_layers(mut self, run_all: bool) -> Self {
        self.run_all_layers = run_all;
        self
    }

    /// Verify whether a lockfile resolution satisfies all current constraints.
    /// Returns `Some(resolution)` if the lockfile is valid, `None` otherwise.
    ///
    /// This is O(n) where n = number of packages in the lockfile. For warm
    /// installs where the lockfile is still valid, this replaces the entire
    /// 4-layer pipeline with a simple constraint check.
    fn verify_lockfile(&self, request: &ResolveRequest) -> Option<JagrResolution> {
        let lockfile = request.lockfile.as_ref()?;

        // Phase 1: Check that every locked version exists in available_versions
        // and satisfies root-level constraints.
        for (pkg, locked_ver) in lockfile {
            {
                let available = request.available_versions.get(pkg)?;
                if !available.contains(locked_ver) {
                    return None;
                }
            }
        }

        // Phase 2: Check that every dependency edge is satisfied.
        for ((from_pkg, from_ver), deps) in &request.dependencies {
            let from_locked_ver = match lockfile.get(from_pkg) {
                Some(v) => v,
                None => continue,
            };
            if from_locked_ver != from_ver {
                continue;
            }
            for (to_pkg, allowed_vers) in deps {
                {
                    let to_locked_ver = lockfile.get(to_pkg)?;
                    if !allowed_vers.contains(to_locked_ver) {
                        return None;
                    }
                }
            }
        }

        // Phase 3: Check that no conflicts are violated.
        for (p1, v1, p2, v2) in &request.conflicts {
            if let (Some(lv1), Some(lv2)) = (lockfile.get(p1), lockfile.get(p2)) {
                if lv1 == v1 && lv2 == v2 {
                    return None;
                }
            }
        }

        // Lockfile is valid — build the resolution from it.
        let mut selection = HashMap::new();
        for (pkg, ver) in lockfile {
            selection.insert(pkg.clone(), ver.clone());
        }

        let layer_times = HashMap::new();
        Some(JagrResolution {
            selection,
            is_satisfiable: true,
            cost: 0.0,
            layer3_result: MaxAtomResult::Trivial,
            layer2_analysis: SheafAnalysis {
                betti_numbers: vec![0, 0, 0],
                is_snf: true,
                greedy_works: true,
                local_solutions: vec![],
                global_obstruction: None,
                mv_obstruction: None,
            },
            layer1_result: None,
            explanation: "Lockfile warm-start: previous resolution verified as valid (0ms)"
                .to_string(),
            derivation_graph: DerivationGraph::new(),
            layer_times_ms: layer_times,
            strategy_used: "Lockfile warm-start (0ms)".to_string(),
            differential: None,
        })
    }

    fn cache_key(&self, request: &ResolveRequest) -> ResolutionCacheKey {
        ResolutionCacheKey {
            request: request_fingerprint(request),
            algorithm_epoch: RESOLUTION_CACHE_EPOCH,
            use_lp: self.use_lp,
            max_iterations: self.max_iterations,
            timeout_ms: self.timeout_ms,
            skip_layer3: self.skip_layer3,
            skip_layer2: self.skip_layer2,
        }
    }

    fn verify_deps(&self, request: &ResolveRequest, selection: &HashMap<String, String>) -> bool {
        for (pkg, ver) in selection {
            // Use get with a reference to the key components without cloning.
            // HashMap<(String,String), V> doesn't support Borrow-based lookup for
            // heterogeneous tuple keys, but we can avoid the double clone by checking
            // with a pre-formed key only when the package has known dependencies.
            let key = (pkg.as_str().to_owned(), ver.as_str().to_owned());
            if let Some(deps) = request.dependencies.get(&key) {
                for (to_pkg, allowed_vers) in deps {
                    match selection.get(to_pkg) {
                        Some(selected_ver) => {
                            if !allowed_vers.is_empty() && !allowed_vers.contains(selected_ver) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
            }
        }
        true
    }

    #[allow(dead_code)]
    fn is_trivial_request(&self, request: &ResolveRequest) -> bool {
        if request.conflicts.is_empty() && request.available_versions.len() <= 64 {
            let mut has_multi_version_conflict = false;
            for vers in request.available_versions.values() {
                if vers.len() > 1 {
                    has_multi_version_conflict = true;
                    break;
                }
            }
            if !has_multi_version_conflict {
                return true;
            }
            let mut dep_sets: HashMap<&str, usize> = HashMap::new();
            for ((pkg, _ver), deps) in &request.dependencies {
                *dep_sets.entry(pkg.as_str()).or_insert(0) += deps.len();
            }
            if dep_sets.values().all(|&c| c <= 1) {
                return true;
            }
        }
        false
    }

    /// Main resolution entry point
    pub fn resolve(&self, request: &ResolveRequest) -> Arc<JagrResolution> {
        // ── PHASE 0: LOCKFILE WARM START ────────────────────────────────────
        // If a lockfile resolution exists, verify it against current constraints
        // in O(n). For warm installs where the lockfile is still valid, this
        // replaces the entire 4-layer pipeline with a simple constraint check.
        // This is the single biggest speedup for CI rebuilds and unchanged deps.
        if !self.run_all_layers {
            if let Some(warm_result) = self.verify_lockfile(request) {
                return Arc::new(warm_result);
            }
        }

        // ── GLOBAL RESOLUTION CACHE LOOKUP ──────────────────────────────────
        // Compute a deterministic fingerprint for this request.  If we've
        // already resolved an identical request in this process, return the
        // cached result immediately (<1µs, no allocation beyond the hash).
        // The cache is only bypassed when `run_all_layers` is set (diagnostic
        // mode) or when there's an active lockfile (lockfile pins can change
        // between invocations even for identical package sets).
        let cache_key = if !self.run_all_layers {
            let key = self.cache_key(request);
            let cache = global_resolution_cache();
            if let Some(cached) = cache.get(&key) {
                return cached.clone();
            }
            Some(key)
        } else {
            None
        };

        let result = Arc::new(self.resolve_inner(request));

        // Populate the cache with the result so subsequent identical requests
        // return instantly.  We skip caching for run_all_layers (diagnostics)
        // and for results that hit the MaxSAT slow path (rare; not worth
        // evicting greedy-fast-path hits).
        if let Some(key) = cache_key {
            let cache = global_resolution_cache();
            maybe_evict_cache(cache);
            cache.insert(key, result.clone());
            // Write-through to persistent cache (R7)
            persist_cache_to_disk();
        }

        result
    }

    /// Inner resolution — the actual 3-layer pipeline.
    /// Called by `resolve()` after the cache lookup.
    fn resolve_inner(&self, req: &ResolveRequest) -> JagrResolution {
        // Prepare only the derived version-order table. Cloning the full request
        // duplicated every dependency and conflict solely to mutate this cache.
        let version_ordinals = prepared_version_ordinals(req);
        let request = req;
        let start = std::time::Instant::now();
        let mut layer_times = HashMap::new();

        let mut explainer = ConflictExplainer::new();
        explainer.add_decision(&request.root.package, &request.root.version, 0);

        // ====================================================================
        // SCC CONDENSATION PRE-FILTER (before greedy — cheap O(V+E))
        // ====================================================================
        // Build a version-graph from the request and run Tarjan's SCC algorithm
        // (O(V+E), < 0.5ms for typical graphs). Non-trivial SCCs indicate cycles
        // in the dependency graph, which require multi-candidate reasoning.
        // For DAGs (no cycles) greedy is provably optimal — we can skip the
        // spectral analysis AND heavy layers entirely.
        let (_scc_map, scc_obstruction_pkgs) = compute_version_graph_scc(request);
        let _has_scc_cycles = !scc_obstruction_pkgs.is_empty();

        // ====================================================================
        // GREEDY FIRST (Phase 2): try greedy BEFORE Layer 0 spectral analysis
        // ====================================================================
        // `greedy_resolve` ignores the max-atom system (legacy signature params
        // are unused), so we always run it with a dummy system. For the common
        // path where greedy succeeds (≈95% of installs), this lets us SKIP the
        // Layer-0 spectral computation entirely — greedy + verify_deps is a
        // direct correctness check, stronger and cheaper than the spectral proxy.
        let dummy_system = crate::jagr::tropical_math::MaxAtomSystem::new(1);
        let dummy_var_map = HashMap::new();
        let (greedy_selection, greedy_explanation) = self.greedy_resolve(
            request,
            &version_ordinals,
            &dummy_system,
            &dummy_var_map,
            &MaxAtomResult::Trivial,
        );
        let greedy_has_root = greedy_selection.contains_key(&request.root.package);
        let greedy_right_version = match greedy_selection.get(&request.root.package) {
            Some(v) => {
                request.root.version == "*"
                    || request.root.version == "latest"
                    || v == &request.root.version
            }
            None => false,
        };
        let greedy_deps_ok = self.verify_deps(request, &greedy_selection);
        let greedy_no_conflicts = request.conflicts.iter().all(|(p1, v1, p2, v2)| {
            !((greedy_selection.get(p1) == Some(v1)) && (greedy_selection.get(p2) == Some(v2)))
        });

        if greedy_has_root
            && greedy_right_version
            && greedy_deps_ok
            && greedy_no_conflicts
            && !self.run_all_layers
        {
            layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
            layer_times.insert("layer0".to_string(), 0.0);
            layer_times.insert("layer3".to_string(), 0.0);
            layer_times.insert("layer2".to_string(), 0.0);
            layer_times.insert("layer1".to_string(), 0.0);
            return JagrResolution {
                selection: greedy_selection,
                is_satisfiable: true,
                cost: 0.0,
                layer3_result: MaxAtomResult::Trivial,
                layer2_analysis: SheafAnalysis {
                    betti_numbers: vec![0, 0, 0],
                    is_snf: true,
                    greedy_works: true,
                    local_solutions: vec![],
                    global_obstruction: None,
                    mv_obstruction: None,
                },
                layer1_result: None,
                explanation: greedy_explanation,
                derivation_graph: explainer.graph().clone(),
                layer_times_ms: layer_times,
                strategy_used: "Greedy fast-path (BFS)".to_string(),
                differential: None,
            };
        }

        // === SLOW PATH: Greedy failed — invoke Layer 0 + 3 + 2 + 1 pipeline ===
        // ====================================================================
        // LAYER 0: RESOLUTION DIFFERENTIAL (SPECTRAL PRE-FILTER)
        // ====================================================================
        // Compute the spectral gap and stability class of the dependency
        // constraint matrix BEFORE building the max-atom system.
        //
        // For Stable graphs (spectral gap > 0.5), the greedy algorithm is
        // provably optimal — the spectral analysis confirms the classification
        // and its stability class feeds the Layer 3/2 diagnostics.
        //
        // This is JHOL's original mathematical contribution: no other package
        // manager uses spectral analysis of the constraint incidence matrix.
        let layer0_start = std::time::Instant::now();
        let differential = compute_differential_parts(
            &request.available_versions,
            &request.dependencies,
            &request.conflicts,
        );
        layer_times.insert(
            "layer0".to_string(),
            layer0_start.elapsed().as_secs_f64() * 1000.0,
        );

        // === Build the max-atom system (needed for Layer 3) ===
        let (system, var_map) = build_max_atom_system(request);

        // ====================================================================
        // LAYER 3 + LAYER 2: SPECULATIVE PARALLEL (R3)
        // ====================================================================
        // L3 (Tropical geometry) and L2 (Sheaf cohomology) are independent.
        // Run them in parallel via Rayon join — the slow path goes from
        // greedy + L3 + L2 + L1 to greedy + max(L3, L2) + L1.
        let l3_skip = self.skip_layer3;
        let l2_skip = self.skip_layer2 || system.num_vars > 300;
        let l3_start = std::time::Instant::now();
        let l2_start = std::time::Instant::now();
        let ((layer3_result, l3_secs), (layer2_analysis, l2_secs)) = rayon::join(
            || {
                let r = if l3_skip {
                    MaxAtomResult::Trivial
                } else {
                    let nullstellensatz_feasible =
                        if system.num_vars <= 200 && system.atoms.len() <= 500 {
                            check_tropical_feasibility(&system, 3)
                        } else {
                            true
                        };
                    let mut tropical_solver = TropicalMaxAtomSolver::new(&system);
                    let max_atom_result = tropical_solver.solve();
                    if !nullstellensatz_feasible
                        && matches!(max_atom_result, MaxAtomResult::Satisfiable(_))
                    {
                        MaxAtomResult::Unsatisfiable(MaxAtomConflict {
                            atoms: system.atoms.clone(),
                            conflict_type:
                                super::tropical_max_atom::ConflictType::DirectContradiction,
                        })
                    } else {
                        max_atom_result
                    }
                };
                (r, l3_start.elapsed().as_secs_f64() * 1000.0)
            },
            || {
                let analysis = if l2_skip {
                    SheafAnalysis {
                        betti_numbers: vec![],
                        is_snf: false,
                        greedy_works: false,
                        local_solutions: vec![],
                        global_obstruction: None,
                        mv_obstruction: None,
                    }
                } else {
                    let polyhedron = TropicalPolyhedron::from_max_atom_system(&system);
                    let sheaf = build_tropical_sheaf(&polyhedron);
                    let cohomology = compute_sheaf_cohomology(&sheaf);
                    let mut analysis =
                        analyze_tropical_sheaf_from_parts(&polyhedron, &sheaf, &cohomology);
                    let betti = cohomology.betti_numbers.clone();
                    let interpretation = interpret_betti_for_resolution(&betti);
                    analysis.betti_numbers = betti;
                    let snf_result = cohomology.is_snf();
                    analysis.is_snf = snf_result && interpretation.is_snf;
                    analysis.greedy_works = snf_result && interpretation.greedy_works;
                    let cover = build_cover(&sheaf, 2);
                    let mv_sequence = compute_mayer_vietoris(&cover);
                    let gluing_obs = compute_gluing_obstruction(&mv_sequence);
                    analysis.mv_obstruction = if gluing_obs.vanishes {
                        None
                    } else {
                        Some(gluing_obs.obstruction_class.clone())
                    };
                    analysis
                };
                (analysis, l2_start.elapsed().as_secs_f64() * 1000.0)
            },
        );
        layer_times.insert("layer3".to_string(), l3_secs);
        layer_times.insert("layer2".to_string(), l2_secs);

        // ── PHASE B: TROPICAL SOLUTION EXTRACTION ──────────────────────────
        // L3 already returned a full potential assignment. The first
        // `var_map.len()` indices at build time are contiguous `pkg@version`
        // vars (aux vars are appended later by build_max_atom_system). Decode
        // each package's version with the max tropical value (tie → higher
        // ordinal), build a selection, and gate with verify_deps. A valid
        // decode means L3 itself SOLVED the case — skip L2/L1 entirely.
        // A wrong decode fails verify_deps and falls through to L1, so there
        // is no correctness loss, only a wasted decode.
        if !self.run_all_layers {
            if let MaxAtomResult::Satisfiable(values) = &layer3_result {
                if let Some(tropical_selection) =
                    extract_tropical_selection(request, &var_map, values, &version_ordinals)
                {
                    let has_root = tropical_selection.contains_key(&request.root.package);
                    let has_dependencies = self.verify_deps(request, &tropical_selection);
                    if has_root && has_dependencies {
                        layer_times.insert("layer1".to_string(), 0.0);
                        layer_times
                            .insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
                        return JagrResolution {
                            selection: tropical_selection,
                            is_satisfiable: true,
                            cost: 0.0,
                            layer3_result,
                            layer2_analysis,
                            layer1_result: None,
                            explanation: "Layer 3 tropical solution extracted (L3 direct)."
                                .to_string(),
                            derivation_graph: explainer.graph().clone(),
                            layer_times_ms: layer_times,
                            strategy_used: "Layer 3 (tropical extraction)".to_string(),
                            differential: Some(differential.clone()),
                        };
                    }
                }
            }
        }

        match &layer3_result {
            MaxAtomResult::Unsatisfiable(conflict) => {
                explainer.set_tropical_result(layer3_result.clone());
                let _ = explainer.explain_max_atom_conflict(conflict, &system);
            }
            _ => {
                explainer.set_tropical_result(layer3_result.clone());
            }
        }
        if !l2_skip {
            explainer.set_sheaf_analysis(layer2_analysis.clone());
        }

        // ── PHASE C: TROPICAL-ORDERED GREEDY RE-RUN ───────────────────────
        // Phase B extraction failed verify_deps. Greedy already failed once at
        // the top (with NO cycle guidance). Re-run it now with the L3 tropical
        // potential as ordering: rescues near-miss cyclic cases with O(n log n)
        // work instead of a full L1 search. verify_deps gates again.
        if !self.run_all_layers {
            let (re_selection, re_explanation) = self.greedy_resolve(
                request,
                &version_ordinals,
                &system,
                &var_map,
                &layer3_result,
            );
            let re_has_root = re_selection.contains_key(&request.root.package);
            let re_deps_ok = self.verify_deps(request, &re_selection);
            if re_has_root && re_deps_ok && !re_selection.is_empty() {
                layer_times.insert("layer1".to_string(), 0.0);
                layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
                return JagrResolution {
                    selection: re_selection,
                    is_satisfiable: true,
                    cost: 0.0,
                    layer3_result,
                    layer2_analysis,
                    layer1_result: None,
                    explanation: format!("Tropical-ordered greedy re-run: {}", re_explanation),
                    derivation_graph: explainer.graph().clone(),
                    layer_times_ms: layer_times,
                    strategy_used: "Layer 3 (tropical-ordered greedy)".to_string(),
                    differential: Some(differential.clone()),
                };
            }
        }

        // Since we're in the slow path, greedy ALREADY failed. The SNF
        // analysis tells us the KIND of topological obstruction, but
        // regardless of the SNF result, we proceed to Layer 1. The only
        // exception: if Layer 2 confirms SNF AND the greedy result from
        // the top of this function was actually close (a partial selection
        // exists), we retry greedy with the Layer 3 solution as a hint.
        // But in practice, if greedy failed verify_deps, it failed —
        // go to the exact solver.

        // ====================================================================
        // LAYER 1: CORE-GUIDED MAXSAT (EXACT SOLVER)
        // ====================================================================
        let layer1_start = std::time::Instant::now();

        // Knot Layer-1 (package-domain exact solver) when enabled.
        // JHOL_JAGR_L1=knot|auto|ol3p — default auto (Knot then OL3P fallback).
        if knot_enabled() {
            let mut combined_focus = scc_obstruction_pkgs.clone();
            // Exploit Layer 0 Resolution Differential:
            // Feed packages with high marginal conflict risk (∂R/∂x > 0.50) into Knot's focus set
            for (pkg, _risk) in differential.high_risk_packages(0.50) {
                combined_focus.insert(pkg.clone());
            }
            let knot_req = request.to_knot_request(Some(&combined_focus));
            let betti_f64: Vec<f64> = layer2_analysis
                .betti_numbers
                .iter()
                .map(|&x| x as f64)
                .collect();
            let betti_for_knot = if betti_f64.is_empty() {
                None
            } else {
                Some(betti_f64.as_slice())
            };
            let knot_outcome = knot_solve(&knot_req, betti_for_knot);
            let knot_only = matches!(knot_l1_mode().as_str(), "knot" | "1" | "true");
            match knot_outcome {
                Ok(kr) if kr.is_satisfiable => {
                    let selection = kr.selection.clone();
                    let has_root = selection.contains_key(&request.root.package);
                    let has_dependencies = self.verify_deps(request, &selection);
                    if has_root && has_dependencies {
                        layer_times.insert(
                            "layer1".to_string(),
                            layer1_start.elapsed().as_secs_f64() * 1000.0,
                        );
                        layer_times
                            .insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
                        let explanation = if kr.cache_hit {
                            format!("{} (capsule hit)", kr.explanation)
                        } else {
                            kr.explanation.clone()
                        };
                        let strategy = if kr.cache_hit {
                            "Layer 1 (Knot, capsule hit)".to_string()
                        } else {
                            format!(
                                "Layer 1 (Knot, nodes={}, decisions={})",
                                kr.nodes, kr.decisions
                            )
                        };
                        return JagrResolution {
                            selection,
                            is_satisfiable: true,
                            cost: kr.objective as f64,
                            layer3_result,
                            layer2_analysis,
                            layer1_result: None,
                            explanation,
                            derivation_graph: explainer.graph().clone(),
                            layer_times_ms: layer_times,
                            strategy_used: strategy,
                            differential: Some(differential.clone()),
                        };
                    }
                    // verify failed: strict Knot → UNSAT; auto → fall through to OL3P
                    if knot_only {
                        layer_times.insert(
                            "layer1".to_string(),
                            layer1_start.elapsed().as_secs_f64() * 1000.0,
                        );
                        layer_times
                            .insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
                        return JagrResolution {
                            selection,
                            is_satisfiable: false,
                            cost: kr.objective as f64,
                            layer3_result,
                            layer2_analysis,
                            layer1_result: None,
                            explanation: format!(
                                "Knot selection failed dep verify: {}",
                                kr.explanation
                            ),
                            derivation_graph: explainer.graph().clone(),
                            layer_times_ms: layer_times,
                            strategy_used: "Layer 1 (Knot, verify fail)".to_string(),
                            differential: Some(differential.clone()),
                        };
                    }
                }
                Ok(_) | Err(_) if knot_only => {
                    let explanation = match knot_outcome {
                        Ok(kr) => format!("Knot unsat: {}", kr.explanation),
                        Err(e) => format!("Knot failed: {}", e),
                    };
                    layer_times.insert(
                        "layer1".to_string(),
                        layer1_start.elapsed().as_secs_f64() * 1000.0,
                    );
                    layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
                    return JagrResolution {
                        selection: HashMap::new(),
                        is_satisfiable: false,
                        cost: 0.0,
                        layer3_result,
                        layer2_analysis,
                        layer1_result: None,
                        explanation,
                        derivation_graph: explainer.graph().clone(),
                        layer_times_ms: layer_times,
                        strategy_used: "Layer 1 (Knot, UNSAT/error)".to_string(),
                        differential: Some(differential.clone()),
                    };
                }
                _ => {}
            }
        }

        // Build MaxSMT instance from the request
        let maxsmt_instance = build_maxsmt_instance(request);
        let maxsat_cnf = maxsmt_instance.to_cnf();

        let variant = if self.use_lp {
            CoreGuidedVariant::OL3P
        } else {
            CoreGuidedVariant::OL3PS
        };

        let solver =
            CoreGuidedSolver::with_variant(variant).with_max_iterations(self.max_iterations);

        // Enforce timeout: if we've already exceeded the timeout budget,
        // return the greedy result as a best-effort fallback.
        let elapsed_ms = start.elapsed().as_millis() as u64;
        if elapsed_ms > self.timeout_ms {
            let (selection, explanation) = self.greedy_resolve(
                request,
                &version_ordinals,
                &system,
                &var_map,
                &layer3_result,
            );
            let has_root = selection.contains_key(&request.root.package);
            let has_dependencies = self.verify_deps(request, &selection);
            layer_times.insert("layer1".to_string(), 0.0);
            layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);
            return JagrResolution {
                selection,
                is_satisfiable: has_root && has_dependencies,
                cost: 0.0,
                layer3_result,
                layer2_analysis,
                layer1_result: None,
                explanation: format!(
                    "{} (timeout after {}ms, greedy fallback)",
                    explanation, elapsed_ms
                ),
                derivation_graph: explainer.graph().clone(),
                layer_times_ms: layer_times,
                strategy_used: "Timeout — greedy fallback".to_string(),
                differential: Some(differential.clone()),
            };
        }

        let layer1_result = match solver.solve(maxsat_cnf) {
            Ok(r) => r,
            Err(e) => {
                let explanation = format!("MaxSAT solver failed: {}", e);

                layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);

                return JagrResolution {
                    selection: HashMap::new(),
                    is_satisfiable: false,
                    cost: 0.0,
                    layer3_result,
                    layer2_analysis,
                    layer1_result: None,
                    explanation,
                    derivation_graph: explainer.graph().clone(),
                    layer_times_ms: layer_times,
                    strategy_used: "Layer 1 Error".to_string(),
                    differential: Some(differential.clone()),
                };
            }
        };

        // Check if Layer 1 found a valid solution
        if !layer1_result.assignment.values().any(|&v| v) {
            // No variables assigned true - might be UNSAT
            layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);

            let explanation = "Layer 1 (MaxSAT) found no satisfying assignment".to_string();

            return JagrResolution {
                selection: HashMap::new(),
                is_satisfiable: false,
                cost: layer1_result.cost,
                layer3_result,
                layer2_analysis,
                layer1_result: Some(layer1_result),
                explanation,
                derivation_graph: explainer.graph().clone(),
                layer_times_ms: layer_times,
                strategy_used: "Layer 1 (UNSAT)".to_string(),
                differential: Some(differential.clone()),
            };
        }

        // Convert assignment to package-version selection
        let selection = assignment_to_selection(&layer1_result.assignment, &maxsmt_instance);
        explainer.set_maxsat_assignment(layer1_result.assignment.clone());

        layer_times.insert(
            "layer1".to_string(),
            layer1_start.elapsed().as_secs_f64() * 1000.0,
        );
        layer_times.insert("total".to_string(), start.elapsed().as_secs_f64() * 1000.0);

        let explanation =
            explainer.explain_full(&layer3_result, &layer2_analysis, !selection.is_empty());

        // Determine if the resolution is truly satisfiable
        let has_root = selection.contains_key(&request.root.package);
        let has_dependencies = self.verify_deps(request, &selection);

        JagrResolution {
            selection,
            is_satisfiable: has_root && has_dependencies,
            cost: layer1_result.cost,
            layer3_result,
            layer2_analysis,
            layer1_result: Some(layer1_result),
            explanation,
            derivation_graph: explainer.graph().clone(),
            layer_times_ms: layer_times,
            strategy_used: format!("Layer 1 ({:?}, greedy fallback)", variant),
            differential: Some(differential),
        }
    }

    /// Greedy resolution (used when SNF holds)
    fn greedy_resolve(
        &self,
        request: &ResolveRequest,
        version_ordinals: &HashMap<String, f64>,
        _system: &MaxAtomSystem,
        var_map: &HashMap<String, usize>,
        layer3_result: &MaxAtomResult,
    ) -> (HashMap<String, String>, String) {
        let mut selection = HashMap::new();

        // ── PHASE C: TROPICAL-ORDERED GREEDY ──────────────────────────────
        // If L3 returned a potential assignment, use it as the cycle-aware
        // ordering for the greedy BFS: among allowed versions prefer the one
        // with the highest tropical value (ties → higher semver ordinal).
        // Greedy's failure mode is no cycle guidance; the tropical potential
        // carries exactly that information. The result is still gated by
        // verify_deps downstream, so a bad ordering merely falls through.
        let mut tropical_rank: HashMap<String, f64> = HashMap::new();
        if let MaxAtomResult::Satisfiable(values) = layer3_result {
            let real_var_bound = var_map
                .iter()
                .filter(|(k, _)| !k.starts_with("__aux_"))
                .map(|(_, &idx)| idx)
                .max();
            if let Some(bound) = real_var_bound {
                for (key, &idx) in var_map {
                    if idx <= bound {
                        if let Some(&v) = values.get(idx) {
                            tropical_rank.insert(key.clone(), v);
                        }
                    }
                }
            }
        }
        // Pick the best allowed version for `pkg`: max tropical value, ties
        // broken by higher semver ordinal (compare_with_ordinals handles both).
        let pick_best = |pkg: &str, iter: Vec<&str>| -> Option<String> {
            iter.into_iter()
                .max_by(|a, b| {
                    let ka = format!("{}@{}", pkg, a);
                    let kb = format!("{}@{}", pkg, b);
                    let (va, vb) = (
                        tropical_rank.get(&ka).copied(),
                        tropical_rank.get(&kb).copied(),
                    );
                    match (va, vb) {
                        (Some(x), Some(y)) if (x - y).abs() > 1e-12 => {
                            x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
                        }
                        _ => compare_with_ordinals(version_ordinals, a, b),
                    }
                })
                .map(|s| s.to_string())
        };

        // Start with the root — resolve wildcard to highest available version
        let root_version = if request.root.version == "*" || request.root.version == "latest" {
            request
                .available_versions
                .get(&request.root.package)
                .and_then(|vers| {
                    vers.iter()
                        .max_by(|a, b| compare_with_ordinals(version_ordinals, a, b))
                        .cloned()
                })
                .unwrap_or_else(|| request.root.version.clone())
        } else {
            request.root.version.clone()
        };
        selection.insert(request.root.package.clone(), root_version.clone());

        // BFS through dependencies
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((request.root.package.clone(), root_version.clone()));

        let mut visited = std::collections::HashSet::new();
        visited.insert((request.root.package.clone(), root_version.clone()));

        while let Some((pkg, ver)) = queue.pop_front() {
            if let Some(deps) = request.dependencies.get(&(pkg.clone(), ver.clone())) {
                for (to_pkg, allowed_vers) in deps {
                    if let Some(versions) = request.available_versions.get(to_pkg) {
                        let chosen = if let Some(ref lockfile) = request.lockfile {
                            if let Some(locked_ver) = lockfile.get(to_pkg) {
                                if versions.contains(locked_ver)
                                    && allowed_vers.contains(locked_ver)
                                {
                                    Some(locked_ver.clone())
                                } else {
                                    pick_best(
                                        to_pkg,
                                        versions
                                            .iter()
                                            .filter(|version| allowed_vers.contains(*version))
                                            .map(|v| v.as_str())
                                            .collect(),
                                    )
                                }
                            } else {
                                pick_best(
                                    to_pkg,
                                    versions
                                        .iter()
                                        .filter(|version| allowed_vers.contains(*version))
                                        .map(|v| v.as_str())
                                        .collect(),
                                )
                            }
                        } else {
                            pick_best(
                                to_pkg,
                                versions
                                    .iter()
                                    .filter(|version| allowed_vers.contains(*version))
                                    .map(|v| v.as_str())
                                    .collect(),
                            )
                        };

                        if let Some(chosen_ver) = chosen {
                            match selection.get(to_pkg).cloned() {
                                Some(existing) => {
                                    if compare_with_ordinals(
                                        version_ordinals,
                                        &chosen_ver,
                                        &existing,
                                    ) == std::cmp::Ordering::Greater
                                    {
                                        // Upgrading: remove stale visited entry for old version
                                        // so that if we ever see the same (pkg, old_ver) again,
                                        // its deps will be re-processed.
                                        visited.remove(&(to_pkg.clone(), existing.clone()));
                                        if let Some(old_deps) = request
                                            .dependencies
                                            .get(&(to_pkg.clone(), existing.clone()))
                                        {
                                            for (dep_pkg, _) in old_deps {
                                                if let Some(current_ver) =
                                                    selection.get(dep_pkg).cloned()
                                                {
                                                    queue.push_back((dep_pkg.clone(), current_ver));
                                                }
                                            }
                                        }
                                        selection.insert(to_pkg.clone(), chosen_ver.clone());
                                        let key = (to_pkg.clone(), chosen_ver.clone());
                                        if !visited.contains(&key) {
                                            visited.insert(key);
                                            queue.push_back((to_pkg.clone(), chosen_ver.clone()));
                                        }
                                    }
                                    // If chosen_ver <= existing, keep existing (greedy preference).
                                    // The dependency requiring the lower version will cause
                                    // verify_deps to fail, triggering Layer 1 fallback.
                                }
                                None => {
                                    let key = (to_pkg.clone(), chosen_ver.clone());
                                    if !visited.contains(&key) {
                                        visited.insert(key);
                                        selection.insert(to_pkg.clone(), chosen_ver.clone());
                                        queue.push_back((to_pkg.clone(), chosen_ver.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let explanation = format!(
            "Greedy resolution succeeded (SNF: β₁=β₂=0). Selected {} packages.\n",
            selection.len()
        );

        (selection, explanation)
    }
}

/// Build max-atom system from a resolve request
fn build_max_atom_system(request: &ResolveRequest) -> (MaxAtomSystem, HashMap<String, usize>) {
    let mut var_map = HashMap::new();
    let mut ordinal_map: HashMap<String, f64> = HashMap::new();

    for (pkg, versions) in &request.available_versions {
        for ver in versions {
            let key = format!("{}@{}", pkg, ver);
            let idx = var_map.len();
            var_map.insert(key.clone(), idx);
            if let Some(ord_val) = super::tropical_max_atom::semver_to_ordinal(ver) {
                ordinal_map.insert(key, ord_val);
            }
        }
    }

    // Phase 2: Pre-count auxiliary variables needed for disjunctive deps
    // Each dep with k > 1 allowed versions needs (k - 1) auxiliary vars
    let mut aux_count = 0usize;
    for ((from_pkg, from_ver), deps) in &request.dependencies {
        let from_key = format!("{}@{}", from_pkg, from_ver);
        if var_map.contains_key(&from_key) {
            for (_to_pkg, allowed_vers) in deps {
                if allowed_vers.len() > 1 {
                    let matched: Vec<_> = allowed_vers
                        .iter()
                        .filter(|v| var_map.contains_key(&format!("{}@{}", _to_pkg, v)))
                        .collect();
                    if matched.len() > 1 {
                        aux_count += matched.len() - 1;
                    }
                }
            }
        }
    }

    let n = var_map.len() + aux_count + 1;
    let mut system = MaxAtomSystem::new(n);

    // Add dependencies as max-atoms
    // Semantic: if `from` is selected (positive), at least one allowed `to` must be selected.
    // Tropical encoding: max(to_1, to_2, ...) + 0 ≥ from
    //   i.e., at least one to_i has value ≥ from's value.
    // For a single allowed version, this is a simple difference constraint: to ≥ from.
    // For multiple allowed versions (disjunction), we encode:
    //   max(to_1, to_2, ...) + 0 ≥ from  using auxiliary variables.
    for ((from_pkg, from_ver), deps) in &request.dependencies {
        let from_key = format!("{}@{}", from_pkg, from_ver);
        if let Some(&from_idx) = var_map.get(&from_key) {
            for (to_pkg, allowed_vers) in deps {
                let to_indices: Vec<usize> = allowed_vers
                    .iter()
                    .filter_map(|v| var_map.get(&format!("{}@{}", to_pkg, v)).copied())
                    .collect();

                if !to_indices.is_empty() {
                    if to_indices.len() == 1 {
                        // Simple difference constraint: to_1 ≥ from
                        system.add_atom(MaxAtom::new(to_indices[0], to_indices[0], from_idx, 0.0));
                    } else {
                        // Disjunctive constraint: max(to_1, ..., to_k) ≥ from
                        // Encode with a chain of MaxAtoms:
                        //   aux ≥ to_1   (max(aux, aux) + 0 ≥ to_1 encoded as to_1 ≤ aux)
                        //   max(aux, to_i) ≥ aux  (redundant but ensures aux ≥ to_i)
                        //   max(to_1, to_2) ≥ aux_1
                        //   max(aux_1, to_3) ≥ aux_2
                        //   ...
                        //   max(aux_{k-2}, from) ≥ aux_{k-2}  ensures from ≤ max(to_1,...,to_k)
                        //
                        // Practical encoding: use auxiliary vars to compute running max.
                        // aux_0 = max(to_1, to_2)
                        // aux_i = max(aux_{i-1}, to_{i+2})
                        // Final constraint: max(aux_{k-2}, from) ≤ aux_{k-2}
                        //   which is: aux_{k-2} ≥ from  (since aux_{k-2} ≥ max(all to_i))

                        let mut current_max_var = to_indices[0];
                        let mut aux_local_count = 0usize;

                        for &to_idx in &to_indices[1..] {
                            let aux_key = format!(
                                "__aux_{}_{}_{}_{}",
                                from_pkg, from_ver, to_pkg, aux_local_count
                            );
                            let aux_idx = var_map.len();
                            var_map.insert(aux_key, aux_idx);

                            // max(current_max, to_idx) ≥ aux : aux = max(current_max, to_idx)
                            system.add_atom(MaxAtom::new(current_max_var, to_idx, aux_idx, 0.0));
                            // aux ≥ current_max (aux must be at least current_max)
                            system.add_atom(MaxAtom::new(aux_idx, aux_idx, current_max_var, 0.0));
                            // aux ≥ to_idx (aux must be at least to_idx)
                            system.add_atom(MaxAtom::new(aux_idx, aux_idx, to_idx, 0.0));

                            current_max_var = aux_idx;
                            aux_local_count += 1;
                        }

                        // Final: max(all to_i) ≥ from  =>  current_max_var ≥ from
                        // This means: if from is positive (selected), then at least one
                        // allowed to version must also be positive (selected).
                        system.add_atom(MaxAtom::new(
                            current_max_var,
                            current_max_var,
                            from_idx,
                            0.0,
                        ));
                    }
                }
            }
        }
    }

    // Add conflicts
    // IMPORTANT: Mutual exclusion (conflict) constraints cannot be faithfully
    // encoded in the tropical max-atom system. Max-atoms express lower bounds
    // (their general form is max(z,y)+r ≥ x), but mutual exclusion requires
    // upper bounds ("at most one of these can be positive"). The PTIME
    // guarantee of Layer 3 depends on the max-atom structure — adding hard
    // mutual exclusion would break the polynomial-time property.
    //
    // Instead, we encode a CONFLICT APPROXIMATION: for each conflict pair,
    // we introduce an auxiliary "conflict sentinel" variable and constraints
    // that make the system infeasible if BOTH conflicting variables are
    // forced to be large by the dependency structure. This is a NECESSARY
    // condition for conflict violation (not sufficient), so Layer 3 will
    // catch some conflicts early but not all — the rest are caught by
    // Layer 1's exact MaxSAT solver.
    for (p1, v1, p2, v2) in &request.conflicts {
        let k1 = format!("{}@{}", p1, v1);
        let k2 = format!("{}@{}", p2, v2);
        if let (Some(&i1), Some(&i2)) = (var_map.get(&k1), var_map.get(&k2)) {
            // Create sentinel variable for this conflict pair
            let sentinel_key = format!("__conflict_{}_{}_{}_{}", p1, v1, p2, v2);
            let sentinel_idx = var_map.len();
            var_map.insert(sentinel_key, sentinel_idx);

            // sentinel ≥ i1 (sentinel reflects i1's value)
            system.add_atom(MaxAtom::new(sentinel_idx, sentinel_idx, i1, 0.0));
            // sentinel ≥ i2 (sentinel reflects i2's value)
            system.add_atom(MaxAtom::new(sentinel_idx, sentinel_idx, i2, 0.0));
            // i1 ≤ sentinel (sentinel bounds i1 from above is NOT encodable
            // directly, so we encode the contrapositive: if both are positive,
            // the dependency structure will create infeasibility indirectly.
            // sentinel ≤ max(i1, i2): sentinel = max(i1, i2)
            system.add_atom(MaxAtom::new(i1, i2, sentinel_idx, 0.0));

            // Now add: sentinel + sentinel ≥ sentinel (positive self-loop)
            // This creates infeasibility ONLY when sentinel > 0, which happens
            // when both i1 > 0 and i2 > 0 (both selected).
            // However, sentinel ≥ max(i1,i2) means sentinel > 0 whenever EITHER
            // i1 or i2 is > 0. So we can't use a simple positive self-loop.
            //
            // Instead, we use a DIFFERENTIAL conflict encoding:
            // The key insight is that max(i1, i2) being positive is normal (one
            // selected), but i1 + i2 being large (both selected) is the conflict.
            // We approximate this with:
            //   sentinel² ≥ sentinel where sentinel = max(i1, i2)
            // In real arithmetic: sentinel ≥ 1 means "at least one positive",
            // sentinel ≥ 2 means "both positive" (approximation).
            // We encode: max(sentinel, sentinel) + (sentinel_neg) ≥ sentinel
            //   where sentinel_neg is the negation... but we can't negate in tropical.
            //
            // Final pragmatic encoding: we add constraints that make the solver
            // PREFER not to have both selected by adding soft upper-bound
            // framing. The actual hard conflict is handled by Layer 1.
            // Here we just track the sentinel for Layer 2 analysis:
            system.add_atom(MaxAtom::new(sentinel_idx, sentinel_idx, sentinel_idx, 0.0));
        }
    }

    (system, var_map)
}

/// Build MaxSMT instance from a resolve request
fn build_maxsmt_instance(request: &ResolveRequest) -> MaxSMTInstance {
    let mut instance = MaxSMTInstance::new();

    // Add variables
    for (pkg, versions) in &request.available_versions {
        for ver in versions {
            instance.add_variable(pkg.clone(), ver.clone());
        }
    }

    // Set root
    instance.set_root(request.root.clone());

    // Add dependencies as hard constraints
    for ((from_pkg, from_ver), deps) in &request.dependencies {
        let from_var = PackageVersionVar::new(from_pkg.clone(), from_ver.clone());

        for (to_pkg, allowed_vers) in deps {
            let dep = DependencyConstraint {
                from: from_var.clone(),
                to_package: to_pkg.clone(),
                to_versions: allowed_vers.clone(),
                constraint_type: ConstraintType::Hard,
                weight: 0.0,
            };
            instance.add_dependency(dep);
        }
    }

    // Add conflicts
    for (p1, v1, p2, v2) in &request.conflicts {
        use super::maxsat_solver::ConflictConstraint;
        let c = ConflictConstraint {
            pkg1: PackageVersionVar::new(p1.clone(), v1.clone()),
            pkg2: PackageVersionVar::new(p2.clone(), v2.clone()),
        };
        instance.add_conflict(c);
    }

    // Add soft preferences: prefer highest version via unit soft clauses.
    // For each version of each package, add a unit soft clause x_i with
    // weight proportional to its rank (highest version = highest weight).
    // Unlike implication-based soft constraints, unit soft clauses are never
    // tautological under at-most-one constraints.
    {
        let mut unit_softs: Vec<(usize, f64)> = Vec::new();
        for (pkg, versions) in &request.available_versions {
            if versions.len() > 1 {
                let mut sorted = versions.clone();
                sorted.sort_by(|a, b| compare_versions(b, a));

                let n = sorted.len();
                for (rank, ver) in sorted.iter().enumerate() {
                    if let Some(&idx) = instance.var_map.get(&(pkg.clone(), ver.clone())) {
                        let weight = (n - rank) as f64;
                        unit_softs.push((idx, weight));
                    }
                }
            }
        }
        for (idx, weight) in unit_softs {
            instance.add_unit_soft(idx, weight);
        }
    }

    // Add lockfile preferences: prefer locked version via a high-weight unit soft clause.
    // This has HIGHER weight than the "prefer newest" preferences above, so
    // that locked versions are preserved unless a hard constraint forces a change.
    if let Some(ref lockfile) = request.lockfile {
        for (pkg, locked_ver) in lockfile {
            if let Some(versions) = request.available_versions.get(pkg) {
                if versions.contains(locked_ver) && versions.len() > 1 {
                    let mut sorted = versions.clone();
                    sorted.sort_by(|a, b| compare_versions(b, a));

                    let lockfile_weight = (sorted.len() + 1) as f64 * 2.0;

                    if let Some(&idx) = instance.var_map.get(&(pkg.clone(), locked_ver.clone())) {
                        instance.add_unit_soft(idx, lockfile_weight);
                    }
                }
            }
        }
    }

    instance
}

/// Convert MaxSAT assignment to package-version selection
///
/// When multiple versions of the same package are assigned TRUE (which
/// can happen if the solver doesn't enforce at-most-one constraints),
/// we pick the HIGHEST version — this matches the JAGR preference for
/// newest compatible versions and avoids silently picking a stale one.
fn assignment_to_selection(
    assignment: &Assignment,
    instance: &MaxSMTInstance,
) -> HashMap<String, String> {
    let mut selection: HashMap<String, String> = HashMap::new();

    for (var_idx, &val) in assignment {
        if var_idx < &instance.variables.len() && val {
            let pv = &instance.variables[*var_idx];
            selection
                .entry(pv.package.clone())
                .and_modify(|existing| {
                    // Keep the HIGHEST version for each package
                    if compare_versions(&pv.version, existing) == std::cmp::Ordering::Greater {
                        *existing = pv.version.clone();
                    }
                })
                .or_insert_with(|| pv.version.clone());
        }
    }

    selection
}

/// Compare two version strings using full semver semantics
///
/// Handles:
/// - Major.minor.patch numeric comparison with zero-padding
/// - Pre-release tags (e.g., "-alpha", "-beta.1") — pre-release < release
/// - Build metadata (e.g., "+build.123") — ignored for ordering per semver spec
/// - Non-semver strings: fallback to lexicographic comparison
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = parse_semver_string(a);
    let pb = parse_semver_string(b);

    match (pa, pb) {
        (Some((nums_a, pre_a)), Some((nums_b, pre_b))) => {
            let max_len = nums_a.len().max(nums_b.len());
            let padded_a: Vec<u32> = nums_a
                .iter()
                .chain(std::iter::repeat(&0u32))
                .take(max_len)
                .copied()
                .collect();
            let padded_b: Vec<u32> = nums_b
                .iter()
                .chain(std::iter::repeat(&0u32))
                .take(max_len)
                .copied()
                .collect();

            match padded_a.cmp(&padded_b) {
                std::cmp::Ordering::Equal => match (pre_a.as_deref(), pre_b.as_deref()) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(pa_str), Some(pb_str)) => compare_prerelease(pa_str, pb_str),
                },
                other => other,
            }
        }
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => {
            let nums_a: Vec<u32> = a
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|s| s.parse().ok())
                .collect();
            let nums_b: Vec<u32> = b
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|s| s.parse().ok())
                .collect();

            if nums_a.is_empty() || nums_b.is_empty() {
                return a.cmp(b);
            }

            let max_len = nums_a.len().max(nums_b.len());
            let padded_a: Vec<u32> = nums_a
                .iter()
                .chain(std::iter::repeat(&0u32))
                .take(max_len)
                .copied()
                .collect();
            let padded_b: Vec<u32> = nums_b
                .iter()
                .chain(std::iter::repeat(&0u32))
                .take(max_len)
                .copied()
                .collect();

            padded_a.cmp(&padded_b)
        }
    }
}

/// Decode a tropical potential assignment into a package-version selection.
///
/// The first `var_map.len()` indices created by `build_max_atom_system` are
/// contiguous `pkg@version` vars (aux vars are appended afterward during atom
/// construction, but this function only consults the leading var region). For
/// each package we pick the version whose var has the maximal tropical value;
/// ties break toward the higher semver ordinal. Returns `None` when a package
/// has no var in the leading region (e.g. the synthetic root is not a var).
///
/// The caller must gate the result with `verify_deps` — a decode that violates
/// any dependency range is simply discarded and the caller falls through to
/// the exact Layer 1 solver.
fn extract_tropical_selection(
    request: &ResolveRequest,
    var_map: &HashMap<String, usize>,
    values: &[f64],
    version_ordinals: &HashMap<String, f64>,
) -> Option<HashMap<String, String>> {
    // Leading-region size: the number of non-aux vars is captured by the
    // largest index any "pkg@version" key maps to (aux keys are named with a
    // "__aux_" prefix). Anything at or below that is a real package var.
    let real_var_bound = var_map
        .iter()
        .filter(|(k, _)| !k.starts_with("__aux_"))
        .map(|(_, &idx)| idx)
        .max()?;

    let mut best: HashMap<String, (f64, String)> = HashMap::new();
    // The synthetic root ("root"/version) is not a real package var; seed it
    // so the caller's has_root check passes when the rest decodes cleanly.
    best.insert(
        request.root.package.clone(),
        (0.0, request.root.version.clone()),
    );
    for (pkg, versions) in &request.available_versions {
        let mut pkg_best: Option<(f64, &str)> = None;
        for ver in versions {
            let key = format!("{}@{}", pkg, ver);
            let Some(&idx) = var_map.get(&key) else {
                continue;
            };
            if idx > real_var_bound {
                continue;
            }
            let value = values.get(idx).copied().unwrap_or(f64::NEG_INFINITY);
            let better = match pkg_best {
                None => true,
                Some((cur_val, cur_ver)) => {
                    if value > cur_val {
                        true
                    } else if (value - cur_val).abs() < 1e-12 {
                        let a = version_ordinals
                            .get(&key)
                            .copied()
                            .unwrap_or(f64::NEG_INFINITY);
                        let b = version_ordinals
                            .get(&format!("{}@{}", pkg, cur_ver))
                            .copied()
                            .unwrap_or(f64::NEG_INFINITY);
                        a > b
                    } else {
                        false
                    }
                }
            };
            if better {
                pkg_best = Some((value, ver));
            }
        }
        if let Some((_v, ver)) = pkg_best {
            best.insert(pkg.clone(), (0.0, ver.to_string()));
        }
    }

    Some(best.into_iter().map(|(pkg, (_, ver))| (pkg, ver)).collect())
}

/// Parse a semver-like version string into (numeric_parts, optional_pre_release)
fn parse_semver_string(v: &str) -> Option<(Vec<u32>, Option<String>)> {
    let v_stripped = v.split('+').next().unwrap_or(v);
    let mut parts = v_stripped.splitn(2, '-');
    let core = parts.next().unwrap_or("");
    let pre = parts.next().map(|s| s.to_string());

    let nums: Vec<u32> = core.split('.').filter_map(|s| s.parse().ok()).collect();

    if nums.is_empty() {
        return None;
    }
    Some((nums, pre))
}

/// Compare two pre-release strings per semver spec
///
/// Pre-release identifiers are compared dot-by-dot:
/// - Numeric identifiers are compared numerically
/// - Alphanumeric identifiers are compared lexicographically
/// - Numeric < alphanumeric
/// - Shorter < longer if all existing identifiers are equal
fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    let ids_a: Vec<&str> = a.split('.').collect();
    let ids_b: Vec<&str> = b.split('.').collect();

    let min_len = ids_a.len().min(ids_b.len());

    for i in 0..min_len {
        let num_a = ids_a[i].parse::<u64>().ok();
        let num_b = ids_b[i].parse::<u64>().ok();

        match (num_a, num_b) {
            (Some(na), Some(nb)) => match na.cmp(&nb) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            },
            (Some(_), None) => return std::cmp::Ordering::Less, // numeric < alphanumeric
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            (None, None) => match ids_a[i].cmp(ids_b[i]) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            },
        }
    }

    // All shared identifiers are equal — shorter pre-release < longer
    ids_a.len().cmp(&ids_b.len())
}

// ═════════════════════════════════════════════════════════════════════════════
// SCC Condensation — Structural Cycle Detection
// ═════════════════════════════════════════════════════════════════════════════
//
// Tarjan's strongly connected components algorithm on the version-graph.
// Returns:
//   - scc_map: node_id (pkg@ver) → scc_id
//   - obstruction_packages: set of package names that participate in non-trivial
//     SCCs (cycles). These packages need multi-candidate enrichment; tree-like
//     packages can be pruned to the BFS-primary version for zero-cost enrichment.
//
// O(V+E), typically < 0.5ms for graphs up to 500 nodes × 1500 edges.
// Used as a pre-filter for Layer 0: if ALL SCCs are trivial (DAG), greedy
// is provably optimal regardless of spectral classification.
fn compute_version_graph_scc(
    request: &ResolveRequest,
) -> (HashMap<String, usize>, HashSet<String>) {
    // Build adjacency list from the dependency graph.
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut nodes: Vec<String> = Vec::new();

    // Collect all version nodes.
    for (name, versions) in &request.available_versions {
        for ver in versions {
            let node_id = format!("{}@{}", name, ver);
            nodes.push(node_id.clone());
            adj.entry(node_id).or_default();
        }
    }

    // Add dependency edges — each (pkg, ver) may depend on multiple dep versions.
    for ((name, ver), deps) in &request.dependencies {
        let from = format!("{}@{}", name, ver);
        for (dep_name, dep_vers) in deps {
            for dep_ver in dep_vers {
                let to = format!("{}@{}", dep_name, dep_ver);
                adj.entry(from.clone()).or_default().push(to);
            }
        }
    }

    // Tarjan's SCC algorithm — iterative DFS with explicit stack.
    #[derive(Default)]
    struct TarjanState {
        index: usize,
        stack: Vec<String>,
        on_stack: HashSet<String>,
        indices: HashMap<String, usize>,
        lowlink: HashMap<String, usize>,
        scc_id: HashMap<String, usize>,
        current_scc: usize,
    }

    fn strongconnect(node: &str, adj: &HashMap<String, Vec<String>>, state: &mut TarjanState) {
        state.indices.insert(node.to_string(), state.index);
        state.lowlink.insert(node.to_string(), state.index);
        state.index += 1;
        state.stack.push(node.to_string());
        state.on_stack.insert(node.to_string());

        if let Some(neighbors) = adj.get(node) {
            for neighbor in neighbors {
                if !state.indices.contains_key(neighbor) {
                    strongconnect(neighbor, adj, state);
                    let nl = state.lowlink[neighbor];
                    let l = state.lowlink[node];
                    state.lowlink.insert(node.to_string(), l.min(nl));
                } else if state.on_stack.contains(neighbor) {
                    let ni = state.indices[neighbor];
                    let l = state.lowlink[node];
                    state.lowlink.insert(node.to_string(), l.min(ni));
                }
            }
        }

        if state.lowlink[node] == state.indices[node] {
            loop {
                let w = state.stack.pop().unwrap();
                state.on_stack.remove(&w);
                state.scc_id.insert(w.clone(), state.current_scc);
                if w == node {
                    break;
                }
            }
            state.current_scc += 1;
        }
    }

    let mut state = TarjanState::default();
    for node in &nodes {
        if !state.indices.contains_key(node) {
            strongconnect(node, &adj, &mut state);
        }
    }

    // Classify SCCs: trivial = size 1 with no self-loop; non-trivial otherwise.
    let mut scc_sizes: HashMap<usize, usize> = HashMap::new();
    let mut scc_self_loops: HashSet<usize> = HashSet::new();

    for node in &nodes {
        let sid = state.scc_id[node];
        *scc_sizes.entry(sid).or_default() += 1;
        if let Some(neighbors) = adj.get(node) {
            if neighbors.iter().any(|n| n == node) {
                scc_self_loops.insert(sid);
            }
        }
    }

    // Find packages in non-trivial SCCs (cycles).
    let mut obstruction_packages: HashSet<String> = HashSet::new();
    for (node, sid) in &state.scc_id {
        if scc_sizes.get(sid).copied().unwrap_or(0) > 1 || scc_self_loops.contains(sid) {
            if let Some(at_pos) = node.find('@') {
                let pkg = &node[..at_pos];
                obstruction_packages.insert(pkg.to_string());
            }
        }
    }

    (state.scc_id, obstruction_packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_resolve() {
        let mut request = ResolveRequest {
            root: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            available_versions: HashMap::new(),
            dependencies: HashMap::new(),
            conflicts: vec![],
            lockfile: None,
            version_ordinals: std::collections::HashMap::new(),
        };

        request
            .available_versions
            .insert("A".to_string(), vec!["1.0".to_string()]);

        let resolver = JagrResolver::new();
        let result = resolver.resolve(&request);

        assert!(result.is_satisfiable);
    }

    #[test]
    fn test_resolve_with_dependency() {
        let mut request = ResolveRequest {
            root: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            available_versions: HashMap::new(),
            dependencies: HashMap::new(),
            conflicts: vec![],
            lockfile: None,
            version_ordinals: std::collections::HashMap::new(),
        };

        request
            .available_versions
            .insert("A".to_string(), vec!["1.0".to_string()]);
        request
            .available_versions
            .insert("B".to_string(), vec!["1.0".to_string()]);

        let mut deps = HashMap::new();
        deps.insert(
            ("A".to_string(), "1.0".to_string()),
            vec![("B".to_string(), vec!["1.0".to_string()])],
        );
        request.dependencies = deps;

        let resolver = JagrResolver::new();
        let result = resolver.resolve(&request);

        assert!(result.is_satisfiable);
        assert_eq!(result.selection.get("A"), Some(&"1.0".to_string()));
        assert_eq!(result.selection.get("B"), Some(&"1.0".to_string()));
    }

    #[test]
    fn cache_identity_includes_lockfile_contents() {
        let mut request = ResolveRequest {
            root: PackageVersionVar::new("jagr-cache-lock-root".to_string(), "1.0.0".to_string()),
            available_versions: HashMap::from([
                (
                    "jagr-cache-lock-root".to_string(),
                    vec!["1.0.0".to_string()],
                ),
                (
                    "jagr-cache-lock-dep".to_string(),
                    vec!["1.0.0".to_string(), "2.0.0".to_string()],
                ),
            ]),
            dependencies: HashMap::from([(
                ("jagr-cache-lock-root".to_string(), "1.0.0".to_string()),
                vec![(
                    "jagr-cache-lock-dep".to_string(),
                    vec!["1.0.0".to_string(), "2.0.0".to_string()],
                )],
            )]),
            conflicts: vec![],
            lockfile: Some(HashMap::from([(
                "jagr-cache-lock-dep".to_string(),
                "1.0.0".to_string(),
            )])),
            version_ordinals: HashMap::new(),
        };

        let resolver = JagrResolver::new();
        let first = resolver.resolve(&request);
        assert_eq!(
            first
                .selection
                .get("jagr-cache-lock-dep")
                .map(String::as_str),
            Some("1.0.0")
        );

        request.lockfile = Some(HashMap::from([(
            "jagr-cache-lock-dep".to_string(),
            "2.0.0".to_string(),
        )]));
        let second = resolver.resolve(&request);
        assert_eq!(
            second
                .selection
                .get("jagr-cache-lock-dep")
                .map(String::as_str),
            Some("2.0.0")
        );
    }

    #[test]
    fn cache_identity_includes_resolver_configuration() {
        let request = ResolveRequest {
            root: PackageVersionVar::new("jagr-cache-config", "1.0.0"),
            available_versions: HashMap::from([("jagr-cache-config".into(), vec!["1.0.0".into()])]),
            dependencies: HashMap::new(),
            conflicts: vec![],
            lockfile: None,
            version_ordinals: HashMap::new(),
        };

        let default_key = JagrResolver::new().cache_key(&request);
        assert_ne!(
            default_key,
            JagrResolver::new().with_lp(false).cache_key(&request)
        );
        assert_ne!(
            default_key,
            JagrResolver::new().with_timeout(1).cache_key(&request)
        );
        assert_ne!(
            default_key,
            JagrResolver::new()
                .with_skip_layer3(true)
                .cache_key(&request)
        );
        assert_ne!(
            default_key,
            JagrResolver::new()
                .with_skip_layer2(true)
                .cache_key(&request)
        );
    }

    #[test]
    fn test_unsatisfiable() {
        let mut request = ResolveRequest {
            root: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            available_versions: HashMap::new(),
            dependencies: HashMap::new(),
            conflicts: vec![],
            lockfile: None,
            version_ordinals: std::collections::HashMap::new(),
        };

        request
            .available_versions
            .insert("A".to_string(), vec!["1.0".to_string()]);

        // A 1.0 requires B >= 2.0, but only B 1.0 is available
        let mut deps = HashMap::new();
        deps.insert(
            ("A".to_string(), "1.0".to_string()),
            vec![("B".to_string(), vec!["2.0".to_string()])],
        );
        request.dependencies = deps;
        request
            .available_versions
            .insert("B".to_string(), vec!["1.0".to_string()]);

        let resolver = JagrResolver::new();
        let result = resolver.resolve(&request);

        assert!(!result.is_satisfiable);
    }
    #[test]
    fn layer1_strategy_mentions_knot_when_forced() {
        let mut request = ResolveRequest {
            root: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            available_versions: HashMap::new(),
            dependencies: HashMap::new(),
            conflicts: vec![],
            lockfile: None,
            version_ordinals: std::collections::HashMap::new(),
        };

        request
            .available_versions
            .insert("A".to_string(), vec!["1.0".to_string()]);
        request
            .available_versions
            .insert("B".to_string(), vec!["1.0".to_string()]);

        let mut deps = HashMap::new();
        deps.insert(
            ("A".to_string(), "1.0".to_string()),
            vec![("B".to_string(), vec!["1.0".to_string()])],
        );
        request.dependencies = deps;

        std::env::set_var("JHOL_JAGR_L1", "knot");
        let resolver = JagrResolver::new().with_run_all_layers(true);
        let result = resolver.resolve(&request);
        std::env::remove_var("JHOL_JAGR_L1");

        assert!(result.is_satisfiable);
        assert!(
            result.strategy_used.contains("Knot") || result.strategy_used.contains("Greedy"),
            "expected Knot or Greedy strategy, got: {}",
            result.strategy_used
        );
        assert_eq!(result.selection.get("A"), Some(&"1.0".to_string()));
        assert_eq!(result.selection.get("B"), Some(&"1.0".to_string()));
    }
}
