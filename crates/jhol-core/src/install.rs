use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use crate::backend::{self, Backend};
use crate::bin_links;
use crate::error_handling::JholError;
use crate::lockfile;
use crate::registry;
use crate::selective_extract;  // JHOL Selective Extraction
use crate::utils::{self, NPM_SHOW_TIMEOUT_SECS};

fn download_concurrency() -> usize {
    std::env::var("JHOL_DOWNLOAD_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 128))  // Increased to 128 for maximum parallelism
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 4).clamp(32, 128))  // 4x cores, max 128
                .unwrap_or(64)  // Default to 64 (was 8)
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

fn transitive_cache_enabled() -> bool {
    std::env::var("JHOL_DAEMON_MODE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn transitive_memo_cache() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn transitive_metadata_snapshot() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    static CACHE: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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
        crate::utils::log(&format!("[jhol-profile] stage={} delta_ms={} total_ms={}", stage, delta, total));
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

fn constraints_satisfied(version: &semver::Version, constraints: &[String]) -> bool {
    let vs = version.to_string();
    constraints.iter().all(|spec| {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed == "latest" || trimmed == "*" {
            return true;
        }
        if let Ok(req) = semver::VersionReq::parse(trimmed) {
            return req.matches(version);
        }
        registry::version_satisfies(trimmed, &vs)
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
    let Some(value) = value else { return true; };
    let Some(list) = value.as_array() else {
        return value.as_str().map(|s| s == current).unwrap_or(true);
    };

    let mut positives = Vec::new();
    for entry in list {
        let Some(raw) = entry.as_str() else { continue; };
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

fn expand_with_transitive_dependencies(packages: &[&str]) -> Result<Vec<String>, String> {
    // Compatibility-first default: include at least one transitive layer for direct installs.
    // Keeps `axios -> follow-redirects` and similar chains correct without requiring env tuning.
    let max_depth = std::env::var("JHOL_TRANSITIVE_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2);
    let cache_key = {
        let mut specs: Vec<String> = packages.iter().map(|p| (*p).to_string()).collect();
        specs.sort();
        format!("d{}:{}", max_depth, specs.join(","))
    };

    if transitive_cache_enabled() {
        if let Ok(cache) = transitive_memo_cache().lock() {
            if let Some(hit) = cache.get(&cache_key) {
                return Ok(hit.clone());
            }
        }
    }

    let mut constraints: HashMap<String, Vec<String>> = HashMap::new();
    let mut required: HashMap<String, bool> = HashMap::new();
    let mut selected: HashMap<String, String> = HashMap::new();
    let mut metadata_cache: HashMap<String, serde_json::Value> = HashMap::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    for package in packages {
        let base = base_name(package).to_string();
        let spec = requested_version_spec(package);
        constraints.entry(base.clone()).or_default().push(spec);
        required.insert(base.clone(), true);
        queue.push_back((base, 0));
    }

    while let Some((name, depth)) = queue.pop_front() {
        let Some(pkg_constraints) = constraints.get(&name).cloned() else {
            continue;
        };

        if !metadata_cache.contains_key(&name) {
            // Zero-network fast path: resolve from local binary package index and read deps
            // directly from bundled binary package metadata when available.
            if let Some(binary_version) = registry::best_binary_version_matching(&name, &pkg_constraints) {
                let changed = selected
                    .get(&name)
                    .map(|existing| existing != &binary_version)
                    .unwrap_or(true);

                if changed {
                    selected.insert(name.clone(), binary_version.clone());
                    if depth < max_depth {
                        if let Some(required_deps) = registry::binary_package_dependencies(&name, &binary_version) {
                            for (dep_name, dep_spec) in required_deps {
                                let dep_constraints = constraints.entry(dep_name.clone()).or_default();
                                if !dep_constraints.iter().any(|s| s == &dep_spec) {
                                    dep_constraints.push(dep_spec);
                                    required.insert(dep_name.clone(), true);
                                    queue.push_back((dep_name, depth + 1));
                                }
                            }
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            let snapshot_hit = if transitive_cache_enabled() {
                transitive_metadata_snapshot()
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&name).cloned())
            } else {
                None
            };
            let meta = if let Some(hit) = snapshot_hit {
                hit
            } else {
                let fetched = registry::fetch_metadata(&name)
                    .map_err(|e| format!("Failed to fetch metadata for {}: {}", name, e))?;
                if transitive_cache_enabled() {
                    if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                        cache.insert(name.clone(), fetched.clone());
                    }
                }
                fetched
            };
            metadata_cache.insert(name.clone(), meta);
        }

        let metadata = metadata_cache
            .get(&name)
            .ok_or_else(|| format!("Missing metadata for {}", name))?;

        let version = resolve_version_for_constraints(metadata, &pkg_constraints)
            .ok_or_else(|| {
                format!(
                    "Dependency conflict for {} (constraints: {})",
                    name,
                    pkg_constraints.join(", ")
                )
            })?;

        let is_required = *required.get(&name).unwrap_or(&true);
        if !version_supported_on_current_platform(metadata, &version) {
            if is_required {
                return Err(format!(
                    "Package {}@{} is incompatible with current platform (os/cpu)",
                    name, version
                ));
            }
            continue;
        }

        let changed = selected
            .get(&name)
            .map(|existing| existing != &version)
            .unwrap_or(true);

        if !changed {
            continue;
        }

        selected.insert(name.clone(), version.clone());

        if depth < max_depth {
            let required_deps = registry::get_version_required_dependencies(metadata, &version);
            for (dep_name, dep_spec) in required_deps {
                let dep_constraints = constraints.entry(dep_name.clone()).or_default();
                if !dep_constraints.iter().any(|s| s == &dep_spec) {
                    dep_constraints.push(dep_spec);
                    required.insert(dep_name.clone(), true);
                    queue.push_back((dep_name, depth + 1));
                }
            }

            let optional_deps = registry::get_version_optional_dependencies(metadata, &version);
            for (dep_name, dep_spec) in optional_deps {
                let dep_constraints = constraints.entry(dep_name.clone()).or_default();
                if !dep_constraints.iter().any(|s| s == &dep_spec) {
                    dep_constraints.push(dep_spec);
                    required.entry(dep_name.clone()).or_insert(false);
                    queue.push_back((dep_name, depth + 1));
                }
            }
        }
    }

    let mut specs: Vec<String> = selected
        .into_iter()
        .map(|(name, version)| format!("{}@{}", name, version))
        .collect();
    specs.sort();
    if transitive_cache_enabled() {
        if let Ok(mut cache) = transitive_memo_cache().lock() {
            cache.insert(cache_key, specs.clone());
        }
    }
    Ok(specs)
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
    let Ok(content) = fs::read_to_string(pkg_json) else { return; };
    let Ok(json) = content.parse::<serde_json::Value>() else { return; };
    let (Some(version), Some(name)) = (
        json.get("version").and_then(|v| v.as_str()),
        json.get("name").and_then(|v| v.as_str()),
    ) else { return; };

    packages.insert(name.to_string(), crate::lockfile_write::ResolvedPackage {
        version: version.to_string(),
        resolved: String::new(),
        integrity: None,
        dependencies: HashMap::new(),
        peer_dependencies: HashMap::new(),
        peer_dependencies_meta: HashMap::new(),
    });
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
                if name.starts_with(&format!("{}-", package)) && name.ends_with(".tgz") && !name.contains("store") {
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
    std::fs::create_dir_all(node_modules)
        .map_err(|e| crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e))?;
    std::fs::create_dir_all(node_modules.join(".bin"))
        .map_err(|e| crate::error_handling::utils::io_error("create_bin_dir", Some("node_modules/.bin"), e))?;

    let store_index = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index()
    };

    let mut to_fetch: Vec<crate::lockfile::LockfileInstallEntry> = Vec::new();
    let mut cache_hits: Vec<(crate::lockfile::LockfileInstallEntry, std::path::PathBuf)> = Vec::new();
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

    let mut install_entry = |entry: &crate::lockfile::LockfileInstallEntry, tarball_path: &Path| -> Result<(), String> {
        let install_path = Path::new(&entry.install_path);
        if install_path.exists() {
            std::fs::remove_dir_all(install_path).map_err(|e| e.to_string())?;
        }
        if let Some(parent) = install_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        selective_extract::extract_selective_to_path(tarball_path, install_path)?;
        if entry.top_level {
            let _ = bin_links::link_bins_for_package(node_modules, &entry.package);
        }
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

    let _ = bin_links::rebuild_bin_links(node_modules);
    Ok(true)
}

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
        }
    }
}

/// Only update lockfile (no node_modules). Uses native resolver and lockfile writer.
pub fn install_lockfile_only(_backend: Backend) -> Result<(), JholError> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(crate::error_handling::utils::io_error(
            "install_lockfile_only",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found")
        ));
    }
    crate::lockfile_write::validate_root_peer_conflicts(pj)
        .map_err(|e| crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e))?;

    let tree = crate::lockfile_write::resolve_full_tree(pj)
        .map_err(|e| crate::error_handling::utils::config_error("resolve_full_tree", None, &e))?;
    let lock_path = Path::new("package-lock.json");
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree)
        .map_err(|e| crate::error_handling::utils::io_error("write_package_lock", Some("package-lock.json"), std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    Ok(())
}

fn check_script_allowlist(packages: &[String], allowlist: &std::collections::HashSet<String>) -> Result<(), JholError> {
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
            &format!("Scripts are only allowed for allowlisted packages. Denied: {}", denied.join(", "))
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

/// Install dependencies from package.json (and optional package-lock.json or bun.lock). Returns list of specs to install.
/// If strict_lockfile is true, requires lockfile to exist and all deps to be in lockfile.
fn can_reuse_lockfile_for_requested(packages: &[&str]) -> Option<Vec<String>> {
    let resolved = lockfile::read_resolved_from_dir(Path::new("."))?;

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
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found")
        ));
    }
    let deps = lockfile::read_package_json_deps(pj_path)
        .ok_or_else(|| crate::error_handling::utils::config_error("read_package_json_deps", Some("dependencies"), "Could not read package.json dependencies"))?;
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

    Ok(lockfile::resolve_deps_for_install(&deps, resolved.as_ref()))
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with backend fallback.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), JholError> {
    let mut profiler = InstallProfiler::new();
    let download_conc = download_concurrency();
    let cache_install_conc = cache_install_concurrency();
    let worker_pool_seq_threshold = worker_pool_sequential_threshold();
    let legacy_chunk_scheduler = use_legacy_chunk_scheduler();

    if options.strict_peer_deps {
        let pj = Path::new("package.json");
        if pj.exists() {
            crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
                crate::error_handling::utils::config_error(
                    "validate_root_peer_conflicts",
                    None,
                    &e,
                )
            })?;
        }
    }

    let requested_packages: Vec<String> = if options.from_lockfile {
        packages.iter().map(|p| (*p).to_string()).collect()
    } else if let Some(specs) = can_reuse_lockfile_for_requested(packages) {
        if !options.quiet {
            crate::utils::log(&format!("Reusing lockfile graph ({} packages)", specs.len()));
        }
        specs
    } else {
        match expand_with_transitive_dependencies(packages) {
            Ok(specs) if !specs.is_empty() => specs,
            Ok(_) => packages.iter().map(|p| (*p).to_string()).collect(),
            Err(err) => {
                crate::utils::log(&format!("warning: transitive expansion failed, continuing with direct specs: {}", err));
                packages.iter().map(|p| (*p).to_string()).collect()
            }
        }
    };

    if options.from_lockfile {
        if install_lockfile_layout(options)? {
            sync_hidden_lockfile();
            return Ok(());
        }
    }

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

    for package in &requested_packages {
        let base = base_name(package);
        if seen_packages.contains(base) {
            if !options.quiet {
                crate::utils::log(&format!("Warning: Multiple versions of {} requested.", base));
            }
        }
        seen_packages.insert(base.to_string());
        if !options.quiet {
            utils::log(&format!("Installing package: {}", package));
        }

        if !options.no_cache {
            if let Some(tarball) = get_cached_tarball_fast(package, &cache_dir_for_lookup, &store_index) {
                if !options.quiet {
                    crate::utils::log(&format!("Installing {} from cache...", package));
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
    if !to_fetch.is_empty() && !options.from_lockfile && !options.strict_lockfile && !options.native_only {
        let results = registry::parallel_validate_packages(&to_fetch, NPM_SHOW_TIMEOUT_SECS);
        let invalid: Vec<String> = results.iter().filter(|(_, ok)| !*ok).map(|(p, _)| p.clone()).collect();
        if !invalid.is_empty() {
            return Err(crate::error_handling::utils::registry_error_with_package(
            "validate_packages",
            &invalid.join(", "),
            None,
            "Package(s) not found or invalid"
        ));
        }
    }

    // Install from cache: link from unpacked store, or fall back to backend/copy
    if !to_install_from_cache.is_empty() {
        let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
        let node_modules = Path::new("node_modules");
        std::fs::create_dir_all(node_modules).map_err(|e| crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e))?;
        // Create .bin directory for compatibility with npm
        std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| crate::error_handling::utils::io_error("create_bin_dir", Some("node_modules/.bin"), e))?;
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
                                    || selective_extract::extract_selective(&tarball_path, &node_modules, &base).is_ok()
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
                                || selective_extract::extract_selective(&tarball_path, &node_modules, &base).is_ok()
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
            let fallback_pkgs: Vec<String> = fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
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
                    let pkgs: Vec<String> = fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
                    check_script_allowlist(&pkgs, allowlist)?;
                }
            }
            let paths: Vec<std::path::PathBuf> = fallback_tarballs.iter().map(|(_, p)| p.clone()).collect();
            match backend::backend_install_tarballs(&paths, options.backend, options.no_scripts) {
                Ok(()) => {
                    for (pkg, _) in &fallback_tarballs {
                        if !options.quiet {
                            utils::log(&format!("Installed {} from cache (backend).", pkg));
                        }
                    }
                }
                Err(e) => return Err(crate::error_handling::utils::application_error(
                    "backend_install_tarballs",
                    Some("install_failed"),
                    &e
                )),
            }
        }
    }
    profiler.mark("install_from_cache");

    if to_fetch.is_empty() {
        sync_hidden_lockfile();
        return Ok(());
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e))?;

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
                        let pkg_key = exact_version_from_spec(&pkg)
                            .map(|v| format!("{}@{}", base_name(&pkg), v))
                            .unwrap_or_else(|| pkg.clone());
                        let res = registry::download_tarball_to_store_hash_only(
                            &url,
                            &cache_dir,
                            &pkg_key,
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
                let pkg_key = exact_version_from_spec(&pkg)
                    .map(|v| format!("{}@{}", base_name(&pkg), v))
                    .unwrap_or_else(|| pkg.clone());
                let res = registry::download_tarball_to_store_hash_only(
                    &url,
                    &cache_dir,
                    &pkg_key,
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
                        if let Err(e) = selective_extract::extract_selective(&store_path, node_modules, base) {
                            let msg = format!("Extract failed for {}: {}", pkg, e);
                            utils::log(&msg);
                            npm_fallback.push(pkg);
                            continue;
                        }
                    }
                    let _ = bin_links::link_bins_for_package(node_modules, base);
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
            utils::write_store_index(&index).map_err(|e| crate::error_handling::utils::io_error("write_store_index", Some("cache/index.json"), std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
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
                            let pkg_key = format!("{}@{}", base_name(&pkg), version);
                            let res = registry::download_tarball_to_store_hash_only(
                                &url,
                                &cache_dir,
                                &pkg_key,
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
                    let pkg_key = format!("{}@{}", base_name(&pkg), version);
                    let res = registry::download_tarball_to_store_hash_only(
                        &url,
                        &cache_dir,
                        &pkg_key,
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
                            || selective_extract::extract_selective(&store_path, node_modules, base).is_ok()
                        {
                            let _ = bin_links::link_bins_for_package(node_modules, base);
                            if !options.quiet {
                                crate::utils::log(&format!("Installed {}@{} (native)", base, version));
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
                utils::write_store_index(&index).map_err(|e| crate::error_handling::utils::io_error("write_store_index", Some("cache/index.json"), std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
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
        sync_hidden_lockfile();
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
        let mut offline_cache = crate::offline_cache::OfflineCache::new(
            std::path::PathBuf::from(utils::get_cache_dir())
        );
        
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
                sync_hidden_lockfile();
                return Ok(());
            }
            Err(e) => {
                if attempts <= 1 {
                    return Err(crate::error_handling::utils::application_error(
                        "backend_install",
                        Some("install_failed"),
                        &e
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
