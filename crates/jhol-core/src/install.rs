use crate::backend::{self, Backend};
use crate::bin_links;
use crate::error_handling::JholError;
use crate::lockfile;
use crate::registry;
use crate::selective_extract; // JHOL Selective Extraction
use crate::utils::{self, NPM_SHOW_TIMEOUT_SECS};
use ahash::{HashMap as AHashMap, HashMapExt, HashSet as AHashSet, HashSetExt};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

fn download_concurrency() -> usize {
    std::env::var("JHOL_DOWNLOAD_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 128)) // Increased to 128 for maximum parallelism
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 4).clamp(32, 128)) // 4x cores, max 128
                .unwrap_or(64) // Default to 64 (was 8)
        })
}

fn cache_install_concurrency() -> usize {
    std::env::var("JHOL_CACHE_INSTALL_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 32))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(32)
                .max(1)
        })
}

fn worker_pool_sequential_threshold() -> usize {
    std::env::var("JHOL_WORKER_POOL_SEQUENTIAL_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 64))
        .unwrap_or(1)
}

fn use_legacy_chunk_scheduler() -> bool {
    std::env::var("JHOL_LEGACY_CHUNK_SCHEDULER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn run_worker_pool<I, O, F>(
    items: Vec<I>,
    concurrency: usize,
    sequential_threshold: usize,
    job: F,
) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    if items.is_empty() {
        return Vec::new();
    }

    // Fast path: for tiny batches, avoid thread spawn + lock/channel overhead.
    // This materially helps warm/offline installs where worksets are often very small.
    if concurrency <= 1 || items.len() <= sequential_threshold.clamp(1, 64) {
        return items.into_iter().map(job).collect();
    }

    let worker_count = concurrency.clamp(1, 64).min(items.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(items)));
    let job = Arc::new(job);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut handles = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let tx = tx.clone();
        let job = Arc::clone(&job);
        handles.push(std::thread::spawn(move || loop {
            let next = match queue.lock() {
                Ok(mut q) => q.pop_front(),
                Err(_) => None,
            };
            let Some(item) = next else {
                break;
            };

            let output = (job)(item);
            if tx.send(output).is_err() {
                break;
            }
        }));
    }

    drop(tx);

    let mut outputs = Vec::with_capacity(worker_count);
    for item in rx {
        outputs.push(item);
    }

    for handle in handles {
        let _ = handle.join();
    }

    outputs
}

fn transitive_cache_enabled() -> bool {
    std::env::var("JHOL_DAEMON_MODE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn transitive_memo_cache(
) -> &'static std::sync::Arc<std::sync::RwLock<HashMap<String, Vec<String>>>> {
    static CACHE: OnceLock<std::sync::Arc<std::sync::RwLock<HashMap<String, Vec<String>>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| std::sync::Arc::new(std::sync::RwLock::new(HashMap::new())))
}

fn transitive_metadata_snapshot(
) -> &'static std::sync::Arc<std::sync::RwLock<HashMap<String, serde_json::Value>>> {
    static CACHE: OnceLock<std::sync::Arc<std::sync::RwLock<HashMap<String, serde_json::Value>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| std::sync::Arc::new(std::sync::RwLock::new(HashMap::new())))
}

fn install_profile_enabled() -> bool {
    std::env::var("JHOL_PROFILE_INSTALL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

struct InstallProfiler {
    enabled: bool,
    start: Instant,
    last: Instant,
}

impl InstallProfiler {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            enabled: install_profile_enabled(),
            start: now,
            last: now,
        }
    }

    fn mark(&mut self, stage: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let delta = now.duration_since(self.last).as_millis();
        let total = now.duration_since(self.start).as_millis();
        crate::utils::log(&format!(
            "[jhol-profile] stage={} delta_ms={} total_ms={}",
            stage, delta, total
        ));
        self.last = now;
    }
}

#[derive(Clone)]
struct ResolvedFetch {
    pkg: String,
    url: String,
    integrity: Option<String>,
    version: String,
}

trait ColdResolveStrategy {
    fn resolve(&self, to_fetch: &[String], npm_fallback: &mut Vec<String>) -> Vec<ResolvedFetch>;
}

struct ManifestThenPackumentStrategy;

impl ColdResolveStrategy for ManifestThenPackumentStrategy {
    fn resolve(&self, to_fetch: &[String], npm_fallback: &mut Vec<String>) -> Vec<ResolvedFetch> {
        let mut resolved_work = Vec::new();
        let mut manifest_requests: Vec<(String, String, String)> =
            Vec::with_capacity(to_fetch.len());
        let mut request_meta: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::with_capacity(to_fetch.len());

        for (idx, pkg) in to_fetch.iter().enumerate() {
            let base = base_name(pkg).to_string();
            let version_req = if pkg.contains('@') && !pkg.starts_with('@') {
                pkg.splitn(2, '@')
                    .nth(1)
                    .unwrap_or("latest")
                    .trim()
                    .to_string()
            } else if pkg.starts_with('@') {
                let idx = pkg.rfind('@').unwrap_or(0);
                if idx > 0 {
                    pkg[idx + 1..].trim().to_string()
                } else {
                    "latest".to_string()
                }
            } else {
                "latest".to_string()
            };

            // Local binary index fast path: avoid manifest/packument for latest specs.
            if version_req == "latest" {
                if let Some(v) = registry::best_binary_version(&base) {
                    resolved_work.push(ResolvedFetch {
                        pkg: pkg.clone(),
                        url: lockfile::tarball_url_from_registry(&base, &v),
                        integrity: None,
                        version: v,
                    });
                    continue;
                }
            }

            let req_id = idx.to_string();
            manifest_requests.push((req_id.clone(), base.clone(), version_req.clone()));
            request_meta.insert(req_id, (pkg.clone(), base, version_req));
        }

        let mut pending_packument: Vec<(String, String, String)> = Vec::new();
        let mut manifest_results =
            registry::parallel_resolve_tarballs_via_manifest(&manifest_requests);
        manifest_results.sort_by(|a, b| a.0.cmp(&b.0));

        for (req_id, result) in manifest_results {
            let Some((pkg, base, version_req)) = request_meta.remove(&req_id) else {
                continue;
            };

            match result {
                Ok(Some((version, url, integrity))) => {
                    resolved_work.push(ResolvedFetch {
                        pkg,
                        url,
                        integrity,
                        version,
                    });
                }
                Ok(None) | Err(_) => {
                    pending_packument.push((pkg, base, version_req));
                }
            }
        }

        if !pending_packument.is_empty() {
            let cache_arc = Arc::new(Mutex::new(std::collections::HashMap::<
                String,
                serde_json::Value,
            >::new()));

            let mut unique_names: Vec<String> = pending_packument
                .iter()
                .map(|(_, base, _)| base.clone())
                .collect();
            unique_names.sort();
            unique_names.dedup();

            let mut meta_results =
                registry::parallel_fetch_metadata(&unique_names, &cache_arc, false);
            meta_results.sort_by(|a, b| a.0.cmp(&b.0));

            let mut meta_map: std::collections::HashMap<String, serde_json::Value> =
                std::collections::HashMap::new();
            for (name, res) in meta_results {
                if let Ok(meta) = res {
                    meta_map.insert(name, meta);
                }
            }

            for (pkg, base, version_req) in pending_packument {
                let Some(meta) = meta_map.get(&base) else {
                    npm_fallback.push(pkg);
                    continue;
                };

                let Some(version) = registry::resolve_version(meta, &version_req) else {
                    npm_fallback.push(pkg);
                    continue;
                };
                let Some(url) = registry::get_tarball_url(meta, &version) else {
                    npm_fallback.push(pkg);
                    continue;
                };
                let integrity = registry::get_integrity_for_version(meta, &version);
                resolved_work.push(ResolvedFetch {
                    pkg,
                    url,
                    integrity,
                    version,
                });
            }
        }

        resolved_work
    }
}

/// Package name without version: lodash@4 -> lodash, @scope/pkg@1.0 -> @scope/pkg
fn base_name(package: &str) -> &str {
    if let Some(idx) = package.rfind('@') {
        // "@scope/pkg" (no version) or paths containing scoped names have '/' after '@'.
        // A version suffix never contains '/'.
        if idx > 0 && !package[idx + 1..].contains('/') {
            return &package[..idx];
        }
    }
    package
}

/// Interned package name — zero allocation on second+ calls.
/// lasso thread-safe interner stores each unique package name once.
/// Returns a 'static reference that lives for the process lifetime.
pub fn interned_base_name(package: &str) -> &'static str {
    let base = base_name(package);
    thread_local! {
        static INTERNER: std::cell::OnceCell<std::sync::Arc<parking_lot::RwLock<ahash::HashMap<String, &'static str>>>> = std::cell::OnceCell::new();
    }
    INTERNER.with(|cell| {
        let interner = cell.get_or_init(|| {
            std::sync::Arc::new(parking_lot::RwLock::new(ahash::HashMap::default()))
        });
        // Fast path
        if let Some(s) = interner.read().get(base) {
            return *s;
        }
        // Insert path: leak the string to get &'static str
        let static_str: &'static str = Box::leak(base.to_string().into_boxed_str());
        interner.write().insert(base.to_string(), static_str);
        static_str
    })
}

/// Exact version from spec, if available (e.g. lodash@4.17.23 -> Some("4.17.23")).
fn exact_version_from_spec(package: &str) -> Option<&str> {
    let idx = package.rfind('@')?;
    if idx == 0 || package[idx + 1..].contains('/') {
        return None;
    }
    let version = package[idx + 1..].trim();
    if version.is_empty()
        || version == "latest"
        || version == "*"
        || version.starts_with('^')
        || version.starts_with('~')
        || version.starts_with('>')
        || version.starts_with('<')
        || version.starts_with('=')
    {
        return None;
    }
    Some(version)
}

/// Read version from node_modules/<base>/package.json (base may be @scope/pkg)
fn read_installed_version(base: &str) -> Option<String> {
    let path = Path::new("node_modules").join(base).join("package.json");
    let s = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("version")?.as_str().map(String::from)
}

fn requested_version_spec(package: &str) -> String {
    if let Some(v) = exact_version_from_spec(package) {
        return v.to_string();
    }
    if package.starts_with('@') {
        if let Some(idx) = package.rfind('@') {
            if idx > 0 && !package[idx + 1..].contains('/') {
                let spec = package[idx + 1..].trim();
                if !spec.is_empty() {
                    return spec.to_string();
                }
            }
        }
    }
    if package.contains('@') && !package.starts_with('@') {
        return package
            .splitn(2, '@')
            .nth(1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "latest".to_string());
    }
    "latest".to_string()
}

/// npm treats a bare `1.2.3` as an exact pin. The Rust `semver` crate treats
/// bare versions as caret (`^1.2.3`), which would wrongly accept `1.9.0`.
fn normalize_npm_version_spec(spec: &str) -> String {
    let trimmed = spec.trim();
    if trimmed.is_empty() || trimmed == "latest" || trimmed == "*" {
        return trimmed.to_string();
    }
    if trimmed.starts_with('=')
        || trimmed.starts_with('^')
        || trimmed.starts_with('~')
        || trimmed.starts_with('>')
        || trimmed.starts_with('<')
        || trimmed.starts_with('*')
        || trimmed.contains(" - ")
        || trimmed.contains("||")
        || trimmed.contains(' ')
        || trimmed.contains(',')
    {
        return trimmed.to_string();
    }
    if semver::Version::parse(trimmed).is_ok() {
        return format!("={}", trimmed);
    }
    trimmed.to_string()
}

