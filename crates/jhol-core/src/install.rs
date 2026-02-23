use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use crate::backend::{self, Backend};
use crate::lockfile;
use crate::registry;
use crate::utils::{self, NPM_SHOW_TIMEOUT_SECS};

fn download_concurrency() -> usize {
    std::env::var("JHOL_DOWNLOAD_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 64))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 2).clamp(8, 32))
                .unwrap_or(8)
        })
}

fn cache_install_concurrency() -> usize {
    std::env::var("JHOL_CACHE_INSTALL_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 32))
        .unwrap_or(1)
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
        eprintln!("[jhol-profile] stage={} delta_ms={} total_ms={}", stage, delta, total);
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
    fn resolve(
        &self,
        to_fetch: &[String],
        npm_fallback: &mut Vec<String>,
    ) -> Vec<ResolvedFetch>;
}

struct ManifestThenPackumentStrategy;

impl ColdResolveStrategy for ManifestThenPackumentStrategy {
    fn resolve(
        &self,
        to_fetch: &[String],
        npm_fallback: &mut Vec<String>,
    ) -> Vec<ResolvedFetch> {
        let mut resolved_work = Vec::new();
        let mut manifest_requests: Vec<(String, String, String)> = Vec::with_capacity(to_fetch.len());
        let mut request_meta: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::with_capacity(to_fetch.len());

        for (idx, pkg) in to_fetch.iter().enumerate() {
            let base = base_name(pkg).to_string();
            let version_req = if pkg.contains('@') && !pkg.starts_with('@') {
                pkg.splitn(2, '@').nth(1).unwrap_or("latest").trim().to_string()
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

            let req_id = idx.to_string();
            manifest_requests.push((req_id.clone(), base.clone(), version_req.clone()));
            request_meta.insert(req_id, (pkg.clone(), base, version_req));
        }

        let mut pending_packument: Vec<(String, String, String)> = Vec::new();
        let mut manifest_results = registry::parallel_resolve_tarballs_via_manifest(&manifest_requests);
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

            let mut meta_results = registry::parallel_fetch_metadata(&unique_names, &cache_arc);
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

/// Read version from node_modules/<base>/package.json (base may be @scope/pkg)
fn read_installed_version(base: &str) -> Option<String> {
    let path = Path::new("node_modules").join(base).join("package.json");
    let s = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("version")?.as_str().map(String::from)
}

/// Faster cache lookup that reuses a preloaded store index (avoids repeated JSON parse per package).
fn get_cached_tarball_fast(
    package: &str,
    cache_dir: &std::path::Path,
    store_index: &HashMap<String, String>,
) -> Option<std::path::PathBuf> {
    if !cache_dir.exists() {
        return None;
    }

    let store_dir = cache_dir.join("store");
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
                if name.starts_with(&format!("{}-", package)) && name.ends_with(".tgz") && !name.contains("store") {
                    return Some(e.path());
                }
            }
        }
    }

    None
}

pub struct InstallOptions {
    pub no_cache: bool,
    pub quiet: bool,
    pub backend: Backend,
    pub lockfile_only: bool,
    pub offline: bool,
    pub strict_lockfile: bool,
    /// When true, specs came from lockfile; skip npm show and use tarball URLs only (no packument).
    pub from_lockfile: bool,
    /// When true, never call Bun/npm; fail with clear error if native install fails.
    pub native_only: bool,
    /// When true, skip lifecycle scripts in backend fallback paths.
    pub no_scripts: bool,
    /// Optional allowlist for packages allowed to run scripts in backend fallback mode.
    pub script_allowlist: Option<std::collections::HashSet<String>>,
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
            from_lockfile: false,
            native_only: true,
            no_scripts: true,
            script_allowlist: None,
        }
    }
}

/// Only update lockfile (no node_modules). Uses native resolver and lockfile writer.
pub fn install_lockfile_only(_backend: Backend) -> Result<(), String> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let tree = crate::lockfile_write::resolve_full_tree(pj)?;
    let lock_path = Path::new("package-lock.json");
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree)?;
    Ok(())
}

