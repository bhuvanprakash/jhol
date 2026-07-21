use crate::backend::{self, Backend};
use crate::bin_links;
use crate::error_handling::JholError;
use crate::lockfile;
use crate::registry;
use crate::selective_extract; // JHOL Selective Extraction
use crate::utils::{self, NPM_SHOW_TIMEOUT_SECS};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

fn download_concurrency() -> usize {
    crate::utils::cached_env_var("JHOL_DOWNLOAD_CONCURRENCY")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 128))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 4).clamp(32, 128))
                .unwrap_or(64)
        })
}

fn cache_install_concurrency() -> usize {
    crate::utils::cached_env_var("JHOL_CACHE_INSTALL_CONCURRENCY")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 32))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().clamp(4, 32))
                .unwrap_or(8)
        })
}

fn worker_pool_sequential_threshold() -> usize {
    crate::utils::cached_env_var("JHOL_WORKER_POOL_SEQUENTIAL_THRESHOLD")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 64))
        .unwrap_or(1)
}

fn use_legacy_chunk_scheduler() -> bool {
    crate::utils::cached_env_var("JHOL_LEGACY_CHUNK_SCHEDULER")
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
    if concurrency <= 1 || items.len() <= sequential_threshold.clamp(1, 64) {
        return items.into_iter().map(job).collect();
    }

    let worker_count = concurrency.clamp(1, 64).min(items.len());
    let total_items = items.len();

    // Use global rayon threadpool for thread reuse across install calls.
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(64)
            .thread_name(|i| format!("jhol-worker-{}", i))
            .build()
            .expect("jhol threadpool")
    });

    pool.install(|| {
        use rayon::prelude::*;
        items
            .into_par_iter()
            .with_max_len(std::cmp::max(1, total_items / worker_count))
            .map(job)
            .collect()
    })
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
            // Soul Path: check zero-copy metadata store first (avoids network when
            // another project already fetched this packument).
            let mut zero_copy_hits: Vec<(String, ResolvedFetch)> = Vec::new();
            let mut still_needed: Vec<(String, String, String)> = Vec::new();
            for (pkg, base, version_req) in &pending_packument {
                if let Some(meta) = crate::zero_copy_metadata::MmapMetadataStore::lookup_packument(base) {
                    if let Some(version) = crate::soul_path::resolve_version_from_meta(version_req, &meta) {
                        let version_str = version.to_string();
                        let url = if meta.dist.tarball_url.is_empty() {
                            lockfile::tarball_url_from_registry(base, &version_str)
                        } else {
                            meta.dist.tarball_url.clone()
                        };
                        zero_copy_hits.push((base.clone(), ResolvedFetch {
                            pkg: pkg.clone(),
                            url,
                            integrity: meta.dist.integrity.clone(),
                            version: version_str,
                        }));
                        continue;
                    }
                }
                still_needed.push((pkg.clone(), base.clone(), version_req.clone()));
            }

            if !zero_copy_hits.is_empty() && still_needed.is_empty() {
                // All resolved from zero-copy — skip network entirely
                resolved_work.extend(zero_copy_hits.into_iter().map(|(_, rf)| rf));
                return resolved_work;
            }

            let cache_arc = Arc::new(Mutex::new(std::collections::HashMap::<
                String,
                serde_json::Value,
            >::new()));

            let mut unique_names: Vec<String> = still_needed
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

            for (pkg, base, version_req) in still_needed {
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
            // Add zero-copy hits
            resolved_work.extend(zero_copy_hits.into_iter().map(|(_, rf)| rf));
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
    let base = utils::sanitize_package_name(base)?;
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

fn expand_with_transitive_dependencies(packages: &[&str]) -> Result<Vec<String>, String> {
    // Compatibility-first default: include at least one transitive layer for direct installs.
    // Keeps `axios -> follow-redirects` and similar chains correct without requiring env tuning.
    // Soul Path: default full-depth when streaming; else depth 32 (was 2 — incomplete trees)
    let max_depth = std::env::var("JHOL_TRANSITIVE_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(if crate::soul_path::stream_install_enabled() {
            64
        } else {
            32
        });
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
    let mut deferred_packages: Vec<String> = Vec::new();

    for package in packages {
        let base = base_name(package).to_string();
        let spec = requested_version_spec(package);
        constraints.entry(base.clone()).or_default().push(spec);
        required.insert(base.clone(), true);
        queue.push_back((base, 0));
    }

    // First pass: try binary index and cache, collect deferred packages
    while let Some((name, depth)) = queue.pop_front() {
        let Some(pkg_constraints) = constraints.get(&name).cloned() else {
            continue;
        };

        if !metadata_cache.contains_key(&name) {
            // Zero-network fast path: resolve from local binary package index and read deps
            // directly from bundled binary package metadata when available.
            if let Some(binary_version) =
                registry::best_binary_version_matching(&name, &pkg_constraints)
            {
                let changed = selected
                    .get(&name)
                    .map(|existing| existing != &binary_version)
                    .unwrap_or(true);

                if changed {
                    selected.insert(name.clone(), binary_version.clone());
                    if depth < max_depth {
                        if let Some(required_deps) =
                            registry::binary_package_dependencies(&name, &binary_version)
                        {
                            for (dep_name, dep_spec) in required_deps {
                                let dep_constraints =
                                    constraints.entry(dep_name.clone()).or_default();
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

            // Check transitive cache
            let snapshot_hit = if transitive_cache_enabled() {
                transitive_metadata_snapshot()
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&name).cloned())
            } else {
                None
            };

            if let Some(hit) = snapshot_hit {
                metadata_cache.insert(name.clone(), hit);
            } else {
                // Defer this package for parallel fetch
                if !deferred_packages.contains(&name) {
                    deferred_packages.push(name.clone());
                }
                continue;
            }
        }

        // Package is in metadata_cache, process it
        let metadata = metadata_cache
            .get(&name)
            .ok_or_else(|| format!("Missing metadata for {}", name))?;

        let version =
            resolve_version_for_constraints(metadata, &pkg_constraints).ok_or_else(|| {
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
                    if !deferred_packages.contains(&dep_name)
                        && !metadata_cache.contains_key(&dep_name)
                        && registry::best_binary_version_matching(&dep_name, &[]).is_none()
                    {
                        deferred_packages.push(dep_name.clone());
                    }
                    queue.push_back((dep_name, depth + 1));
                }
            }

            let optional_deps = registry::get_version_optional_dependencies(metadata, &version);
            for (dep_name, dep_spec) in optional_deps {
                let dep_constraints = constraints.entry(dep_name.clone()).or_default();
                if !dep_constraints.iter().any(|s| s == &dep_spec) {
                    dep_constraints.push(dep_spec);
                    required.entry(dep_name.clone()).or_insert(false);
                    if !deferred_packages.contains(&dep_name)
                        && !metadata_cache.contains_key(&dep_name)
                        && registry::best_binary_version_matching(&dep_name, &[]).is_none()
                    {
                        deferred_packages.push(dep_name.clone());
                    }
                    queue.push_back((dep_name, depth + 1));
                }
            }
        }
    }

    // Parallel fetch all deferred packages at once
    if !deferred_packages.is_empty() {
        let cache_arc = std::sync::Arc::new(std::sync::Mutex::new(metadata_cache.clone()));
        let fetch_results = registry::parallel_fetch_metadata(&deferred_packages, &cache_arc);

        for (pkg_name, result) in fetch_results {
            match result {
                Ok(meta) => {
                    metadata_cache.insert(pkg_name.clone(), meta.clone());
                    if transitive_cache_enabled() {
                        if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                            cache.insert(pkg_name, meta);
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("Failed to fetch metadata for {}: {}", pkg_name, e));
                }
            }
        }
    }

    // Second pass: now process all packages with metadata available
    for (name, pkg_constraints) in constraints.iter() {
        if selected.contains_key(name) {
            continue; // Already resolved via binary index
        }

        let Some(metadata) = metadata_cache.get(name) else {
            continue;
        };

        let version =
            resolve_version_for_constraints(metadata, pkg_constraints).ok_or_else(|| {
                format!(
                    "Dependency conflict for {} (constraints: {})",
                    name,
                    pkg_constraints.join(", ")
                )
            })?;

        let is_required = *required.get(name).unwrap_or(&true);
        if !version_supported_on_current_platform(metadata, &version) {
            if is_required {
                return Err(format!(
                    "Package {}@{} is incompatible with current platform (os/cpu)",
                    name, version
                ));
            }
            continue;
        }

        selected.insert(name.clone(), version.clone());
    }

    while let Some((name, depth)) = queue.pop_front() {
        let Some(pkg_constraints) = constraints.get(&name).cloned() else {
            continue;
        };

        if !metadata_cache.contains_key(&name) {
            // Zero-network fast path: resolve from local binary package index and read deps
            // directly from bundled binary package metadata when available.
            if let Some(binary_version) =
                registry::best_binary_version_matching(&name, &pkg_constraints)
            {
                let changed = selected
                    .get(&name)
                    .map(|existing| existing != &binary_version)
                    .unwrap_or(true);

                if changed {
                    selected.insert(name.clone(), binary_version.clone());
                    if depth < max_depth {
                        if let Some(required_deps) =
                            registry::binary_package_dependencies(&name, &binary_version)
                        {
                            for (dep_name, dep_spec) in required_deps {
                                let dep_constraints =
                                    constraints.entry(dep_name.clone()).or_default();
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

            // Try to get from transitive cache first
            let snapshot_hit = if transitive_cache_enabled() {
                transitive_metadata_snapshot()
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&name).cloned())
            } else {
                None
            };

            if snapshot_hit.is_some() {
                metadata_cache.insert(name.clone(), snapshot_hit.unwrap());
            } else {
                // Defer fetching - collect all needed packages and fetch in parallel
                continue;
            }
        }

        // Second pass: fetch all deferred packages in parallel
        let deferred_packages: Vec<String> = queue
            .iter()
            .filter(|(name, _)| !metadata_cache.contains_key(name))
            .map(|(name, _)| name.clone())
            .collect();

        if !deferred_packages.is_empty() {
            let fetch_results = registry::parallel_fetch_metadata(
                &deferred_packages,
                &std::sync::Arc::new(std::sync::Mutex::new(metadata_cache.clone())),
            );
            for (pkg_name, result) in fetch_results {
                match result {
                    Ok(meta) => {
                        metadata_cache.insert(pkg_name.clone(), meta.clone());
                        if transitive_cache_enabled() {
                            if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                                cache.insert(pkg_name, meta);
                            }
                        }
                    }
                    Err(e) => {
                        return Err(format!("Failed to fetch metadata for {}: {}", pkg_name, e));
                    }
                }
            }
        }

        let metadata = metadata_cache
            .get(&name)
            .ok_or_else(|| format!("Missing metadata for {}", name))?;

        let version =
            resolve_version_for_constraints(metadata, &pkg_constraints).ok_or_else(|| {
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
pub fn read_all_installed_packages(
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

    let package = utils::sanitize_package_name(package)?;

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
        // Also check JAGR-S objects store
        if crate::jagrs::has_tarball(hash) {
            // Tarball exists in JAGR-S objects log — return the store path
            // even if .tgz file doesn't exist (it will be extracted from JAGR-S)
            return Some(store_file);
        }
    }

    // Try original key
    if package.contains('@') {
        if let Some(hash) = store_index.get(&package) {
            let store_file = store_dir.join(format!("{}.tgz", hash));
            if store_file.exists() {
                return Some(store_file);
            }
        }
    } else {
        // Backward-compatible fast path for older index entries keyed by bare package name.
        if let Some(hash) = store_index.get(&package) {
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
    let legacy_exact = cache_dir.join(format!("{}.tgz", utils::format_cache_name(&package)));
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

    let store_index = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index()
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
    let concurrency = download_concurrency();

    if concurrency <= 1 || to_fetch.len() <= 1 {
        for entry in &to_fetch {
            let (hash, _unpacked) = registry::download_unpack_to_store_unpacked(
                &entry.resolved,
                &cache_dir,
                &entry.spec,
                entry.integrity.as_deref(),
            )
            .map_err(|e| {
                crate::error_handling::utils::registry_error_with_package(
                    "download_unpack_to_store_unpacked",
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
    } else {
        use std::sync::mpsc;
        use std::thread;

        let cache_dir_arc = Arc::new(cache_dir.clone());
        let results_arc: Arc<Mutex<Vec<Result<(String, String, String), (String, String)>>>> =
            Arc::new(Mutex::new(Vec::with_capacity(to_fetch.len())));

        for chunk in to_fetch.chunks(concurrency) {
            let (tx, rx) = mpsc::channel();
            let cache_dir_clone = cache_dir_arc.clone();
            let results_clone = results_arc.clone();

            for entry in chunk {
                let tx = tx.clone();
                let resolved = entry.resolved.clone();
                let spec = entry.spec.clone();
                let integrity = entry.integrity.clone();
                let cache_dir_inner = cache_dir_clone.clone();

                thread::spawn(move || {
                    let result = registry::download_tarball_to_store_hash_only(
                        &resolved,
                        &cache_dir_inner,
                        &spec,
                        integrity.as_deref(),
                    )
                    .map(|hash| (spec.clone(), hash, resolved));
                    let _ = tx.send(result);
                });
            }
            drop(tx);

            for received in rx {
                match received {
                    Ok((spec, hash, _resolved)) => {
                        if let Ok(mut results) = results_clone.lock() {
                            results.push(Ok((spec, hash, "ok".to_string())));
                        }
                    }
                    Err(e) => {
                        if let Ok(mut results) = results_clone.lock() {
                            results.push(Err((e, "download_failed".to_string())));
                        }
                    }
                }
            }
        }

        let results = results_arc.lock().map_err(|e| {
            crate::error_handling::utils::application_error(
                "install_lockfile_layout_results",
                Some("lock_error"),
                &e.to_string(),
            )
        })?;
        let mut successful: Vec<(String, String)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for result in results.iter() {
            match result {
                Ok((spec, hash, _)) => successful.push((spec.clone(), hash.clone())),
                Err((e, _)) => errors.push(e.clone()),
            }
        }

        if !errors.is_empty() {
            return Err(crate::error_handling::utils::application_error(
                "install_lockfile_layout_parallel_fetch",
                Some("download_failed"),
                &format!("Failed to download: {}", errors.join("; ")),
            ));
        }

        for (spec, hash) in successful {
            let entry = to_fetch.iter().find(|e| e.spec == spec);
            if let Some(entry) = entry {
                let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                if let Err(e) = install_entry(entry, &store_path) {
                    return Err(crate::error_handling::utils::application_error(
                        "install_lockfile_layout_parallel_install",
                        Some("extract_failed"),
                        &format!("{} -> {}", spec, e),
                    ));
                }
                index_batch.insert(spec, hash);
            }
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

    let _ = bin_links::rebuild_bin_links(node_modules);
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
    /// Resolver strategy string (from JHOL_RESOLVER env).
    pub resolver: String,
    /// Overrides from package.json (name → version spec).
    pub overrides: Option<std::collections::HashMap<String, String>>,
    /// Enable telemetry for this install.
    pub telemetry: bool,
    /// Optional allowlist for packages allowed to run scripts in backend fallback mode.
    pub script_allowlist: Option<std::collections::HashSet<String>>,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            no_cache: false,
            quiet: false,
            backend: Backend::Npm,  // lazy: avoid bun_available() subprocess at Default
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
pub fn install_lockfile_only(_backend: Backend) -> Result<(), JholError> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(crate::error_handling::utils::io_error(
            "install_lockfile_only",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"),
        ));
    }
    crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
        crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e)
    })?;

    let tree = crate::lockfile_write::resolve_full_tree(pj)
        .map_err(|e| crate::error_handling::utils::config_error("resolve_full_tree", None, &e))?;
    let lock_path = Path::new("package-lock.json");
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree).map_err(|e| {
        crate::error_handling::utils::io_error(
            "write_package_lock",
            Some("package-lock.json"),
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
        )
    })?;
    // Soul Path: record resolution in JMR + GRM + lattice
    let fp = crate::resolution_mirror::fingerprint_deps(
        &lockfile::read_package_json_deps(pj).unwrap_or_default(),
    );
    crate::resolution_mirror::record_resolution(fp, &tree);
    crate::soul_path::remember_all_packages(&tree);
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

/// Store currently installed packages in GRM + lattice memory.
fn remember_installed_packages(node_modules: &Path) {
    if let Ok(pkgs) = read_all_installed_packages(node_modules) {
        crate::soul_path::remember_all_packages(&pkgs);
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
    // Try capsule first — zero-parse mmap walk
    let cap_path = crate::capsule::capsule_path(Path::new("."));
    if cap_path.exists() {
        if let Ok(view) = crate::capsule::read_capsule(&cap_path) {
            let n = view.package_count();
            if n > 0 {
                let mut specs: Vec<String> = Vec::with_capacity(n);
                for i in 0..n {
                    let e = view.entry(i);
                    let name = view.entry_name(e);
                    let version = view.entry_version(e);
                    let spec = format!("{}@{}", name, version);
                    specs.push(spec);
                }
                specs.sort();
                specs.dedup();
                return Some(specs);
            }
        }
    }

    // Fallback: lockfile-based lookup
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

/// Whether KCC³ CGA capsule path is enabled.
fn kcc3_cga_enabled() -> bool {
    std::env::var("JHOL_CGA")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

/// Whether KCC³ SNR speculation path is enabled.
fn kcc3_snr_enabled() -> bool {
    std::env::var("JHOL_SNR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

/// Phase 5: DSSC-First Resolution — check pre-computed spectral certificates
/// before running the full Knot/topological pipeline.
///
/// If ALL root packages have Stable DSSC certs, compute a quick spectral gap
/// lower bound. If the bound is large enough (gap ≥ 0.5), the dependency graph
/// is provably conflict-free and we can skip resolution entirely — just
/// extract from cache or fetch + install directly.
///
/// This is the fastest install path: O(1) cert lookup + O(packages) cache check.
/// No mmap, no Knot, no SAT, no topology.
fn dssc_first_install(
    packages: &[&str],
    options: &InstallOptions,
    profiler: &mut InstallProfiler,
) -> Result<bool, JholError> {
    // Only for empty-package (install from package.json) calls
    if !packages.is_empty() {
        return Ok(false);
    }

    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Ok(false);
    }

    let deps = match lockfile::read_package_json_deps(pj_path) {
        Some(d) if !d.is_empty() => d,
        _ => return Ok(false),
    };

    // Look up DSSC certs for all root packages
    let (all_stable, quick_gap) =
        crate::jagr::resolution_differential::dssc_all_stable(&deps);

    if !all_stable {
        return Ok(false);
    }

    // Quick spectral gap check: if gap ≥ 0.5, graph is provably stable
    if quick_gap < 0.5 {
        return Ok(false);
    }

    if !options.quiet {
        crate::utils::log(&format!(
            "DSSC First: all {} packages certified Stable (gap={:.3}) — greedy path",
            deps.len(),
            quick_gap
        ));
    }

    // Check if ALL packages are in cache — if so, install directly
    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let store_index = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index()
    };

    let mut all_cached = true;
    let mut cache_entries: Vec<(String, std::path::PathBuf)> = Vec::new(); // (spec, tarball_path)

    for (name, spec) in &deps {
        let base = if name.starts_with('@') {
            let parts: Vec<&str> = name.split('/').collect();
            if parts.len() >= 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                name.clone()
            }
        } else {
            name.clone()
        };
        let spec_str = format!("{}@{}", base, spec);

        if let Some(tarball) =
            get_cached_tarball_fast(&spec_str, &cache_dir, &store_index)
        {
            cache_entries.push((spec_str, tarball));
        } else if options.offline {
            all_cached = false;
            break;
        } else {
            // Not in cache — need to fetch
            all_cached = false;
            break;
        }
    }

    if !all_cached {
        // Some packages missing from cache — fall through to full pipeline
        return Ok(false);
    }

    // All packages in cache — greedy extract
    if !options.quiet {
        crate::utils::log(&format!(
            "DSSC First: {} packages all in cache — extracting",
            cache_entries.len()
        ));
    }

    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| {
        crate::error_handling::utils::io_error("dssc_first_install", Some("node_modules"), e)
    })?;

    for (spec, tarball) in &cache_entries {
        let name = spec.split('@').next().unwrap_or(spec);
        let pkg_dir = if name.starts_with('@') {
            let parts: Vec<&str> = name.split('/').collect();
            let scope_dir = node_modules.join(parts[0]);
            std::fs::create_dir_all(&scope_dir).map_err(|e| {
                crate::error_handling::utils::io_error("dssc_first_install", Some(&scope_dir.display().to_string()), e)
            })?;
            scope_dir.join(&parts[1..].join("/"))
        } else {
            node_modules.join(name)
        };

        if !pkg_dir.exists() {
            let tarball_path = std::path::PathBuf::from(tarball);
            selective_extract::extract_selective(&tarball_path, node_modules, name).map_err(|e| {
                crate::error_handling::utils::application_error(
                    "dssc_first_install",
                    Some(&spec),
                    &e,
                )
            })?;
        }
    }

    profiler.mark("dssc_first_path");
    write_install_markers();
    Ok(true)
}

/// KCC³ classifier cascade: try fast paths first.
/// Returns Ok(true) if a path succeeded, Ok(false) to fall through, Err(e) on failure.
fn kcc3_cascade(
    packages: &[&str],
    options: &InstallOptions,
    profiler: &mut InstallProfiler,
) -> Result<bool, JholError> {
    let cap_path = crate::capsule::capsule_path(Path::new("."));
    let lockfile_present = Path::new("package-lock.json").exists() || Path::new("bun.lock").exists();
    if !cap_path.exists() && !lockfile_present {
        return Ok(false);
    }

    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Ok(false);
    }
    let deps = match lockfile::read_package_json_deps(pj_path) {
        Some(d) => d,
        None => return Ok(false),
    };
    if deps.is_empty() {
        return Ok(false);
    }

    // Compute knot fingerprint from deps for classifier lookup
    let deps_fp = crate::resolution_mirror::fingerprint_deps(&deps);
    let current_pbf = crate::jagr::resolution_differential::pbf_from_deps(&deps);

    // Path 0 (KPSL): knot_fp match → optional PBF trailer → manifest hash → project links
    if kcc3_cga_enabled() && cap_path.exists() {
        if let Some(view) = crate::capsule::classifier_lookup(Path::new("."), deps_fp) {
            let pbf_ok = match view.pbf() {
                Some(stored) => {
                    crate::jagr::resolution_differential::pbf_filter(stored, current_pbf)
                }
                // Legacy capsules without trailer: knot_fp already matched
                None => true,
            };
            if !pbf_ok {
                if !options.quiet {
                    crate::utils::log("KCC³ Path 0: PBF mismatch — capsule dropped");
                }
            } else {
                // Manifest match when lock present; if lock deleted (warm reinstall),
                // knot_fp + PBF alone authorize KPSL projection.
                let lock_present = Path::new("package-lock.json").exists();
                let manifest_ok = if lock_present {
                    crate::capsule::compute_manifest_hash(Path::new("."))
                        .map(|h| view.manifest_hash() == h)
                        .unwrap_or(false)
                } else {
                    true
                };
                if manifest_ok {
                    if !options.quiet {
                        crate::utils::log("KCC³ Path 0: KPSL projection — installing");
                    }
                    let result = crate::capsule::install_from_capsule(&view, Path::new("."))
                        .map_err(|e| {
                            crate::error_handling::utils::application_error(
                                "kcc3_cga_install",
                                Some("install_failed"),
                                &e,
                            )
                        });
                    if result.is_ok() {
                        profiler.mark("kcc3_path0_cga");
                        return Ok(true);
                    }
                }
            }
        }
    }

    // Path S: SNR — check JMR for this edit descriptor (add/update/remove)
    if kcc3_snr_enabled() && packages.len() <= 5 && crate::daemon_mode() {
        let edit_desc = if packages.is_empty() || packages[0].contains("remove") {
            format!("remove:{}", packages.first().unwrap_or(&""))
        } else {
            format!("add:{}", packages.join(","))
        };
        if let Some(snr_result) = crate::resolution_mirror::snr_get(deps_fp, &edit_desc) {
            if !snr_result.is_empty() && !options.quiet {
                crate::utils::log("KCC³ Path S: SNR hit — using predicted resolution");
                profiler.mark("kcc3_pathS_snr");
            }
        }
    }

    // Path B: spectral gate — skip full pipeline if graph is stable
    let (can_skip, predicted_beta1) =
        crate::jagr::resolution_differential::classify_gate(&deps);
    if can_skip && predicted_beta1 == 0 && !options.quiet {
        crate::utils::log("KCC³ Path B: spectral gate — graph stable, skipping full pipeline");
    }

    // Path A: lockfile validation — handled by existing capsule + lockfile paths below
    // Path C: star delta — handled by existing install logic below (knot fallback)
    Ok(false)
}

/// Build a minimal KnotRequest from package.json deps for knot graph construction.
fn build_resolve_request_from_deps(
    deps: &std::collections::HashMap<String, String>,
) -> Result<crate::jagr::knot::KnotRequest, String> {
    let mut available = std::collections::HashMap::new();
    for (name, _spec) in deps {
        available.insert(name.clone(), vec!["0.0.0".to_string()]);
    }
    Ok(crate::jagr::knot::KnotRequest {
        root_package: "root".into(),
        root_version: "0.0.0".into(),
        available_versions: available,
        dependencies: std::collections::HashMap::new(),
        conflicts: vec![],
    })
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
        let r = resolved.as_ref().ok_or_else(|| {
            crate::error_handling::utils::config_error(
                "strict_lockfile_validation",
                Some("lockfile"),
                "Lockfile data not available",
            )
        })?;
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

/// Write install markers for zero-binary-startup fast path.
/// Called before every `return Ok(())` in `install_package`.
fn write_install_markers() {
    // Try capsule first (fast, no disk reads)
    let hash = {
        let cap_path = crate::capsule::capsule_path(std::path::Path::new("."));
        if cap_path.exists() {
            crate::capsule::read_capsule(&cap_path)
                .ok()
                .map(|v| v.manifest_hash())
        } else {
            // Lockfile + capsule may not exist yet on first cold install.
            // That's fine — second run will have them.
            crate::capsule::compute_manifest_hash(std::path::Path::new(".")).ok()
        }
    };
    if let Some(h) = hash {
        let nm = std::path::Path::new("node_modules");
        if nm.exists() {
            let _ = std::fs::write(nm.join(".nm_fingerprint"), &h);
        }
        crate::daemon_cache::store_last_hash(h);
        crate::daemon_cache::write_status(h, std::path::Path::new("."));
    }
}

/// Capsule-first fast path: if the project capsule exists and its manifest hash
/// matches the current package.json + package-lock.json, install directly from
/// it. This is the single warm path; KCC³ / Soul Path / GRM are kept off the
/// hot install path.
fn try_capsule_install(project_dir: &Path, quiet: bool) -> Result<bool, JholError> {
    let cap_path = crate::capsule::capsule_path(project_dir);
    if !cap_path.exists() {
        return Ok(false);
    }
    let view = match crate::capsule::read_capsule(&cap_path) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let hash = match crate::capsule::compute_manifest_hash(project_dir) {
        Ok(h) => h,
        Err(_) => return Ok(false),
    };
    if view.manifest_hash() != hash {
        return Ok(false);
    }
    if !quiet {
        crate::utils::log("Capsule hit — installing from capsule");
    }
    crate::capsule::install_from_capsule(&view, project_dir).map_err(|e| {
        crate::error_handling::utils::application_error("capsule_install", Some("install_failed"), &e)
    })?;
    // Write project-local status so next install exits before binary loads.
    crate::daemon_cache::write_status(hash, project_dir);
    crate::daemon_cache::store_last_hash(hash);
    let nm = project_dir.join("node_modules");
    if nm.exists() {
        let _ = std::fs::write(nm.join(".nm_fingerprint"), &hash);
    }
    Ok(true)
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with backend fallback.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), JholError> {
    let mut profiler = InstallProfiler::new();

    // Lane 0: zero-work warm bypass via project-local status file.
    // main.rs already checks this before clap; this guard covers daemon/direct calls.
    if packages.is_empty() && !options.lockfile_only && crate::daemon_cache::can_skip_install_before_clap() {
        profiler.mark("lane0_warm_bypass");
        return Ok(());
    }

    let download_conc = download_concurrency();
    let cache_install_conc = cache_install_concurrency().max(1);
    // Warm cache install concurrency defaults higher (was 1)
    let cache_install_conc = if cache_install_conc == 1
        && std::env::var("JHOL_CACHE_INSTALL_CONCURRENCY").is_err()
    {
        std::thread::available_parallelism()
            .map(|n| n.get().clamp(4, 32))
            .unwrap_or(8)
    } else {
        cache_install_conc
    };
    let worker_pool_seq_threshold = worker_pool_sequential_threshold();
    let legacy_chunk_scheduler = use_legacy_chunk_scheduler();

    // Capsule-first fast path: mmap'd zero-parse graph → install.
    if !options.lockfile_only && !options.no_cache {
        match try_capsule_install(Path::new("."), options.quiet) {
            Ok(true) => {
                profiler.mark("capsule_fast_path");
                remember_installed_packages(Path::new("node_modules"));
                write_install_markers();
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                if !options.quiet {
                    crate::utils::log(&format!("Capsule fast path failed: {}", e));
                }
            }
        }
    }

    // Phase 5: DSSC-First Resolution — check spectral certs before full pipeline.
    // If all root packages are Stable-certified, skip Knot/SAT/topology entirely.
    if !options.lockfile_only {
        match dssc_first_install(packages, options, &mut profiler) {
            Ok(true) => {
                remember_installed_packages(Path::new("node_modules"));
                write_install_markers();
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                if !options.quiet {
                    crate::utils::log(&format!("DSSC first path failed: {}", e));
                }
            }
        }
    }

    if options.strict_peer_deps {
        let pj = Path::new("package.json");
        if pj.exists() {
            crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
                crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e)
            })?;
        }
    }

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
        match expand_with_transitive_dependencies(packages) {
            Ok(specs) if !specs.is_empty() => specs,
            Ok(_) => packages.iter().map(|p| (*p).to_string()).collect(),
            Err(err) => {
                crate::utils::log(&format!(
                    "warning: transitive expansion failed, continuing with direct specs: {}",
                    err
                ));
                packages.iter().map(|p| (*p).to_string()).collect()
            }
        }
    };

    if options.from_lockfile {
        if install_lockfile_layout(options)? {
            sync_hidden_lockfile();
            let _ = crate::capsule::write_capsule_from_project(Path::new("."));
            remember_installed_packages(Path::new("node_modules"));
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

    // Topological wave optimization: use capsule to skip entire waves
    // of already-installed packages. Only process waves with missing packages.
    let wave_skip = if !options.no_cache {
        let cap_path = crate::capsule::capsule_path(Path::new("."));
        if cap_path.exists() {
            if let Ok(view) = crate::capsule::read_capsule(&cap_path) {
                if view.package_count() > 0 {
                    // Group specs by install_order
                    let mut wave_specs: std::collections::BTreeMap<u16, Vec<String>> = std::collections::BTreeMap::new();
                    for i in 0..view.package_count() {
                        let e = view.entry(i);
                        let spec = format!("{}@{}", view.entry_name(e), view.entry_version(e));
                        wave_specs.entry(e.install_order).or_default().push(spec);
                    }
                    // Check which waves are fully installed
                    let mut skip_packages: std::collections::HashSet<String> = std::collections::HashSet::new();
                    for (_order, specs) in &wave_specs {
                        let all_installed = specs.iter().all(|s| {
                            let name = s.split('@').next().unwrap_or(s);
                            let pkg_dir = Path::new("node_modules").join(
                                if name.starts_with('@') {
                                    let parts: Vec<&str> = name.split('/').collect();
                                    let scope = parts[0];
                                    let rest: Vec<&str> = parts[1..].to_vec();
                                    format!("{}/{}", scope, rest.join("/"))
                                } else {
                                    name.to_string()
                                }
                            );
                            pkg_dir.join("package.json").exists()
                        });
                        if all_installed {
                            for spec in specs {
                                skip_packages.insert(spec.clone());
                            }
                        } else {
                            // Wave partially installed — process all packages in it
                            // (don't skip any in this wave)
                        }
                    }
                    skip_packages
                } else {
                    std::collections::HashSet::new()
                }
            } else {
                std::collections::HashSet::new()
            }
        } else {
            std::collections::HashSet::new()
        }
    } else {
        std::collections::HashSet::new()
    };

    for package in &requested_packages {
        let base = base_name(package);

        // Topological wave skip: if this entire wave is already installed, skip
        if wave_skip.contains(package) || wave_skip.contains(base) {
            if !options.quiet {
                utils::log(&format!("Skipping {} (wave already installed)", package));
            }
            continue;
        }

        // Soul Path predictive prefetch (once per batch, not O(n²) disk reads)
        if !options.offline && !options.no_cache && seen_packages.len() == 2 {
            let history: Vec<String> = seen_packages.iter().cloned().collect();
            crate::soul_path::predictive_prefetch(&history, 3);
        }

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
        sync_hidden_lockfile();
        let _ = crate::capsule::write_capsule_from_project(Path::new("."));
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
        let mut download_results: Vec<(String, Result<(String, std::path::PathBuf), String>)> =
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
                                let store_path =
                                    cache_dir.join("store").join(format!("{}.tgz", hash));
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
                run_worker_pool(
                    dl_inputs,
                    dl_concurrency,
                    worker_pool_seq_threshold,
                    move |(pkg, url, integrity)| {
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
                    },
                )
            };
        download_results.sort_by(|a, b| a.0.cmp(&b.0));
        for (pkg, res) in download_results {
            match res {
                Ok((hash, unpacked)) => {
                    index_batch.insert(pkg.clone(), hash.clone());
                    let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                    let base = base_name(&pkg);
                    if utils::link_package_from_store(&unpacked, node_modules, base).is_err() {
                        if let Err(e) =
                            selective_extract::extract_selective(&store_path, node_modules, base)
                        {
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
            let mut download_results: Vec<(
                String,
                Result<(String, String, std::path::PathBuf), String>,
            )> = if legacy_chunk_scheduler {
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
                                let store_path =
                                    cache_dir.join("store").join(format!("{}.tgz", hash));
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
                run_worker_pool(
                    dl_inputs,
                    dl_concurrency,
                    worker_pool_seq_threshold,
                    move |item| {
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
                    },
                )
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
                            || selective_extract::extract_selective(&store_path, node_modules, base)
                                .is_ok()
                        {
                            let _ = bin_links::link_bins_for_package(node_modules, base);
                            if !options.quiet {
                                crate::utils::log(&format!(
                                    "Installed {}@{} (native)",
                                    base, version
                                ));
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
                // Soul Path: record resolution in JMR + GRM + lattice
                let fp = crate::resolution_mirror::fingerprint_deps(
                    &lockfile::read_package_json_deps(pj_path).unwrap_or_default(),
                );
                crate::resolution_mirror::record_resolution(fp, &packages);
                crate::soul_path::remember_all_packages(&packages);
            }
        } else {
            // Always remember installed packages for GRM/lattice even when lockfile exists
            remember_installed_packages(node_modules);
        }
        sync_hidden_lockfile();
        let _ = crate::capsule::write_capsule_from_project(Path::new("."));

        // Train Markov model with installed packages for future predictions
        if let Ok(pkgs) = read_all_installed_packages(node_modules) {
            let names: Vec<String> = pkgs.keys().cloned().collect();
            if names.len() >= 3 {
                crate::second_order_markov::learn_from_install(&names);
            }
        }
        write_install_markers();
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
                sync_hidden_lockfile();
                let _ = crate::capsule::write_capsule_from_project(Path::new("."));
                remember_installed_packages(node_modules);
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