fn constraints_satisfied(version: &semver::Version, constraints: &[String]) -> bool {
    constraints.iter().all(|spec| {
        let trimmed = normalize_npm_version_spec(spec);
        if trimmed.is_empty() || trimmed == "latest" || trimmed == "*" {
            return true;
        }
        // Cache VersionReq parse — crore-scale Domino hits this millions of times.
        thread_local! {
            static REQ_CACHE: std::cell::RefCell<AHashMap<String, Option<semver::VersionReq>>> =
                std::cell::RefCell::new(AHashMap::with_hasher(ahash::RandomState::new()));
        }
        let matched = REQ_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let req = cache.entry(trimmed.clone()).or_insert_with(|| {
                semver::VersionReq::parse(&trimmed).ok()
            });
            req.as_ref().is_some_and(|r| r.matches(version))
        });
        if matched {
            return true;
        }
        // Fallback for weird ranges VersionReq rejects.
        registry::version_satisfies(&trimmed, &version.to_string())
    })
}

fn resolve_version_for_constraints(
    metadata: &serde_json::Value,
    constraints: &[String],
) -> Option<String> {
    let versions = metadata.get("versions")?.as_object()?;
    let mut parsed: Vec<semver::Version> = versions
        .keys()
        .filter_map(|k| semver::Version::parse(k).ok())
        .collect();
    parsed.sort();
    parsed.reverse();

    for version in parsed {
        if constraints_satisfied(&version, constraints) {
            return Some(version.to_string());
        }
    }

    None
}

fn current_npm_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn current_npm_cpu() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" | "i386" | "i686" => "ia32",
        "aarch64" => "arm64",
        other => other,
    }
}

fn field_allows_current(value: Option<&serde_json::Value>, current: &str) -> bool {
    let Some(value) = value else {
        return true;
    };
    let Some(list) = value.as_array() else {
        return value.as_str().map(|s| s == current).unwrap_or(true);
    };

    let mut positives = Vec::new();
    for entry in list {
        let Some(raw) = entry.as_str() else {
            continue;
        };
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(neg) = token.strip_prefix('!') {
            if neg == current {
                return false;
            }
        } else {
            positives.push(token);
        }
    }

    positives.is_empty() || positives.iter().any(|p| *p == current)
}

fn version_supported_on_current_platform(metadata: &serde_json::Value, version: &str) -> bool {
    let Some(versions) = metadata.get("versions").and_then(|v| v.as_object()) else {
        return true;
    };
    let Some(ver_obj) = versions.get(version) else {
        return true;
    };
    field_allows_current(ver_obj.get("os"), current_npm_os())
        && field_allows_current(ver_obj.get("cpu"), current_npm_cpu())
}

fn corolla_enabled() -> bool {
    !std::env::var("JHOL_COROLLA")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        .unwrap_or(false)
}

/// Corolla Pulse — Jhol-unique expand∥download helix.
/// As JMR resolves each `name@version`, fire tarball→CAS immediately so expand
/// RTT overlaps download RTT. Not a Bun/npm clone: pattern URLs + content-addressed store.
struct CorollaPulse {
    tx: Option<mpsc::Sender<CorollaJob>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// name@version → already queued with integrity (true) or spark-only (false).
    seen: Arc<Mutex<HashMap<String, bool>>>,
    indexed: Arc<Mutex<HashMap<String, String>>>,
}

struct CorollaJob {
    name: String,
    version: String,
    url: String,
    integrity: Option<String>,
}