fn check_script_allowlist(packages: &[String], allowlist: &std::collections::HashSet<String>) -> Result<(), String> {
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
        Err(format!(
            "Scripts are only allowed for allowlisted packages. Denied: {}",
            denied.join(", ")
        ))
    }
}

/// Install dependencies from package.json (and optional package-lock.json or bun.lock). Returns list of specs to install.
/// If strict_lockfile is true, requires lockfile to exist and all deps to be in lockfile.
pub fn resolve_install_from_package_json(strict_lockfile: bool) -> Result<Vec<String>, String> {
    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let deps = lockfile::read_package_json_deps(pj_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    if strict_lockfile {
        if resolved.is_none() {
            return Err("Strict lockfile required but no package-lock.json or bun.lock found. Run install without --frozen first.".to_string());
        }
        if !lockfile::lockfile_integrity_complete(Path::new(".")) {
            return Err("Strict lockfile: integrity entries missing. Run install without --frozen to regenerate lockfile with integrity.".to_string());
        }
        let r = resolved.as_ref().unwrap();
        for name in deps.keys() {
            if !r.contains_key(name) {
                return Err(format!("Strict lockfile: dependency {} not in lockfile. Run install without --frozen to update lockfile.", name));
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

    Ok(lockfile::resolve_deps_for_install(&deps, resolved.as_ref()))
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with backend fallback.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), String> {
    let mut profiler = InstallProfiler::new();
    let download_conc = download_concurrency();
    let cache_install_conc = cache_install_concurrency();
    let worker_pool_seq_threshold = worker_pool_sequential_threshold();
    let legacy_chunk_scheduler = use_legacy_chunk_scheduler();

    let mut seen_packages = HashSet::new();
    let mut to_install_from_cache = Vec::new();
    let mut to_fetch = Vec::new();
    let mut missing_for_offline = Vec::new();
    let cache_dir_for_lookup = std::path::PathBuf::from(utils::get_cache_dir());
    let store_index = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index()
    };

    for package in packages {
        let base = base_name(package);
        if seen_packages.contains(base) {
            if !options.quiet {
                println!("Warning: Multiple versions of {} requested.", base);
            }
        }
        seen_packages.insert(base.to_string());
        utils::log(&format!("Installing package: {}", package));

        if !options.no_cache {
            if let Some(tarball) = get_cached_tarball_fast(package, &cache_dir_for_lookup, &store_index) {
                if !options.quiet {
                    println!("Installing {} from cache...", package);
                }
                to_install_from_cache.push((package.to_string(), tarball));
                continue;
            }
        }
        if options.offline {
            missing_for_offline.push(package.to_string());
            continue;
        }
        to_fetch.push(package.to_string());
    }

    if !missing_for_offline.is_empty() {
        return Err(format!(
            "Offline mode: package(s) not in cache: {}. Run without --offline to fetch.",
            missing_for_offline.join(", ")
        ));
    }
    profiler.mark("classify_cache_vs_fetch");

    // Skip npm show when we trust the lockfile or frozen (zero packument)
    if !to_fetch.is_empty() && !options.from_lockfile && !options.strict_lockfile && !options.native_only {
        let results = registry::parallel_validate_packages(&to_fetch, NPM_SHOW_TIMEOUT_SECS);
        let invalid: Vec<String> = results.iter().filter(|(_, ok)| !*ok).map(|(p, _)| p.clone()).collect();
        if !invalid.is_empty() {
            return Err(format!("Package(s) not found or invalid: {}", invalid.join(", ")));
        }
    }

    // Install from cache: link from unpacked store, or fall back to backend/copy
    if !to_install_from_cache.is_empty() {
        let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
        let node_modules = Path::new("node_modules");
        std::fs::create_dir_all(node_modules).map_err(|e| e.to_string())?;
        // Create .bin directory for compatibility with npm
        std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| e.to_string())?;
        let mut fallback_tarballs = Vec::new();
        let cache_install_inputs: Vec<(String, std::path::PathBuf)> = to_install_from_cache
            .iter()
            .map(|(pkg, tarball_path)| (pkg.clone(), tarball_path.clone()))
            .collect();
        let mut install_results: Vec<(String, std::path::PathBuf, bool)> = if legacy_chunk_scheduler {
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
                        let ok = match registry::ensure_unpacked_in_store(&tarball_path, &cache_dir) {
                            Ok(unpacked) => {
                                utils::link_package_from_store(&unpacked, &node_modules, &base).is_ok()
                                    || registry::extract_tarball(&tarball_path, &node_modules, &base).is_ok()
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
                                || registry::extract_tarball(&tarball_path, &node_modules, &base).is_ok()
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
                utils::log(&format!("Installed {} from cache (link/copy).", pkg));
            } else {
                fallback_tarballs.push((pkg, tarball_path));
            }
        }
        if !fallback_tarballs.is_empty() {
            let fallback_pkgs: Vec<String> = fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
            crate::utils::record_fallback_reason("cache_link_or_extract_failed", &fallback_pkgs);
            if options.native_only {
                return Err(format!(
                    "Native-only: could not link or extract from cache for: {}. Try JHOL_LINK=0 or run without --native-only.",
                    fallback_pkgs.join(", ")
                ));
            }
            if !options.no_scripts {
                if let Some(allowlist) = &options.script_allowlist {
                    let pkgs: Vec<String> = fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
                    check_script_allowlist(&pkgs, allowlist)?;
                }
            }
            let paths: Vec<std::path::PathBuf> = fallback_tarballs.iter().map(|(_, p)| p.clone()).collect();
            match backend::backend_install_tarballs(&paths, options.backend, options.no_scripts) {
                Ok(()) => {
                    for (pkg, _) in &fallback_tarballs {
                        utils::log(&format!("Installed {} from cache (backend).", pkg));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    profiler.mark("install_from_cache");

    if to_fetch.is_empty() {
        return Ok(());
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| e.to_string())?;

    let mut npm_fallback = Vec::new();
    let mut index_batch: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if options.from_lockfile {
        // Zero packument: use lockfile URLs and integrity when present, parallel download, then extract
        let (resolved_urls, resolved_integrity) = match lockfile::read_resolved_urls_and_integrity_from_dir(Path::new(".")) {
            Some((u, i)) => (u, i),
            None => (std::collections::HashMap::new(), std::collections::HashMap::new()),
        };
        let mut work: Vec<(String, String, Option<String>)> = Vec::new();
        for pkg in &to_fetch {
            if options.no_cache {
                npm_fallback.push(pkg.clone());
                continue;
            }
            let url = resolved_urls
                .get(pkg)
                .cloned()
                .or_else(|| {
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
        let mut download_results: Vec<(String, Result<(String, std::path::PathBuf), String>)> = if legacy_chunk_scheduler {
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
                    let tx = tx.clone();
                    thread::spawn(move || {
                        let res = registry::download_tarball_to_store_hash_only(
                            &url,
                            &cache_dir,
                            &pkg,
                            integrity.as_deref(),
                        )
                        .and_then(|hash| {
                            let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                            registry::ensure_unpacked_in_store(&store_path, &cache_dir)
                                .map(|unpacked| (hash, unpacked))
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
            run_worker_pool(dl_inputs, dl_concurrency, worker_pool_seq_threshold, move |(pkg, url, integrity)| {
                let res = registry::download_tarball_to_store_hash_only(
                    &url,
                    &cache_dir,
                    &pkg,
                    integrity.as_deref(),
                )
                .and_then(|hash| {
                    let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                    registry::ensure_unpacked_in_store(&store_path, &cache_dir)
                        .map(|unpacked| (hash, unpacked))
                });
                (pkg, res)
            })
        };
        download_results.sort_by(|a, b| a.0.cmp(&b.0));
        for (pkg, res) in download_results {
            match res {
                Ok((hash, unpacked)) => {
                    index_batch.insert(pkg.clone(), hash.clone());
                    let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                    let base = base_name(&pkg);
                    if utils::link_package_from_store(&unpacked, node_modules, base).is_err() {
                        if let Err(e) = registry::extract_tarball(&store_path, node_modules, base) {
                            let msg = format!("Extract failed for {}: {}", pkg, e);
                            utils::log(&msg);
                            npm_fallback.push(pkg);
                            continue;
                        }
                    }
                    if !options.quiet {
                        let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("");
                        println!("Installed {}@{} (native)", base, version);
                    }
                }
                Err(_) => npm_fallback.push(pkg),
            }
        }
        if !index_batch.is_empty() {
            let mut index = utils::read_store_index();
            index.extend(index_batch);
            utils::write_store_index(&index).map_err(|e| e.to_string())?;
        }
        profiler.mark("from_lockfile_resolve_download_install");
    } else {
        if options.no_cache {
            npm_fallback.extend(to_fetch.clone());
        } else {
            let strategy = ManifestThenPackumentStrategy;
            let work = strategy.resolve(&to_fetch, &mut npm_fallback);
            profiler.mark("resolve_cold_specs");

            let dl_inputs: Vec<ResolvedFetch> = work;
            let dl_concurrency = download_conc;
            let mut download_results: Vec<(String, Result<(String, String, std::path::PathBuf), String>)> = if legacy_chunk_scheduler {
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
                        let tx = tx.clone();
                        thread::spawn(move || {
                            let res = registry::download_tarball_to_store_hash_only(
                                &url,
                                &cache_dir,
                                &pkg,
                                integrity.as_deref(),
                            )
                            .and_then(|hash| {
                                let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                                registry::ensure_unpacked_in_store(&store_path, &cache_dir)
                                    .map(|unpacked| (hash, version, unpacked))
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
                run_worker_pool(dl_inputs, dl_concurrency, worker_pool_seq_threshold, move |item| {
                    let pkg = item.pkg;
                    let url = item.url;
                    let integrity = item.integrity;
                    let version = item.version;
                    let res = registry::download_tarball_to_store_hash_only(
                        &url,
                        &cache_dir,
                        &pkg,
                        integrity.as_deref(),
                    )
                    .and_then(|hash| {
                        let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                        registry::ensure_unpacked_in_store(&store_path, &cache_dir)
                            .map(|unpacked| (hash, version, unpacked))
                    });
                    (pkg, res)
                })
            };

            download_results.sort_by(|a, b| a.0.cmp(&b.0));
            for (pkg, res) in download_results {
                match res {
                    Ok((hash, version, unpacked)) => {
                        let index_key = format!("{}@{}", base_name(&pkg), version);
                        index_batch.insert(index_key, hash.clone());
                        let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                        let base = base_name(&pkg);
                        if utils::link_package_from_store(&unpacked, node_modules, base).is_ok()
                            || registry::extract_tarball(&store_path, node_modules, base).is_ok()
                        {
                            if !options.quiet {
                                println!("Installed {}@{} (native)", base, version);
                            }
                        } else {
                            npm_fallback.push(pkg);
                        }
                    }
                    Err(_) => npm_fallback.push(pkg),
                }
            }

            if !index_batch.is_empty() {
                let mut index = utils::read_store_index();
                index.extend(index_batch);
                utils::write_store_index(&index).map_err(|e| e.to_string())?;
            }
            profiler.mark("download_unpack_install_cold");
        }
    }

    if npm_fallback.is_empty() {
        return Ok(());
    }

    if options.native_only {
        crate::utils::record_fallback_reason("native_install_failed", &npm_fallback);
        return Err(format!(
            "Native-only: install failed for: {}. Run without --native-only to use Bun/npm fallback.",
            npm_fallback.join(", ")
        ));
    }

    crate::utils::record_fallback_reason("native_install_failed", &npm_fallback);

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
                    utils::log(&format!("Installed {} via backend.", pkg));
                }
                return Ok(());
            }
            Err(e) => {
                if attempts <= 1 {
                    return Err(e);
                }
                if !options.quiet {
                    eprintln!("Install failed, retrying in 2s...");
                }
            }
        }
        attempts -= 1;
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}