impl CorollaPulse {
    fn spawn(cache_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<CorollaJob>();
        let conc = download_concurrency().clamp(8, 64);
        // Leave headroom for packument/micro H2 during expand waves.
        let conc = (conc / 2).clamp(8, 48);
        let indexed = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let indexed_w = Arc::clone(&indexed);
        let inflight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let inflight_w = Arc::clone(&inflight);
        let handle = std::thread::spawn(move || {
            let pool = match rayon::ThreadPoolBuilder::new()
                .num_threads(conc)
                .thread_name(|i| format!("jhol-corolla-{i}"))
                .build()
            {
                Ok(p) => p,
                Err(_) => {
                    while let Ok(job) = rx.recv() {
                        Self::run_job(&job, &cache_dir, &indexed_w);
                    }
                    return;
                }
            };
            // Fire each job immediately — no batch barrier. Expand wave N+1
            // overlaps Corolla downloads from wave N (helix, not stop-the-world).
            while let Ok(job) = rx.recv() {
                let cache_dir = cache_dir.clone();
                let indexed_w = Arc::clone(&indexed_w);
                inflight_w.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let inflight_done = Arc::clone(&inflight_w);
                pool.spawn(move || {
                    Self::run_job(&job, &cache_dir, &indexed_w);
                    inflight_done.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                });
            }
            while inflight_w.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        Self {
            tx: Some(tx),
            handle: Some(handle),
            seen: Arc::new(Mutex::new(HashMap::new())),
            indexed,
        }
    }

    fn run_job(
        job: &CorollaJob,
        cache_dir: &Path,
        indexed: &Mutex<HashMap<String, String>>,
    ) {
        let key = format!("{}@{}", job.name, job.version);
        // Skip network when CAS already holds this exact pin (capsule/JMR rehit).
        {
            let si = utils::read_store_index_cached();
            if let Some(hash) = si.get(&key) {
                let store_file = cache_dir.join("store").join(format!("{}.tgz", hash));
                if store_file.exists() {
                    indexed.lock().unwrap().insert(key, hash.clone());
                    return;
                }
            }
        }
        // Download + unpack in background during expand metadata (helix).
        // finish() runs AFTER expand mark, so unpack wait does not inflate expand_ms.
        if let Ok(hash) = registry::download_tarball_to_store_hash_only_fast(
            &job.url,
            cache_dir,
            &key,
            job.integrity.as_deref(),
        ) {
            let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
            let _ = registry::ensure_unpacked_in_store(&store_path, cache_dir);
            indexed.lock().unwrap().insert(key, hash);
        }
    }

    fn offer(&self, resolution: &registry::MirrorAwareResolution) {
        self.offer_raw(
            &resolution.package,
            &resolution.version,
            &resolution.tarball_url,
            resolution.integrity.clone(),
        );
    }

    fn offer_raw(
        &self,
        package: &str,
        version: &str,
        tarball_url: &str,
        integrity: Option<String>,
    ) {
        let Some(tx) = &self.tx else {
            return;
        };
        let key = format!("{}@{}", package, version);
        let has_integrity = integrity.is_some();
        {
            let mut seen = self.seen.lock().unwrap();
            if let Some(had_integrity) = seen.get(&key) {
                // Spark may queue first without SRI; allow one upgrade re-queue.
                if *had_integrity || !has_integrity {
                    return;
                }
            }
            seen.insert(key, has_integrity);
        }
        let _ = tx.send(CorollaJob {
            name: package.to_string(),
            version: version.to_string(),
            url: tarball_url.to_string(),
            integrity,
        });
    }

    fn finish(mut self) {
        drop(self.tx.take());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let batch = self.indexed.lock().unwrap().clone();
        if !batch.is_empty() {
            let mut index = utils::read_store_index();
            index.extend(batch);
            let _ = utils::write_store_index(&index);
        }
    }
}

/// Incremental Domino lattice — O(deg(parent)) updates, never O(|E|) full rebuild.
/// Crore-class graphs stay linear in edge churn; AND ranges stay exact.
struct DominoLattice {
    roots: AHashMap<String, Vec<String>>,
    edges_out: AHashMap<String, AHashMap<String, String>>,
    /// dep → parent → range (reverse index for Domino retract)
    edges_in: AHashMap<String, AHashMap<String, String>>,
    live: AHashMap<String, Vec<String>>,
}

impl DominoLattice {
    fn with_capacity(n: usize) -> Self {
        Self {
            roots: AHashMap::with_capacity(n.min(4096)),
            edges_out: AHashMap::with_capacity(n),
            edges_in: AHashMap::with_capacity(n),
            live: AHashMap::with_capacity(n),
        }
    }

    fn seed_root(&mut self, name: String, spec: String) {
        let entry = self.roots.entry(name.clone()).or_default();
        if !entry.iter().any(|e| e == &spec) {
            entry.push(spec);
        }
        self.recompute_live(&name);
    }

    fn recompute_live(&mut self, dep: &str) {
        let mut specs: Vec<String> = self
            .roots
            .get(dep)
            .cloned()
            .unwrap_or_default();
        if let Some(parents) = self.edges_in.get(dep) {
            for spec in parents.values() {
                if !specs.iter().any(|e| e == spec) {
                    specs.push(spec.clone());
                }
            }
        }
        if specs.is_empty() {
            self.live.remove(dep);
        } else {
            self.live.insert(dep.to_string(), specs);
        }
    }

    /// Replace parent edges wholesale. Returns deps whose live AND changed.
    fn set_parent_edges(
        &mut self,
        parent: &str,
        deps: AHashMap<String, String>,
    ) -> AHashSet<String> {
        let mut changed = AHashSet::new();
        if let Some(old) = self.edges_out.remove(parent) {
            for (dep, _) in old {
                if let Some(m) = self.edges_in.get_mut(&dep) {
                    m.remove(parent);
                    if m.is_empty() {
                        self.edges_in.remove(&dep);
                    }
                }
                changed.insert(dep);
            }
        }
        for (dep, spec) in &deps {
            self.edges_in
                .entry(dep.clone())
                .or_default()
                .insert(parent.to_string(), spec.clone());
            changed.insert(dep.clone());
        }
        if deps.is_empty() {
            // parent kept with empty deps still selected
        } else {
            self.edges_out.insert(parent.to_string(), deps);
        }
        for dep in &changed {
            self.recompute_live(dep);
        }
        changed
    }

    fn clear_parent(&mut self, parent: &str) -> AHashSet<String> {
        self.set_parent_edges(parent, AHashMap::new())
    }

    fn live_of(&self, name: &str) -> &[String] {
        self.live
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

/// Full rebuild helper for tests / capsule validate (small graphs).
fn rebuild_live_constraints(
    roots: &HashMap<String, Vec<String>>,
    edges_out: &HashMap<String, HashMap<String, String>>,
) -> HashMap<String, Vec<String>> {
    let mut lat = DominoLattice::with_capacity(roots.len() + edges_out.len());
    for (n, specs) in roots {
        for s in specs {
            lat.seed_root(n.clone(), s.clone());
        }
    }
    for (parent, deps) in edges_out {
        let mapped: AHashMap<String, String> = deps.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        lat.set_parent_edges(parent, mapped);
    }
    lat.live.into_iter().collect()
}

/// Drop selections that no longer satisfy live AND. Returns re-queue names.
fn domino_retract_invalid(
    selected: &mut AHashMap<String, String>,
    lattice: &mut DominoLattice,
) -> Vec<String> {
    let mut retracted = Vec::new();
    let names: Vec<String> = selected.keys().cloned().collect();
    for name in names {
        let Some(ver) = selected.get(&name).cloned() else {
            continue;
        };
        let reqs = lattice.live_of(&name);
        let ok = semver::Version::parse(&ver).is_ok_and(|v| constraints_satisfied(&v, reqs));
        if ok {
            continue;
        }
        selected.remove(&name);
        let _ = lattice.clear_parent(&name);
        retracted.push(name);
    }
    retracted
}

fn domino_reachable(
    lattice: &DominoLattice,
    selected: &AHashMap<String, String>,
) -> AHashSet<String> {
    let mut seen = AHashSet::new();
    let mut stack: Vec<String> = lattice.roots.keys().cloned().collect();
    while let Some(name) = stack.pop() {
        if !selected.contains_key(&name) || !seen.insert(name.clone()) {
            continue;
        }
        if let Some(deps) = lattice.edges_out.get(&name) {
            stack.extend(deps.keys().cloned());
        }
    }
    seen
}

fn domino_prune_unreachable(
    lattice: &mut DominoLattice,
    selected: &mut AHashMap<String, String>,
) {
    let keep = domino_reachable(lattice, selected);
    let drop: Vec<String> = selected
        .keys()
        .filter(|k| !keep.contains(*k))
        .cloned()
        .collect();
    for name in drop {
        selected.remove(&name);
        let _ = lattice.clear_parent(&name);
    }
}

const GRAPH_CAPSULE_FORMAT: u8 = 1;

/// Persistent full-graph capsule (Jhol-unique). Enables expand ≪100ms on hit —
/// selection + live edges + tarball URLs, not competitor lockfile parse.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GraphCapsule {
    format: u8,
    roots: Vec<(String, String)>,
    selection: HashMap<String, String>,
    edges: HashMap<String, HashMap<String, String>>,
    tarballs: HashMap<String, String>,
    integrity: HashMap<String, Option<String>>,
}

struct ExpandOutcome {
    specs: Vec<String>,
    /// Corolla joins AFTER expand mark so expand stage stays metadata-only.
    corolla: Option<CorollaPulse>,
}

fn graph_capsule_path(cache_key: &str) -> PathBuf {
    let key_hash = xxhash_rust::xxh3::xxh3_64(cache_key.as_bytes());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".jhol")
        .join("graph_capsules")
        .join(format!("{:x}.json", key_hash))
}

fn load_graph_capsule(cache_key: &str, packages: &[&str]) -> Option<GraphCapsule> {
    let fp = graph_capsule_path(cache_key);
    let raw = std::fs::read_to_string(fp).ok()?;
    let capsule: GraphCapsule = serde_json::from_str(&raw).ok()?;
    if capsule.format != GRAPH_CAPSULE_FORMAT || capsule.selection.is_empty() {
        return None;
    }
    // Roots must cover every requested package (Domino-safe gate).
    let roots_ok = packages.iter().all(|p| {
        let base = base_name(p);
        let spec = normalize_npm_version_spec(&requested_version_spec(p));
        capsule.roots.iter().any(|(n, s)| n == base && s == &spec)
            || capsule.selection.contains_key(base)
    });
    if !roots_ok {
        return None;
    }
    // Live-edge Domino: rebuild constraints and ensure selection still valid.
    let mut root_constraints: HashMap<String, Vec<String>> = HashMap::new();
    for (n, s) in &capsule.roots {
        root_constraints
            .entry(n.clone())
            .or_default()
            .push(s.clone());
    }
    let live = rebuild_live_constraints(&root_constraints, &capsule.edges);
    for (name, ver) in &capsule.selection {
        let reqs = live.get(name).map(|c| c.as_slice()).unwrap_or(&[]);
        let ok = semver::Version::parse(ver).is_ok_and(|v| constraints_satisfied(&v, reqs));
        if !ok {
            return None;
        }
    }
    Some(capsule)
}

fn save_graph_capsule(cache_key: &str, capsule: &GraphCapsule) {
    let fp = graph_capsule_path(cache_key);
    if let Some(parent) = fp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(capsule) {
        let _ = std::fs::write(fp, json);
    }
}

fn expand_with_transitive_dependencies(packages: &[&str]) -> Result<ExpandOutcome, String> {
    // Full install trees need deep transitive walk. Env overrides for tuning.
    let max_depth = if let Some(v) = {
        let o = TRANSITIVE_DEPTH_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
        (o > 0).then_some(o as usize)
    } {
        v
    } else {
        std::env::var("JHOL_TRANSITIVE_DEPTH")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(64)
    };
    let cache_key = {
        let mut specs: Vec<String> = packages.iter().map(|p| (*p).to_string()).collect();
        specs.sort();
        format!("d{}:{}", max_depth, specs.join(","))
    };

    if transitive_cache_enabled() {
        if let Ok(cache) = transitive_memo_cache().read() {
            if let Some(hit) = cache.get(&cache_key) {
                return Ok(ExpandOutcome {
                    specs: hit.clone(),
                    corolla: None,
                });
            }
        }
    }

    // Graph capsule — expand metadata in ≪100ms when Domino-valid.
    if let Some(capsule) = load_graph_capsule(&cache_key, packages) {
        let corolla = corolla_enabled().then(|| {
            let c = CorollaPulse::spawn(PathBuf::from(utils::get_cache_dir()));
            for (name, ver) in &capsule.selection {
                if let Some(url) = capsule.tarballs.get(name) {
                    let integrity = capsule
                        .integrity
                        .get(name)
                        .cloned()
                        .flatten();
                    let _ = c.offer_raw(name, ver, url, integrity);
                }
            }
            c
        });
        let mut specs: Vec<String> = capsule
            .selection
            .iter()
            .map(|(n, v)| format!("{}@{}", n, v))
            .collect();
        specs.sort();
        specs.dedup();
        if transitive_cache_enabled() {
            if let Ok(mut cache) = transitive_memo_cache().write() {
                cache.insert(cache_key, specs.clone());
            }
        }
        return Ok(ExpandOutcome { specs, corolla });
    }

    // Disk-persisted flat specs (legacy) — no tarball URLs, no Corolla seed.
    {
        let key_hash = xxhash_rust::xxh3::xxh3_64(cache_key.as_bytes());
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let base = std::path::Path::new(&home).join(".jhol");
        let fp = base.join(format!("cached_specs_{:x}.json", key_hash));
        if fp.exists() {
            if let Ok(json) = std::fs::read_to_string(&fp) {
                if let Ok(specs) = serde_json::from_str::<Vec<String>>(&json) {
                    if !specs.is_empty() {
                        let roots_ok = packages.iter().all(|p| {
                            let base = base_name(p);
                            specs.iter().any(|s| {
                                base_name(s) == base
                                    || s == p
                                    || s.starts_with(&format!("{}@", base))
                            })
                        });
                        if roots_ok {
                            // Flat specs have no URLs — Spark Corolla from npm URL pattern.
                            let corolla = corolla_enabled().then(|| {
                                let c = CorollaPulse::spawn(PathBuf::from(utils::get_cache_dir()));
                                for spec in &specs {
                                    let base = base_name(spec);
                                    let ver = requested_version_spec(spec);
                                    let ver = ver.trim_start_matches('=');
                                    if semver::Version::parse(ver).is_ok() {
                                        let url = registry::construct_tarball_url_from_pattern(
                                            base, ver,
                                        );
                                        c.offer_raw(base, ver, &url, None);
                                    }
                                }
                                c
                            });
                            return Ok(ExpandOutcome {
                                specs,
                                corolla,
                            });
                        }
                    }
                }
            }
        }
    }

    // Wave-parallel BFS + incremental Domino lattice (O(deg), crore-ready).
    // Corolla Pulse overlaps CAS downloads with expand RTT (Jhol helix).
    let corolla = corolla_enabled().then(|| {
        CorollaPulse::spawn(PathBuf::from(utils::get_cache_dir()))
    });
    let est = packages.len().saturating_mul(64).max(64);
    let mut lattice = DominoLattice::with_capacity(est);
    let mut selected: AHashMap<String, String> = AHashMap::with_capacity(est);
    let mut tarballs: AHashMap<String, String> = AHashMap::with_capacity(est);
    let mut integrity_map: AHashMap<String, Option<String>> = AHashMap::with_capacity(est);
    let mut current_wave: Vec<String> = Vec::new();
    let mut queued: AHashSet<String> = AHashSet::new();

    for package in packages {
        let base = base_name(package).to_string();
        let spec = normalize_npm_version_spec(&requested_version_spec(package));
        lattice.seed_root(base.clone(), spec.clone());
        // Spark ignition: exact root pins start CAS at t=0.
        if let Some(c) = corolla.as_ref() {
            if let Some(ver) = spec.strip_prefix('=') {
                if semver::Version::parse(ver).is_ok() {
                    let url = registry::construct_tarball_url_from_pattern(&base, ver);
                    c.offer_raw(&base, ver, &url, None);
                }
            }
        }
        if queued.insert(base.clone()) {
            current_wave.push(base);
        }
    }

    let mut depth = 0usize;
    let mut prev_resolved: Option<String> = None;
    while !current_wave.is_empty() && depth <= max_depth {
        let specs: Vec<(String, Vec<String>)> = current_wave
            .iter()
            .map(|name| (name.clone(), lattice.live_of(name).to_vec()))
            .collect();

        let resolved = registry::parallel_wave_expand_resolve(&specs);

        let mut prefetch_names: Vec<String> = Vec::new();
        let mut prefetch_set: AHashSet<String> = AHashSet::new();

        for name in &current_wave {
            let Some(resolution) = resolved.get(name) else {
                selected.remove(name);
                let _ = lattice.clear_parent(name);
                tarballs.remove(name);
                integrity_map.remove(name);
                continue;
            };
            let reqs = lattice.live_of(name);
            let ok = semver::Version::parse(&resolution.version)
                .is_ok_and(|v| constraints_satisfied(&v, reqs));
            if !ok {
                selected.remove(name);
                let _ = lattice.clear_parent(name);
                tarballs.remove(name);
                integrity_map.remove(name);
                continue;
            }

            selected.insert(name.clone(), resolution.version.clone());
            tarballs.insert(name.clone(), resolution.tarball_url.clone());
            integrity_map.insert(name.clone(), resolution.integrity.clone());
            let deps: AHashMap<String, String> = resolution
                .dependencies
                .iter()
                .map(|(k, v)| (k.clone(), normalize_npm_version_spec(v)))
                .collect();
            let _ = lattice.set_parent_edges(name, deps);
            if let Some(c) = corolla.as_ref() {
                c.offer(resolution);
            }
            for dep in resolution.dependencies.keys() {
                if prefetch_set.insert(dep.clone()) {
                    prefetch_names.push(dep.clone());
                }
            }
            if let Some(ref prev) = prev_resolved {
                for ghost in crate::second_order_markov::predict_with_context(prev, name, 8) {
                    if prefetch_set.insert(ghost.clone()) {
                        prefetch_names.push(ghost);
                    }
                }
            }
            prev_resolved = Some(name.clone());
        }

        if !prefetch_names.is_empty()
            && std::env::var("JHOL_GHOST_PREFETCH")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        {
            let names = prefetch_names;
            let _ = std::thread::Builder::new()
                .name("jhol-ghost-prefetch".into())
                .spawn(move || {
                    let _ = registry::fetch_abbreviated_packuments_parallel(&names);
                });
        }

        loop {
            let retracted = domino_retract_invalid(&mut selected, &mut lattice);
            if retracted.is_empty() {
                break;
            }
            for name in &retracted {
                tarballs.remove(name);
                integrity_map.remove(name);
            }
        }
        domino_prune_unreachable(&mut lattice, &mut selected);
        tarballs.retain(|k, _| selected.contains_key(k));
        integrity_map.retain(|k, _| selected.contains_key(k));

        let mut next_wave: Vec<String> = Vec::new();
        let mut next_set: AHashSet<String> = AHashSet::new();

        if depth < max_depth {
            let edge_snapshot: Vec<(String, Vec<String>)> = lattice
                .edges_out
                .iter()
                .filter(|(parent, _)| selected.contains_key(*parent))
                .map(|(parent, deps)| (parent.clone(), deps.keys().cloned().collect()))
                .collect();
            for (_parent, deps) in edge_snapshot {
                for dep_name in deps {
                    let reqs = lattice.live_of(&dep_name);
                    let still_ok = selected
                        .get(&dep_name)
                        .and_then(|v| semver::Version::parse(v).ok())
                        .is_some_and(|v| constraints_satisfied(&v, reqs));
                    if still_ok {
                        continue;
                    }
                    selected.remove(&dep_name);
                    let _ = lattice.clear_parent(&dep_name);
                    tarballs.remove(&dep_name);
                    integrity_map.remove(&dep_name);
                    if next_set.insert(dep_name.clone()) {
                        next_wave.push(dep_name);
                    }
                }
            }
        }

        for name in lattice.roots.keys() {
            if selected.contains_key(name) {
                continue;
            }
            if next_set.insert(name.clone()) {
                next_wave.push(name.clone());
            }
        }

        current_wave = next_wave;
        queued = next_set;
        depth += 1;
    }

    tarballs.retain(|k, _| selected.contains_key(k));
    integrity_map.retain(|k, _| selected.contains_key(k));
    let roots_vec: Vec<(String, String)> = lattice
        .roots
        .iter()
        .flat_map(|(n, specs)| specs.iter().map(|s| (n.clone(), s.clone())))
        .collect();
    let edges_for_capsule: HashMap<String, HashMap<String, String>> = lattice
        .edges_out
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().map(|(a, b)| (a.clone(), b.clone())).collect()))
        .collect();

    // Async persist capsule + flat specs — do not block install wall on disk JSON.
    {
        let cache_key_d = cache_key.clone();
        let capsule = GraphCapsule {
            format: GRAPH_CAPSULE_FORMAT,
            roots: roots_vec,
            selection: selected.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            edges: edges_for_capsule,
            tarballs: tarballs.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            integrity: integrity_map
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        let specs_disk = {
            let mut s: Vec<String> = selected
                .iter()
                .map(|(n, v)| format!("{}@{}", n, v))
                .collect();
            s.sort();
            s.dedup();
            s
        };
        let _ = std::thread::Builder::new()
            .name("jhol-capsule-flush".into())
            .spawn(move || {
                save_graph_capsule(&cache_key_d, &capsule);
                let key_hash = xxhash_rust::xxh3::xxh3_64(cache_key_d.as_bytes());
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                let base = std::path::Path::new(&home).join(".jhol");
                let _ = std::fs::create_dir_all(&base);
                let fp = base.join(format!("cached_specs_{:x}.json", key_hash));
                if let Ok(json) = serde_json::to_string(&specs_disk) {
                    let _ = std::fs::write(fp, json);
                }
            });
    }

    {
        let mut seq: Vec<String> = selected.keys().cloned().collect();
        seq.sort();
        crate::second_order_markov::learn_from_install(&seq);
    }

    let mut specs: Vec<String> = selected
        .into_iter()
        .map(|(name, version)| format!("{}@{}", name, version))
        .collect();
    for package in packages {
        let base = base_name(package);
        if !specs.iter().any(|s| base_name(s) == base) {
            specs.push((*package).to_string());
        }
    }
    specs.sort();
    specs.dedup();
    if transitive_cache_enabled() {
        if let Ok(mut cache) = transitive_memo_cache().write() {
            cache.insert(cache_key.clone(), specs.clone());
        }
    }
    Ok(ExpandOutcome {
        specs,
        corolla,
    })
}

/// Thread-safe flag for daemon mode.
pub static DAEMON_MODE_FLAG: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Thread-safe transitive depth override.
pub static TRANSITIVE_DEPTH_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

pub fn set_daemon_mode(val: bool) {
    DAEMON_MODE_FLAG.store(val, std::sync::atomic::Ordering::Relaxed);
}
pub fn set_transitive_depth(depth: u8) {
    TRANSITIVE_DEPTH_OVERRIDE.store(depth, std::sync::atomic::Ordering::Relaxed);
}
const INSTALL_FINGERPRINT_FORMAT: u8 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct InstallFingerprint {
    format_version: u8,
    manifest_hash: u64,
    lockfile_hash: u64,
}

fn file_xxh3(path: &Path) -> std::io::Result<u64> {
    std::fs::read(path).map(|bytes| xxhash_rust::xxh3::xxh3_64(&bytes))
}

fn project_fingerprint_path(
    project_dir: &Path,
    fingerprint_root: &Path,
) -> std::io::Result<std::path::PathBuf> {
    let canonical = std::fs::canonicalize(project_dir)?;
    let project_id = xxhash_rust::xxh3::xxh3_64(canonical.to_string_lossy().as_bytes());
    Ok(fingerprint_root.join(format!("{:016x}.json", project_id)))
}

fn write_project_fingerprint_in(
    project_dir: &Path,
    fingerprint_root: &Path,
) -> std::io::Result<()> {
    let record = InstallFingerprint {
        format_version: INSTALL_FINGERPRINT_FORMAT,
        manifest_hash: file_xxh3(&project_dir.join("package.json"))?,
        lockfile_hash: file_xxh3(&project_dir.join("package-lock.json"))?,
    };
    std::fs::create_dir_all(fingerprint_root)?;
    let destination = project_fingerprint_path(project_dir, fingerprint_root)?;
    let temporary = destination.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, destination)
}

fn lockfile_fingerprint_matches_in(project_dir: &Path, fingerprint_root: &Path) -> bool {
    if !project_dir.join("node_modules").exists() {
        return false;
    }
    let Ok(path) = project_fingerprint_path(project_dir, fingerprint_root) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<InstallFingerprint>(&bytes) else {
        return false;
    };
    if record.format_version != INSTALL_FINGERPRINT_FORMAT {
        return false;
    }
    matches!(
        (
            file_xxh3(&project_dir.join("package.json")),
            file_xxh3(&project_dir.join("package-lock.json")),
        ),
        (Ok(manifest_hash), Ok(lockfile_hash))
            if manifest_hash == record.manifest_hash && lockfile_hash == record.lockfile_hash
    )
}

fn fingerprint_root() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".jhol").join("fingerprints")
}

fn write_project_fingerprint() -> std::io::Result<()> {
    write_project_fingerprint_in(Path::new("."), &fingerprint_root())
}

pub fn lockfile_fingerprint_matches() -> bool {
    lockfile_fingerprint_matches_in(Path::new("."), &fingerprint_root())
}

/// Read all installed packages from node_modules
fn read_all_installed_packages(
    node_modules: &Path,
) -> Result<HashMap<String, crate::lockfile_write::ResolvedPackage>, String> {
    let mut packages = HashMap::new();

    if let Ok(entries) = fs::read_dir(node_modules) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == ".bin" {
                continue;
            }

            if name.starts_with('@') && entry.path().is_dir() {
                if let Ok(scoped) = fs::read_dir(entry.path()) {
                    for scoped_entry in scoped.flatten() {
                        let pkg_json = scoped_entry.path().join("package.json");
                        insert_installed_package(&mut packages, &pkg_json);
                    }
                }
                continue;
            }

            let pkg_json = entry.path().join("package.json");
            insert_installed_package(&mut packages, &pkg_json);
        }
    }

    Ok(packages)
}

fn insert_installed_package(
    packages: &mut HashMap<String, crate::lockfile_write::ResolvedPackage>,
    pkg_json: &Path,
) {
    if !pkg_json.exists() {
        return;
    }
    let Ok(content) = fs::read_to_string(pkg_json) else {
        return;
    };
    let Ok(json) = content.parse::<serde_json::Value>() else {
        return;
    };
    let (Some(version), Some(name)) = (
        json.get("version").and_then(|v| v.as_str()),
        json.get("name").and_then(|v| v.as_str()),
    ) else {
        return;
    };

    packages.insert(
        name.to_string(),
        crate::lockfile_write::ResolvedPackage {
            version: version.to_string(),
            resolved: String::new(),
            integrity: None,
            dependencies: HashMap::new(),
            peer_dependencies: HashMap::new(),
            peer_dependencies_meta: HashMap::new(),
        },
    );
}

/// Faster cache lookup that reuses a preloaded store index (avoids repeated JSON parse per package).
/// Supports offline mode with normalized cache keys for reliable lookups.
fn get_cached_tarball_fast(
    package: &str,
    cache_dir: &std::path::Path,
    store_index: &HashMap<String, String>,
) -> Option<std::path::PathBuf> {
    if !cache_dir.exists() {
        return None;
    }

    let store_dir = cache_dir.join("store");

    // Normalize package@version for consistent cache lookups (supports scoped packages)
    let normalized_key = if let Some(idx) = package.rfind('@') {
        if idx > 0 && !package[idx + 1..].contains('/') {
            let pkg_name = &package[..idx];
            let pkg_version = package[idx + 1..]
                .trim_start_matches('^')
                .trim_start_matches('~')
                .trim_start_matches('>')
                .trim_start_matches('<')
                .trim_start_matches('=')
                .trim_start_matches('v')
                .trim();
            format!("{}@{}", pkg_name, pkg_version)
        } else {
            package.to_string()
        }
    } else {
        package.to_string()
    };

    // Try normalized key first
    if let Some(hash) = store_index.get(&normalized_key) {
        let store_file = store_dir.join(format!("{}.tgz", hash));
        if store_file.exists() {
            return Some(store_file);
        }
    }

    // Try original key
    if package.contains('@') {
        if let Some(hash) = store_index.get(package) {
            let store_file = store_dir.join(format!("{}.tgz", hash));
            if store_file.exists() {
                return Some(store_file);
            }
        }
    } else {
        // Backward-compatible fast path for older index entries keyed by bare package name.
        if let Some(hash) = store_index.get(package) {
            let store_file = store_dir.join(format!("{}.tgz", hash));
            if store_file.exists() {
                return Some(store_file);
            }
        }
        // Try all versions of this package
        for (k, hash) in store_index {
            if k.starts_with(&format!("{}@", package)) {
                let store_file = store_dir.join(format!("{}.tgz", hash));
                if store_file.exists() {
                    return Some(store_file);
                }
            }
        }
    }

    // Legacy fallback: pkg-version.tgz files in cache root.
    let legacy_exact = cache_dir.join(format!("{}.tgz", utils::format_cache_name(package)));
    if legacy_exact.exists() {
        return Some(legacy_exact);
    }

    if !package.contains('@') {
        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{}-", package))
                    && name.ends_with(".tgz")
                    && !name.contains("store")
                {
                    return Some(e.path());
                }
            }
        }
    }

    None
}

fn install_lockfile_layout(options: &InstallOptions) -> Result<bool, JholError> {
    let Some(entries) = lockfile::read_lockfile_install_entries_from_dir(Path::new(".")) else {
        return Ok(false);
    };
    if entries.is_empty() {
        return Ok(false);
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| {
        crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e)
    })?;
    std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| {
        crate::error_handling::utils::io_error("create_bin_dir", Some("node_modules/.bin"), e)
    })?;

    let store_index: HashMap<String, String> = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index_cached().as_ref().clone()
    };

    let mut to_fetch: Vec<crate::lockfile::LockfileInstallEntry> = Vec::new();
    let mut cache_hits: Vec<(crate::lockfile::LockfileInstallEntry, std::path::PathBuf)> =
        Vec::new();
    let mut missing_offline: Vec<String> = Vec::new();

    for entry in entries {
        if !options.no_cache {
            if let Some(tarball) = get_cached_tarball_fast(&entry.spec, &cache_dir, &store_index) {
                cache_hits.push((entry, tarball));
                continue;
            }
        }

        if options.offline {
            missing_offline.push(entry.spec.clone());
            continue;
        }
        to_fetch.push(entry);
    }

    if options.offline && !missing_offline.is_empty() {
        return Err(crate::error_handling::utils::cache_error(
            "offline_lockfile_layout",
            None,
            &format!(
                "Offline mode: package(s) not in cache: {}. Run online install first to cache dependencies.",
                missing_offline.join(", ")
            ),
        ));
    }

    let mut install_entry = |entry: &crate::lockfile::LockfileInstallEntry,
                             tarball_path: &Path|
     -> Result<(), String> {
        let install_path = Path::new(&entry.install_path);
        if install_path.exists() {
            std::fs::remove_dir_all(install_path).map_err(|e| e.to_string())?;
        }
        if let Some(parent) = install_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        selective_extract::extract_selective_to_path(tarball_path, install_path)?;
        // NOTE: bin linking for top-level packages is handled by the single
        // rebuild_bin_links(node_modules) call at the end of this function.
        // Calling link_bins_for_package here AND rebuild_bin_links at the end
        // was redundant — both scan the filesystem, but rebuild_bin_links is
        // the authoritative pass that correctly handles all packages.
        Ok(())
    };

    for (entry, tarball_path) in &cache_hits {
        install_entry(entry, tarball_path).map_err(|e| {
            crate::error_handling::utils::application_error(
                "install_lockfile_layout_cache",
                Some("extract_failed"),
                &format!("{} -> {}", entry.spec, e),
            )
        })?;
    }

    let mut index_batch: HashMap<String, String> = HashMap::new();
    for entry in &to_fetch {
        let hash = registry::download_tarball_to_store_hash_only(
            &entry.resolved,
            &cache_dir,
            &entry.spec,
            entry.integrity.as_deref(),
        )
        .map_err(|e| {
            crate::error_handling::utils::registry_error_with_package(
                "download_tarball_to_store_hash_only",
                &entry.spec,
                None,
                &e,
            )
        })?;

        let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
        install_entry(entry, &store_path).map_err(|e| {
            crate::error_handling::utils::application_error(
                "install_lockfile_layout_fetch",
                Some("extract_failed"),
                &format!("{} -> {}", entry.spec, e),
            )
        })?;
        index_batch.insert(entry.spec.clone(), hash);
    }

    if !index_batch.is_empty() {
        let mut index = utils::read_store_index();
        index.extend(index_batch);
        utils::write_store_index(&index).map_err(|e| {
            crate::error_handling::utils::io_error(
                "write_store_index",
                Some("cache/index.json"),
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
            )
        })?;
    }

    // Skip rebuild_bin_links when all packages already cached (no new bins added).
    // Per-package link_bins_for_package already handles bin creation.
    if !to_fetch.is_empty() {
        let _ = bin_links::rebuild_bin_links(node_modules);
    }
    Ok(true)
}

#[derive(Clone)]
pub struct InstallOptions {
    pub no_cache: bool,
    pub quiet: bool,
    pub backend: Backend,
    pub lockfile_only: bool,
    pub offline: bool,
    pub strict_lockfile: bool,
    pub strict_peer_deps: bool,
    /// When true, specs came from lockfile; skip npm show and use tarball URLs only (no packument).
    pub from_lockfile: bool,
    /// When true, never call Bun/npm; fail with clear error if native install fails.
    pub native_only: bool,
    /// When true, skip lifecycle scripts in backend fallback paths.
    pub no_scripts: bool,
    /// Optional allowlist for packages allowed to run scripts in backend fallback mode.
    pub script_allowlist: Option<std::collections::HashSet<String>>,
    pub resolver: String,
    pub overrides: Option<std::collections::HashMap<String, String>>,
    pub telemetry: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum OptimizeStrategy {
    None,
    Speed,
    Size,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            no_cache: false,
            quiet: false,
            backend: backend::resolve_backend(None),
            lockfile_only: false,
            offline: false,
            strict_lockfile: false,
            strict_peer_deps: false,
            from_lockfile: false,
            native_only: true,
            no_scripts: true,
            script_allowlist: None,
            resolver: String::new(),
            overrides: None,
            telemetry: false,
        }
    }
}

/// Only update lockfile (no node_modules). Uses native resolver and lockfile writer.
fn lockfile_is_reusable(package_json_path: &Path, lock_path: &Path) -> bool {
    let Ok(manifest_bytes) = fs::read(package_json_path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&manifest_bytes) else {
        return false;
    };
    let Ok(lock_bytes) = fs::read(lock_path) else {
        return false;
    };
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&lock_bytes) else {
        return false;
    };

    let expected_hash = xxhash_rust::xxh3::xxh3_64(&manifest_bytes).to_string();
    if lock
        .get("jholManifestHash")
        .and_then(|value| value.as_str())
        != Some(expected_hash.as_str())
    {
        return false;
    }
    if lock.get("lockfileVersion").and_then(|value| value.as_u64()) != Some(3) {
        return false;
    }

    let manifest_name = manifest
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let manifest_version = manifest
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or("0.0.0");
    if lock.get("name").and_then(|value| value.as_str()) != Some(manifest_name)
        || lock.get("version").and_then(|value| value.as_str()) != Some(manifest_version)
    {
        return false;
    }

    let Some(packages) = lock.get("packages").and_then(|value| value.as_object()) else {
        return false;
    };
    let Some(root) = packages.get("").and_then(|value| value.as_object()) else {
        return false;
    };
    if root.get("name").and_then(|value| value.as_str()) != Some(manifest_name)
        || root.get("version").and_then(|value| value.as_str()) != Some(manifest_version)
    {
        return false;
    }

    let Some(manifest_deps) = lockfile::read_package_json_deps(package_json_path) else {
        return false;
    };
    let manifest_deps = manifest_deps.all_deps();
    let Some(root_deps) = root.get("dependencies").and_then(|value| value.as_object()) else {
        return false;
    };
    if manifest_deps.len() != root_deps.len()
        || !manifest_deps
            .keys()
            .all(|name| root_deps.contains_key(name))
    {
        return false;
    }

    for (name, requested) in &manifest_deps {
        let Some(resolved) = root_deps.get(name).and_then(|value| value.as_str()) else {
            return false;
        };
        if !registry::version_satisfies(requested, resolved) {
            return false;
        }
        let package_key = format!("node_modules/{name}");
        let Some(package) = packages
            .get(&package_key)
            .and_then(|value| value.as_object())
        else {
            return false;
        };
        if package.get("version").and_then(|value| value.as_str()) != Some(resolved) {
            return false;
        }
    }

    packages.iter().all(|(key, package)| {
        key.is_empty()
            || package
                .get("integrity")
                .and_then(|value| value.as_str())
                .is_some_and(|integrity| !integrity.is_empty())
    })
}

pub fn install_lockfile_only(_backend: Backend) -> Result<(), JholError> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(crate::error_handling::utils::io_error(
            "install_lockfile_only",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"),
        ));
    }
    let lock_path = Path::new("package-lock.json");
    if lockfile_is_reusable(pj, lock_path) {
        return Ok(());
    }
    crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
        crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e)
    })?;

    let tree = crate::lockfile_write::resolve_full_tree(pj)
        .map_err(|e| crate::error_handling::utils::config_error("resolve_full_tree", None, &e))?;
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree).map_err(|e| {
        crate::error_handling::utils::io_error(
            "write_package_lock",
            Some("package-lock.json"),
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
        )
    })?;
    Ok(())
}

fn check_script_allowlist(
    packages: &[String],
    allowlist: &std::collections::HashSet<String>,
) -> Result<(), JholError> {
    let mut denied = Vec::new();
    for p in packages {
        let name = base_name(p).to_string();
        if !allowlist.contains(&name) {
            denied.push(name);
        }
    }
    if denied.is_empty() {
        Ok(())
    } else {
        denied.sort();
        denied.dedup();
        Err(crate::error_handling::utils::security_error(
            "check_script_allowlist",
            None,
            &format!(
                "Scripts are only allowed for allowlisted packages. Denied: {}",
                denied.join(", ")
            ),
        ))
    }
}

fn sync_hidden_lockfile() {
    let src = Path::new("package-lock.json");
    if !src.exists() {
        return;
    }
    let dest = Path::new("node_modules").join(".package-lock.json");
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::copy(src, dest);
}

fn finalize_install_metadata() {
    sync_hidden_lockfile();
    let _ = write_project_fingerprint();
}

/// Install dependencies from package.json (and optional package-lock.json or bun.lock). Returns list of specs to install.
/// If strict_lockfile is true, requires lockfile to exist and all deps to be in lockfile.
fn can_reuse_lockfile_for_requested(packages: &[&str]) -> Option<Vec<String>> {
    let lock_path = Path::new("package-lock.json");
    if !lock_path.exists() {
        return None;
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."))?;

    // Seed JMR mirror from ALL lockfile packages with FULL dependency data.
    // Lockfile has 2500+ transitive deps with dependencies. Mirror seed with
    // real deps → predict_full_install recurses through them → ~100% coverage.
    // Eliminates BFS HTTP entirely when lockfile is available.
    if let Some(full) = crate::lockfile::read_lockfile_full_resolutions(Path::new(".")) {
        let mirror = crate::resolution_mirror::global_mirror();
        if let Ok(mut idx) = mirror.index.write() {
            for (name, (version, deps, integrity)) in &full {
                let entry = crate::resolution_mirror::MirrorEntry {
                    package: name.clone(),
                    semver_range: format!("^{}", version),
                    resolved_version: version.clone(),
                    dependencies: deps.clone(),
                    optional_dependencies: std::collections::HashMap::new(),
                    peer_dependencies: std::collections::HashMap::new(),
                    tarball_url: None,
                    integrity: integrity.clone(),
                    timestamp_secs: std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    dep_signature: String::new(),
                };
                idx.entries.insert(name.clone(), entry);
            }
        }
    }

    for pkg in packages {
        let name = base_name(pkg);
        let Some(locked) = resolved.get(name) else {
            return None;
        };

        if let Some(exact) = exact_version_from_spec(pkg) {
            if exact != locked {
                return None;
            }
        }
    }

    let mut specs = lockfile::read_all_resolved_specs_from_dir(Path::new("."))?;
    if specs.is_empty() {
        return None;
    }
    specs.sort();
    specs.dedup();
    Some(specs)
}

pub fn resolve_install_from_package_json(strict_lockfile: bool) -> Result<Vec<String>, JholError> {
    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Err(crate::error_handling::utils::io_error(
            "resolve_install_from_package_json",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"),
        ));
    }
    let deps = lockfile::read_package_json_deps(pj_path).ok_or_else(|| {
        crate::error_handling::utils::config_error(
            "read_package_json_deps",
            Some("dependencies"),
            "Could not read package.json dependencies",
        )
    })?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    if strict_lockfile {
        if resolved.is_none() {
            return Err(crate::error_handling::utils::config_error(
                "strict_lockfile_check",
                Some("lockfile"),
                "Strict lockfile required but no package-lock.json or bun.lock found. Run install without --frozen first."
            ));
        }
        if !lockfile::lockfile_integrity_complete(Path::new(".")) {
            return Err(crate::error_handling::utils::config_error(
                "lockfile_integrity_check",
                Some("integrity"),
                "Strict lockfile: integrity entries missing. Run install without --frozen to regenerate lockfile with integrity."
            ));
        }
        let r = resolved.as_ref().unwrap();
        for name in deps.keys() {
            if !r.contains_key(name) {
                return Err(crate::error_handling::utils::detailed_resolution_error(
                    "strict_lockfile_validation",
                    name,
                    "dependency not in lockfile",
                    "Strict lockfile: dependency not in lockfile. Run install without --frozen to update lockfile."
                ));
            }
        }
    }

    // When lockfile URLs are available, prefer full resolved spec list (top-level + transitive)
    // so native lockfile/offline installs can be deterministic and complete.
    if let Some(mut specs) = lockfile::read_all_resolved_specs_from_dir(Path::new(".")) {
        if !specs.is_empty() {
            specs.sort();
            specs.dedup();
            return Ok(specs);
        }
    }

    Ok(lockfile::resolve_deps_for_install(
        &deps.deps,
        resolved.as_ref(),
    ))
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with backend fallback.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), JholError> {
    let mut profiler = InstallProfiler::new();
    let download_conc = download_concurrency();
    let cache_install_conc = cache_install_concurrency();
    let worker_pool_seq_threshold = worker_pool_sequential_threshold();
    let legacy_chunk_scheduler = use_legacy_chunk_scheduler();

    // TLS/HTTP/2 prewarm — hide first-RTT behind early install work (Jhol helix).
    if !options.offline && !options.no_cache {
        crate::http_client::prewarm();
    }

    if options.strict_peer_deps {
        let pj = Path::new("package.json");
        if pj.exists() {
            crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
                crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e)
            })?;
        }
    }

    // Fast path: node_modules exists and topology unchanged → install is a no-op.
    // Placed BEFORE expand_with_transitive_dependencies to skip BFS entirely.
    if std::path::Path::new("node_modules").exists() && lockfile_fingerprint_matches() {
        return Ok(());
    }

    // Capsule fast-path: if .jhol.capsule exists and manifest hash matches,
    // install directly from capsule — no lockfile parse, no resolution.
    if !options.lockfile_only && !options.no_cache {
        let cap_path = crate::capsule::capsule_path(Path::new("."));
        if cap_path.exists() {
            match crate::capsule::read_capsule(&cap_path) {
                Ok(view) => {
                    if let Ok(hash) = crate::capsule::compute_manifest_hash(Path::new(".")) {
                        if view.manifest_hash() == hash {
                            if !options.quiet {
                                crate::utils::log("Capsule hit — installing from capsule");
                            }
                            return crate::capsule::install_from_capsule(&view, Path::new("."))
                                .map_err(|e| {
                                    crate::error_handling::utils::application_error(
                                        "capsule_install",
                                        Some("install_failed"),
                                        &e,
                                    )
                                })
                                .and_then(|_| {
                                    finalize_install_metadata();
                                    Ok(())
                                });
                        }
                    }
                }
                Err(_) => {
                    // Corrupt capsule, fall through to normal install
                    if !options.quiet {
                        crate::utils::log("Capsule corrupt — falling back to lockfile");
                    }
                }
            }
        }
    }

    let mut pending_corolla: Option<CorollaPulse> = None;
    let requested_packages: Vec<String> = if options.from_lockfile {
        packages.iter().map(|p| (*p).to_string()).collect()
    } else if let Some(specs) = can_reuse_lockfile_for_requested(packages) {
        if !options.quiet {
            crate::utils::log(&format!(
                "Reusing lockfile graph ({} packages)",
                specs.len()
            ));
        }
        specs
    } else {
        // JMR mirror prediction: skip network when Domino-complete (coverage 1.0).
        let jmr_hit: Option<(Vec<String>, Option<CorollaPulse>)> = {
            let top_level: Vec<(String, String)> = packages
                .iter()
                .map(|p| {
                    let name = crate::install::base_name(p);
                    let spec =
                        normalize_npm_version_spec(&crate::install::requested_version_spec(p));
                    (name.to_string(), spec)
                })
                .collect();
            let mirror = crate::resolution_mirror::global_mirror();
            let predicted = mirror.predict_full_install(&top_level);
            let tops_ok = top_level
                .iter()
                .all(|(name, _)| predicted.predicted_selection.contains_key(name));
            // coverage > 0.5 was a Domino footgun — incomplete trees looked "mostly OK".
            if predicted.coverage >= 1.0 && tops_ok && predicted.missing_packages.is_empty() {
                let corolla = corolla_enabled().then(|| {
                    let c = CorollaPulse::spawn(PathBuf::from(utils::get_cache_dir()));
                    for (name, ver) in &predicted.predicted_selection {
                        if let Some(url) = predicted.predicted_tarball_urls.get(name) {
                            let integrity =
                                predicted.predicted_integrity.get(name).cloned().flatten();
                            c.offer_raw(name, ver, url, integrity);
                        }
                    }
                    c
                });
                let specs: Vec<String> = predicted
                    .predicted_selection
                    .into_iter()
                    .map(|(name, ver)| format!("{}@{}", name, ver))
                    .collect();
                Some((specs, corolla))
            } else {
                None
            }
        };
        if let Some((specs, corolla)) = jmr_hit {
            pending_corolla = corolla;
            specs
        } else {
            match expand_with_transitive_dependencies(packages) {
                Ok(ExpandOutcome { specs, corolla }) if !specs.is_empty() => {
                    pending_corolla = corolla;
                    specs
                }
                Ok(_) => packages.iter().map(|p| (*p).to_string()).collect(),
                Err(err) => {
                    crate::utils::log(&format!(
                        "warning: transitive expansion failed, continuing with direct specs: {}",
                        err
                    ));
                    packages.iter().map(|p| (*p).to_string()).collect()
                }
            }
        }
    };
    profiler.mark("expand_or_reuse_graph");
    // Corolla CAS join AFTER expand mark — expand stage stays metadata/capsule only.
    if let Some(c) = pending_corolla {
        c.finish();
    }
    profiler.mark("corolla_cas_join");

    if options.from_lockfile {
        if install_lockfile_layout(options)? {
            finalize_install_metadata();
            return Ok(());
        }
    }

    let mut seen_packages = HashSet::new();
    let mut to_install_from_cache = Vec::new();
    let mut to_fetch = Vec::new();
    let mut missing_for_offline = Vec::new();
    let cache_dir_for_lookup = std::path::PathBuf::from(utils::get_cache_dir());
    let store_index: HashMap<String, String> = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index_cached().as_ref().clone()
    };

    // Batch store presence: one readdir builds HashSet of existing tarballs.
    // Replaces N×stat syscalls with one readdir + HashSet lookups.
    let store_tarballs: std::sync::Arc<std::collections::HashSet<String>> = if options.no_cache {
        std::sync::Arc::new(std::collections::HashSet::new())
    } else {
        let store_dir = cache_dir_for_lookup.join("store");
        let mut set = std::collections::HashSet::new();
        if let Ok(entries) = std::fs::read_dir(&store_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".tgz") {
                    set.insert(name);
                }
            }
        }
        std::sync::Arc::new(set)
    };

    for package in &requested_packages {
        let base = base_name(package);
        if seen_packages.contains(base) {
            if !options.quiet {
                crate::utils::log(&format!(
                    "Warning: Multiple versions of {} requested.",
                    base
                ));
            }
        }
        seen_packages.insert(base.to_string());
        if !options.quiet {
            utils::log(&format!("Installing package: {}", package));
        }

        if !options.no_cache {
            if let Some(tarball) =
                get_cached_tarball_fast(package, &cache_dir_for_lookup, &store_index)
            {
                let fname = tarball
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if store_tarballs.contains(&fname) {
                    if !options.quiet {
                        crate::utils::log(&format!("Installing {} from cache...", package));
                    }
                    to_install_from_cache.push((package.to_string(), tarball));
                    continue;
                }
            }
        }
        if options.offline {
            missing_for_offline.push(package.to_string());
            continue;
        }
        to_fetch.push(package.to_string());
    }

    // Offline mode must be fully cache-backed; never schedule network fetches.
    if options.offline && !missing_for_offline.is_empty() {
        return Err(crate::error_handling::utils::cache_error(
            "offline_install",
            None,
            &format!(
                "Offline mode: package(s) not in cache: {}. Run online install first to cache dependencies.",
                missing_for_offline.join(", ")
            ),
        ));
    }

    if !options.offline && !missing_for_offline.is_empty() {
        to_fetch.extend(missing_for_offline.iter().cloned());
    }
    profiler.mark("classify_cache_vs_fetch");

    // Skip npm show when we trust the lockfile or frozen (zero packument)
    if !to_fetch.is_empty()
        && !options.from_lockfile
        && !options.strict_lockfile
        && !options.native_only
    {
        let results = registry::parallel_validate_packages(&to_fetch, NPM_SHOW_TIMEOUT_SECS);
        let invalid: Vec<String> = results
            .iter()
            .filter(|(_, ok)| !*ok)
            .map(|(p, _)| p.clone())
            .collect();
        if !invalid.is_empty() {
            return Err(crate::error_handling::utils::registry_error_with_package(
                "validate_packages",
                &invalid.join(", "),
                None,
                "Package(s) not found or invalid",
            ));
        }
    }

    // Install from cache: link from unpacked store, or fall back to backend/copy
    if !to_install_from_cache.is_empty() {
        let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
        let node_modules = Path::new("node_modules");
        std::fs::create_dir_all(node_modules).map_err(|e| {
            crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e)
        })?;
        // Create .bin directory for compatibility with npm
        std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| {
            crate::error_handling::utils::io_error("create_bin_dir", Some("node_modules/.bin"), e)
        })?;
        let mut fallback_tarballs = Vec::new();
        let cache_install_inputs: Vec<(String, std::path::PathBuf)> = to_install_from_cache
            .iter()
            .map(|(pkg, tarball_path)| (pkg.clone(), tarball_path.clone()))
            .collect();
        let mut install_results: Vec<(String, std::path::PathBuf, bool)> = if legacy_chunk_scheduler
        {
            let mut outputs = Vec::with_capacity(cache_install_inputs.len());
            for chunk in cache_install_inputs.chunks(cache_install_conc) {
                use std::sync::mpsc;
                use std::thread;
                let (tx, rx) = mpsc::channel();
                for (pkg, tarball_path) in chunk {
                    let pkg = pkg.clone();
                    let tarball_path = tarball_path.clone();
                    let cache_dir = cache_dir.clone();
                    let node_modules = node_modules.to_path_buf();
                    let tx = tx.clone();
                    thread::spawn(move || {
                        let base = base_name(&pkg).to_string();
                        let ok = match registry::ensure_unpacked_in_store(&tarball_path, &cache_dir)
                        {
                            Ok(unpacked) => {
                                utils::link_package_from_store(&unpacked, &node_modules, &base)
                                    .is_ok()
                                    || selective_extract::extract_selective(
                                        &tarball_path,
                                        &node_modules,
                                        &base,
                                    )
                                    .is_ok()
                            }
                            Err(_) => false,
                        };
                        let _ = tx.send((pkg, tarball_path, ok));
                    });
                }
                drop(tx);
                for result in rx {
                    outputs.push(result);
                }
            }
            outputs
        } else {
            let cache_dir = cache_dir.clone();
            let node_modules = node_modules.to_path_buf();
            run_worker_pool(
                cache_install_inputs,
                cache_install_conc,
                worker_pool_seq_threshold,
                move |(pkg, tarball_path)| {
                    let base = base_name(&pkg).to_string();
                    let ok = match registry::ensure_unpacked_in_store(&tarball_path, &cache_dir) {
                        Ok(unpacked) => {
                            utils::link_package_from_store(&unpacked, &node_modules, &base).is_ok()
                                || selective_extract::extract_selective(
                                    &tarball_path,
                                    &node_modules,
                                    &base,
                                )
                                .is_ok()
                        }
                        Err(_) => false,
                    };
                    (pkg, tarball_path, ok)
                },
            )
        };

        install_results.sort_by(|a, b| a.0.cmp(&b.0));
        for (pkg, tarball_path, ok) in install_results {
            if ok {
                let base = base_name(&pkg).to_string();
                let _ = bin_links::link_bins_for_package(node_modules, &base);
                if !options.quiet {
                    utils::log(&format!("Installed {} from cache (link/copy).", pkg));
                }
            } else {
                fallback_tarballs.push((pkg, tarball_path));
            }
        }
        if !fallback_tarballs.is_empty() {
            let fallback_pkgs: Vec<String> =
                fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
            crate::utils::record_fallback_reason("cache_link_or_extract_failed", &fallback_pkgs);
            if options.native_only {
                return Err(crate::error_handling::utils::cache_error(
                    "native_only_install",
                    None,
                    &format!("Native-only: could not link or extract from cache for: {}. Try JHOL_LINK=0 or run without --native-only.", fallback_pkgs.join(", "))
                ));
            }
            if !options.no_scripts {
                if let Some(allowlist) = &options.script_allowlist {
                    let pkgs: Vec<String> =
                        fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
                    check_script_allowlist(&pkgs, allowlist)?;
                }
            }
            let paths: Vec<std::path::PathBuf> =
                fallback_tarballs.iter().map(|(_, p)| p.clone()).collect();
            match backend::backend_install_tarballs(&paths, options.backend, options.no_scripts) {
                Ok(()) => {
                    for (pkg, _) in &fallback_tarballs {
                        if !options.quiet {
                            utils::log(&format!("Installed {} from cache (backend).", pkg));
                        }
                    }
                }
                Err(e) => {
                    return Err(crate::error_handling::utils::application_error(
                        "backend_install_tarballs",
                        Some("install_failed"),
                        &e,
                    ))
                }
            }
        }
    }
    profiler.mark("install_from_cache");

    if to_fetch.is_empty() {
        finalize_install_metadata();
        return Ok(());
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| {
        crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e)
    })?;

    let mut npm_fallback = Vec::new();
    let mut index_batch: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if options.from_lockfile {
        // Zero packument: use lockfile URLs and integrity when present, parallel download, then extract
        let (resolved_urls, resolved_integrity) =
            match lockfile::read_resolved_urls_and_integrity_from_dir(Path::new(".")) {
                Some((u, i)) => (u, i),
                None => (
                    std::collections::HashMap::new(),
                    std::collections::HashMap::new(),
                ),
            };
        let mut work: Vec<(String, String, Option<String>)> = Vec::new();
        for pkg in &to_fetch {
            if options.no_cache {
                npm_fallback.push(pkg.clone());
                continue;
            }
            let url = resolved_urls.get(pkg).cloned().or_else(|| {
                let base = base_name(pkg);
                let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("latest");
                Some(lockfile::tarball_url_from_registry(base, version))
            });
            match url {
                Some(u) => {
                    let integrity = resolved_integrity.get(pkg).cloned();
                    work.push((pkg.clone(), u, integrity));
                }
                None => npm_fallback.push(pkg.clone()),
            }
        }
        let dl_inputs: Vec<(String, String, Option<String>)> = work;
        let dl_concurrency = download_conc;
        let mut download_results: Vec<(String, Result<String, String>)> =
            if legacy_chunk_scheduler {
                let mut outputs = Vec::with_capacity(dl_inputs.len());
                for chunk in dl_inputs.chunks(dl_concurrency) {
                    use std::sync::mpsc;
                    use std::thread;
                    let (tx, rx) = mpsc::channel();
                    for (pkg, url, integrity) in chunk {
                        let pkg = pkg.clone();
                        let url = url.clone();
                        let integrity = integrity.clone();
                        let cache_dir = cache_dir.clone();
                        let node_modules = node_modules.to_path_buf();
                        let tx = tx.clone();
                        thread::spawn(move || {
                            let base = base_name(&pkg).to_string();
                            let pkg_key = exact_version_from_spec(&pkg)
                                .map(|v| format!("{}@{}", base, v))
                                .unwrap_or_else(|| pkg.clone());
                            let res = registry::download_tarball_to_store_hash_only(
                                &url,
                                &cache_dir,
                                &pkg_key,
                                integrity.as_deref(),
                            )
                            .and_then(|hash| {
                                let store_path =
                                    cache_dir.join("store").join(format!("{}.tgz", hash));
                                let unpacked =
                                    registry::ensure_unpacked_in_store(&store_path, &cache_dir)?;
                                let linked = utils::link_package_from_store(
                                    &unpacked,
                                    &node_modules,
                                    &base,
                                )
                                .is_ok()
                                    || selective_extract::extract_selective(
                                        &store_path,
                                        &node_modules,
                                        &base,
                                    )
                                    .is_ok();
                                if !linked {
                                    return Err(format!("link failed for {}", base));
                                }
                                let _ = bin_links::link_bins_for_package(&node_modules, &base);
                                Ok(hash)
                            });
                            let _ = tx.send((pkg, res));
                        });
                    }
                    drop(tx);
                    for result in rx {
                        outputs.push(result);
                    }
                }
                outputs
            } else {
                let cache_dir = cache_dir.clone();
                let node_modules = node_modules.to_path_buf();
                run_worker_pool(
                    dl_inputs,
                    dl_concurrency,
                    worker_pool_seq_threshold,
                    move |(pkg, url, integrity)| {
                        let base = base_name(&pkg).to_string();
                        let pkg_key = exact_version_from_spec(&pkg)
                            .map(|v| format!("{}@{}", base, v))
                            .unwrap_or_else(|| pkg.clone());
                        let res = registry::download_tarball_to_store_hash_only(
                            &url,
                            &cache_dir,
                            &pkg_key,
                            integrity.as_deref(),
                        )
                        .and_then(|hash| {
                            let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                            let unpacked =
                                registry::ensure_unpacked_in_store(&store_path, &cache_dir)?;
                            let linked =
                                utils::link_package_from_store(&unpacked, &node_modules, &base)
                                    .is_ok()
                                    || selective_extract::extract_selective(
                                        &store_path,
                                        &node_modules,
                                        &base,
                                    )
                                    .is_ok();
                            if !linked {
                                return Err(format!("link failed for {}", base));
                            }
                            let _ = bin_links::link_bins_for_package(&node_modules, &base);
                            Ok(hash)
                        });
                        (pkg, res)
                    },
                )
            };
        download_results.sort_by(|a, b| a.0.cmp(&b.0));
        for (pkg, res) in download_results {
            match res {
                Ok(hash) => {
                    index_batch.insert(pkg.clone(), hash);
                    let base = base_name(&pkg);
                    if !options.quiet {
                        let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("");
                        crate::utils::log(&format!("Installed {}@{} (native)", base, version));
                    }
                }
                Err(_) => npm_fallback.push(pkg),
            }
        }
        if !index_batch.is_empty() {
            let mut index = utils::read_store_index();
            index.extend(index_batch);
            utils::write_store_index(&index).map_err(|e| {
                crate::error_handling::utils::io_error(
                    "write_store_index",
                    Some("cache/index.json"),
                    std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                )
            })?;
        }
        profiler.mark("from_lockfile_resolve_download_install");
    } else {
        if options.no_cache {
            npm_fallback.extend(to_fetch.clone());
        } else {
            // Fast path: exact specs can be resolved to tarball URLs without packument fetches.
            let mut work: Vec<ResolvedFetch> = Vec::with_capacity(to_fetch.len());
            let mut needs_resolution: Vec<String> = Vec::new();
            for pkg in &to_fetch {
                if let Some(version) = exact_version_from_spec(pkg) {
                    let base = base_name(pkg);
                    work.push(ResolvedFetch {
                        pkg: pkg.clone(),
                        url: lockfile::tarball_url_from_registry(base, version),
                        integrity: None,
                        version: version.to_string(),
                    });
                } else {
                    needs_resolution.push(pkg.clone());
                }
            }

            if !needs_resolution.is_empty() {
                let strategy = ManifestThenPackumentStrategy;
                let mut resolved = strategy.resolve(&needs_resolution, &mut npm_fallback);
                work.append(&mut resolved);
            }
            profiler.mark("resolve_cold_specs");

            let dl_inputs: Vec<ResolvedFetch> = work;
            let dl_concurrency = download_conc;
            let mut download_results: Vec<(String, Result<(String, String), String>)> =
                if legacy_chunk_scheduler {
                let mut outputs = Vec::with_capacity(dl_inputs.len());
                for chunk in dl_inputs.chunks(dl_concurrency) {
                    use std::sync::mpsc;
                    use std::thread;
                    let (tx, rx) = mpsc::channel();
                    for item in chunk {
                        let pkg = item.pkg.clone();
                        let url = item.url.clone();
                        let integrity = item.integrity.clone();
                        let version = item.version.clone();
                        let cache_dir = cache_dir.clone();
                        let node_modules = node_modules.to_path_buf();
                        let tx = tx.clone();
                        thread::spawn(move || {
                            let base = base_name(&pkg).to_string();
                            let pkg_key = format!("{}@{}", base, version);
                            let res = registry::download_tarball_to_store_hash_only(
                                &url,
                                &cache_dir,
                                &pkg_key,
                                integrity.as_deref(),
                            )
                            .and_then(|hash| {
                                let store_path =
                                    cache_dir.join("store").join(format!("{}.tgz", hash));
                                let unpacked =
                                    registry::ensure_unpacked_in_store(&store_path, &cache_dir)?;
                                let linked = utils::link_package_from_store(
                                    &unpacked,
                                    &node_modules,
                                    &base,
                                )
                                .is_ok()
                                    || selective_extract::extract_selective(
                                        &store_path,
                                        &node_modules,
                                        &base,
                                    )
                                    .is_ok();
                                if !linked {
                                    return Err(format!("link failed for {}", base));
                                }
                                let _ = bin_links::link_bins_for_package(&node_modules, &base);
                                Ok((hash, version))
                            });
                            let _ = tx.send((pkg, res));
                        });
                    }
                    drop(tx);
                    for result in rx {
                        outputs.push(result);
                    }
                }
                outputs
            } else {
                let cache_dir = cache_dir.clone();
                let node_modules = node_modules.to_path_buf();
                run_worker_pool(
                    dl_inputs,
                    dl_concurrency,
                    worker_pool_seq_threshold,
                    move |item| {
                        let pkg = item.pkg;
                        let url = item.url;
                        let integrity = item.integrity;
                        let version = item.version;
                        let base = base_name(&pkg).to_string();
                        let pkg_key = format!("{}@{}", base, version);
                        let res = registry::download_tarball_to_store_hash_only(
                            &url,
                            &cache_dir,
                            &pkg_key,
                            integrity.as_deref(),
                        )
                        .and_then(|hash| {
                            let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                            let unpacked =
                                registry::ensure_unpacked_in_store(&store_path, &cache_dir)?;
                            let linked =
                                utils::link_package_from_store(&unpacked, &node_modules, &base)
                                    .is_ok()
                                    || selective_extract::extract_selective(
                                        &store_path,
                                        &node_modules,
                                        &base,
                                    )
                                    .is_ok();
                            if !linked {
                                return Err(format!("link failed for {}", base));
                            }
                            let _ = bin_links::link_bins_for_package(&node_modules, &base);
                            Ok((hash, version))
                        });
                        (pkg, res)
                    },
                )
            };

            download_results.sort_by(|a, b| a.0.cmp(&b.0));
            for (pkg, res) in download_results {
                match res {
                    Ok((hash, version)) => {
                        let index_key = format!("{}@{}", base_name(&pkg), version);
                        index_batch.insert(index_key, hash.clone());
                        let base = base_name(&pkg);
                        if !options.quiet {
                            crate::utils::log(&format!("Installed {}@{} (native)", base, version));
                        }
                    }
                    Err(_) => npm_fallback.push(pkg),
                }
            }

            if !index_batch.is_empty() {
                let mut index = utils::read_store_index();
                index.extend(index_batch);
                utils::write_store_index(&index).map_err(|e| {
                    crate::error_handling::utils::io_error(
                        "write_store_index",
                        Some("cache/index.json"),
                        std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                    )
                })?;
            }
            profiler.mark("download_unpack_install_cold");
        }
    }

    if npm_fallback.is_empty() {
        let lock_path = Path::new("package-lock.json");
        let pj_path = Path::new("package.json");
        if !lock_path.exists() && pj_path.exists() {
            if let Ok(packages) = read_all_installed_packages(node_modules) {
                let _ = crate::lockfile_write::write_package_lock(lock_path, pj_path, &packages);
            }
        }
        finalize_install_metadata();
        let _ = crate::capsule::write_capsule_from_project(Path::new("."));
        return Ok(());
    }

    if options.native_only {
        crate::utils::record_fallback_reason("native_install_failed", &npm_fallback);
        return Err(crate::error_handling::utils::application_error(
            "native_only_install",
            Some("install_failed"),
            &format!("Native-only: install failed for: {}. Run without --native-only to use Bun/npm fallback.", npm_fallback.join(", "))
        ));
    }

    crate::utils::record_fallback_reason("native_install_failed", &npm_fallback);

    // Save dependency trees to offline cache for future offline installs
    if !options.offline {
        let mut offline_cache = crate::offline_cache::OfflineCache::new(std::path::PathBuf::from(
            utils::get_cache_dir(),
        ));

        // Read installed packages and cache their dependency trees
        if let Ok(packages) = read_all_installed_packages(node_modules) {
            let trees = crate::offline_cache::build_dependency_tree(&packages);
            for tree in trees {
                let _ = offline_cache.save_tree(tree);
            }
        }
    }

    if !options.no_scripts {
        if let Some(allowlist) = &options.script_allowlist {
            check_script_allowlist(&npm_fallback, allowlist)?;
        }
    }

    // Fallback: backend install for any that native failed
    let fetch_refs: Vec<&str> = npm_fallback.iter().map(|s| s.as_str()).collect();
    let mut attempts = 3;
    loop {
        match backend::backend_install(
            &fetch_refs,
            options.backend,
            options.lockfile_only,
            options.no_scripts,
        ) {
            Ok(()) => {
                let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
                for pkg in &npm_fallback {
                    let base = base_name(pkg);
                    if let Some(version) = read_installed_version(base) {
                        let _ = registry::fill_store_from_registry(base, &version, &cache_dir);
                    }
                    if !options.quiet {
                        utils::log(&format!("Installed {} via backend.", pkg));
                    }
                }
                let _ = bin_links::rebuild_bin_links(node_modules);
                finalize_install_metadata();
                let _ = crate::capsule::write_capsule_from_project(Path::new("."));
                return Ok(());
            }
            Err(e) => {
                if attempts <= 1 {
                    return Err(crate::error_handling::utils::application_error(
                        "backend_install",
                        Some("install_failed"),
                        &e,
                    ));
                }
                if !options.quiet {
                    crate::utils::log("Install failed, retrying in 2s...");
                }
            }
        }
        attempts -= 1;
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_fingerprint_project(root: &Path) {
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"name":"fingerprint-test","version":"1.0.0","dependencies":{"a":"^1.0.0"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{}}"#,
        )
        .unwrap();
    }

    #[test]
    fn project_fingerprint_matching_files_hit() {
        let project = tempdir().unwrap();
        let fingerprints = tempdir().unwrap();
        make_fingerprint_project(project.path());

        write_project_fingerprint_in(project.path(), fingerprints.path()).unwrap();

        assert!(lockfile_fingerprint_matches_in(
            project.path(),
            fingerprints.path()
        ));
    }

    #[test]
    fn project_fingerprint_projects_do_not_collide() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let fingerprints = tempdir().unwrap();
        make_fingerprint_project(first.path());
        make_fingerprint_project(second.path());

        write_project_fingerprint_in(first.path(), fingerprints.path()).unwrap();

        assert!(!lockfile_fingerprint_matches_in(
            second.path(),
            fingerprints.path()
        ));
    }

    #[test]
    fn project_fingerprint_manifest_or_lockfile_change_misses() {
        let project = tempdir().unwrap();
        let fingerprints = tempdir().unwrap();
        make_fingerprint_project(project.path());
        write_project_fingerprint_in(project.path(), fingerprints.path()).unwrap();

        fs::write(project.path().join("package.json"), b"{}").unwrap();
        assert!(!lockfile_fingerprint_matches_in(
            project.path(),
            fingerprints.path()
        ));

        make_fingerprint_project(project.path());
        write_project_fingerprint_in(project.path(), fingerprints.path()).unwrap();
        fs::write(project.path().join("package-lock.json"), b"{}").unwrap();
        assert!(!lockfile_fingerprint_matches_in(
            project.path(),
            fingerprints.path()
        ));
    }

    #[test]
    fn project_fingerprint_malformed_record_misses() {
        let project = tempdir().unwrap();
        let fingerprints = tempdir().unwrap();
        make_fingerprint_project(project.path());
        fs::create_dir_all(fingerprints.path()).unwrap();
        fs::write(
            project_fingerprint_path(project.path(), fingerprints.path()).unwrap(),
            b"not-json",
        )
        .unwrap();

        assert!(!lockfile_fingerprint_matches_in(
            project.path(),
            fingerprints.path()
        ));
    }

    #[test]
    fn project_fingerprint_requires_node_modules() {
        let project = tempdir().unwrap();
        let fingerprints = tempdir().unwrap();
        make_fingerprint_project(project.path());
        write_project_fingerprint_in(project.path(), fingerprints.path()).unwrap();
        fs::remove_dir_all(project.path().join("node_modules")).unwrap();

        assert!(!lockfile_fingerprint_matches_in(
            project.path(),
            fingerprints.path()
        ));
    }

    fn make_reusable_lockfile(root: &Path, requested: &str, resolved: &str) {
        let manifest = format!(
            r#"{{"name":"reuse-test","version":"1.0.0","dependencies":{{"a":"{}"}}}}"#,
            requested
        );
        fs::write(root.join("package.json"), manifest.as_bytes()).unwrap();
        let manifest_hash = xxhash_rust::xxh3::xxh3_64(manifest.as_bytes()).to_string();
        let lock = serde_json::json!({
            "name": "reuse-test",
            "version": "1.0.0",
            "lockfileVersion": 3,
            "jholManifestHash": manifest_hash,
            "packages": {
                "": {
                    "name": "reuse-test",
                    "version": "1.0.0",
                    "dependencies": { "a": resolved }
                },
                "node_modules/a": {
                    "version": resolved,
                    "resolved": format!("https://registry.npmjs.org/a/-/a-{}.tgz", resolved),
                    "integrity": "sha512-test"
                }
            }
        });
        fs::write(
            root.join("package-lock.json"),
            serde_json::to_vec_pretty(&lock).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn lockfile_reuse_accepts_exact_manifest_and_compatible_root() {
        let project = tempdir().unwrap();
        make_reusable_lockfile(project.path(), "^1.0.0", "1.2.0");

        assert!(lockfile_is_reusable(
            &project.path().join("package.json"),
            &project.path().join("package-lock.json")
        ));
    }

    #[test]
    fn lockfile_reuse_rejects_manifest_change_or_missing_hash() {
        let project = tempdir().unwrap();
        make_reusable_lockfile(project.path(), "^1.0.0", "1.2.0");
        fs::write(
            project.path().join("package.json"),
            br#"{"name":"reuse-test","version":"1.0.0","dependencies":{"a":"^2.0.0"}}"#,
        )
        .unwrap();
        assert!(!lockfile_is_reusable(
            &project.path().join("package.json"),
            &project.path().join("package-lock.json")
        ));

        make_reusable_lockfile(project.path(), "^1.0.0", "1.2.0");
        let lock_path = project.path().join("package-lock.json");
        let mut lock: serde_json::Value =
            serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
        lock.as_object_mut().unwrap().remove("jholManifestHash");
        fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
        assert!(!lockfile_is_reusable(
            &project.path().join("package.json"),
            &lock_path
        ));
    }

    #[test]
    fn lockfile_reuse_rejects_missing_integrity_or_incompatible_version() {
        let project = tempdir().unwrap();
        make_reusable_lockfile(project.path(), "^1.0.0", "1.2.0");
        let lock_path = project.path().join("package-lock.json");
        let mut lock: serde_json::Value =
            serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
        lock["packages"]["node_modules/a"]
            .as_object_mut()
            .unwrap()
            .remove("integrity");
        fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
        assert!(!lockfile_is_reusable(
            &project.path().join("package.json"),
            &lock_path
        ));

        make_reusable_lockfile(project.path(), "^2.0.0", "1.2.0");
        assert!(!lockfile_is_reusable(
            &project.path().join("package.json"),
            &lock_path
        ));
    }

    #[test]
    fn lockfile_reuse_rejects_root_dependency_set_mismatch_or_malformed_json() {
        let project = tempdir().unwrap();
        make_reusable_lockfile(project.path(), "^1.0.0", "1.2.0");
        let lock_path = project.path().join("package-lock.json");
        let mut lock: serde_json::Value =
            serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
        lock["packages"][""]["dependencies"]["extra"] =
            serde_json::Value::String("1.0.0".to_string());
        fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
        assert!(!lockfile_is_reusable(
            &project.path().join("package.json"),
            &lock_path
        ));

        fs::write(&lock_path, b"not-json").unwrap();
        assert!(!lockfile_is_reusable(
            &project.path().join("package.json"),
            &lock_path
        ));
    }

    #[test]
    fn domino_rebuild_drops_stale_parent_ranges() {
        let mut roots = HashMap::new();
        roots.insert("app".into(), vec!["=1.0.0".into()]);
        let mut edges = HashMap::new();
        // Parent flipped from needing left@1 + right@1 to only mid@2.
        edges.insert(
            "app".into(),
            HashMap::from([("mid".into(), "^2.0.0".into())]),
        );
        let live = rebuild_live_constraints(&roots, &edges);
        assert!(live.get("mid").is_some_and(|c| c.iter().any(|s| s == "^2.0.0")));
        assert!(!live.contains_key("left"));
        assert!(!live.contains_key("right"));
    }

    #[test]
    fn domino_retract_and_prune_orphan_after_parent_flip() {
        let mut lattice = DominoLattice::with_capacity(8);
        lattice.seed_root("app".into(), "=1.0.0".into());
        let mut selected = AHashMap::new();
        selected.insert("app".into(), "1.0.0".into());
        selected.insert("orphan".into(), "1.0.0".into());
        selected.insert("mid".into(), "1.0.0".into());
        let mut mid_edge = AHashMap::new();
        mid_edge.insert("mid".into(), "^2.0.0".into());
        let _ = lattice.set_parent_edges("app", mid_edge);
        // orphan selected but never linked from roots — prune later.
        let _ = lattice.set_parent_edges("orphan", AHashMap::new());
        let _ = lattice.set_parent_edges("mid", AHashMap::new());

        let retracted = domino_retract_invalid(&mut selected, &mut lattice);
        // mid@1.0.0 fails ^2.0.0
        assert!(retracted.iter().any(|n| n == "mid"));
        assert!(!selected.contains_key("mid"));

        domino_prune_unreachable(&mut lattice, &mut selected);
        assert!(selected.contains_key("app"));
        assert!(!selected.contains_key("orphan"));
        assert!(!lattice.edges_out.contains_key("orphan"));
    }

    /// Crore-class Domino: parent flip touches O(deg), not O(|E|).
    #[test]
    fn domino_incremental_parent_flip_is_local() {
        let n = 50_000usize;
        let mut lattice = DominoLattice::with_capacity(n + 8);
        lattice.seed_root("root".into(), "=1.0.0".into());
        let mut big: AHashMap<String, String> = AHashMap::with_capacity(n);
        for i in 0..n {
            big.insert(format!("p{i}"), "^1.0.0".into());
        }
        let changed = lattice.set_parent_edges("root", big);
        assert_eq!(changed.len(), n);
        assert_eq!(lattice.live.len(), n + 1); // root + n deps

        // Flip to one child — old n edges drop, one new; churn must stay O(n) not O(n²).
        let mut only = AHashMap::new();
        only.insert("only".into(), "^2.0.0".into());
        let flipped = lattice.set_parent_edges("root", only);
        assert!(flipped.contains("only"));
        assert_eq!(flipped.len(), n + 1); // n removals + only
        assert!(!lattice.live.contains_key("p0"));
        assert!(lattice.live_of("only").iter().any(|s| s == "^2.0.0"));
        assert_eq!(lattice.edges_out.get("root").map(|m| m.len()), Some(1));
    }

    #[test]
    fn normalize_npm_bare_semver_is_exact_pin() {
        assert_eq!(normalize_npm_version_spec("4.18.2"), "=4.18.2");
        assert_eq!(normalize_npm_version_spec("^4.18.2"), "^4.18.2");
    }
}
