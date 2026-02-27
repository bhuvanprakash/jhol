//! Native npm registry client: fetch metadata and tarballs via HTTP.
//! All HTTP calls go through `crate::http_client` (shared Agent = TCP connection pool).
//! Optimized with pre-resolved package index for instant resolution of common packages.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::OnceLock;
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};
use serde_json::Value;

lazy_static::lazy_static! {
    /// Global package index for O(1) resolution
    static ref PACKAGE_INDEX: Arc<Mutex<crate::package_index::PackageIndex>> = {
        let index_path = PathBuf::from(crate::utils::get_cache_dir()).join("package-index");
        Arc::new(Mutex::new(crate::package_index::PackageIndex::new(index_path)))
    };
}

fn registry_url() -> String {
    crate::config::effective_registry_url(Path::new("."))
}

fn registry_auth_token() -> Option<String> {
    crate::config::registry_auth_token(Path::new("."))
}

fn packument_cache_dir() -> PathBuf {
    PathBuf::from(crate::utils::get_cache_dir()).join("packuments")
}

fn packument_cache_key(package: &str, abbreviated: bool) -> String {
    // Use a more efficient cache key that's shorter but still unique
    let mut hasher = Sha256::new();
    hasher.update(package.as_bytes());
    hasher.update(if abbreviated { b"abbr" } else { b"full" });
    // Take only first 16 bytes (32 hex chars) for faster string operations
    let hash_bytes = hasher.finalize();
    hash_bytes[..16].iter().map(|b| format!("{:02x}", b)).collect::<String>()
}

fn packument_cache_paths(package: &str, abbreviated: bool) -> (PathBuf, PathBuf) {
    let key = packument_cache_key(package, abbreviated);
    let dir = packument_cache_dir();
    (dir.join(format!("{}.json", key)), dir.join(format!("{}.etag", key)))
}

fn read_packument_cache(package: &str, abbreviated: bool) -> (Option<Vec<u8>>, Option<String>) {
    let (body_path, etag_path) = packument_cache_paths(package, abbreviated);
    let body = std::fs::read(&body_path).ok();
    let etag = std::fs::read_to_string(&etag_path).ok().map(|s| s.trim().to_string());
    (body, etag)
}

fn write_packument_cache(
    package: &str,
    abbreviated: bool,
    body: &[u8],
    etag: Option<&str>,
) -> Result<(), String> {
    let (body_path, etag_path) = packument_cache_paths(package, abbreviated);
    if let Some(parent) = body_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&body_path, body).map_err(|e| e.to_string())?;
    if let Some(etag) = etag {
        std::fs::write(&etag_path, etag).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Fetch packument bytes via the shared HTTP client (connection pooling).
/// Supports ETags for conditional requests and abbreviated format.
fn fetch_packument_with_etag(package: &str, abbreviated: bool) -> Result<Vec<u8>, String> {
    let path = if package.starts_with('@') {
        package.replace('/', "%2F")
    } else {
        package.to_string()
    };
    let base = registry_url();
    let url = format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'));
    let auth_token = registry_auth_token();
    let (cached_body_raw, cached_etag) = read_packument_cache(package, abbreviated);
    let cached_body = cached_body_raw.filter(|b| !b.is_empty());

    // Build owned strings for header values — &str refs into these live long enough.
    let auth_hdr: Option<String> = auth_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|t| format!("Bearer {}", t));

    /// Build the headers vec for a request (with or without If-None-Match).
    let make_headers = |with_etag: bool| -> Vec<(String, String)> {
        let mut h: Vec<(String, String)> = Vec::new();
        if abbreviated {
            h.push(("Accept".into(), "application/vnd.npm.install-v1+json".into()));
        }
        if let Some(ref v) = auth_hdr {
            h.push(("Authorization".into(), v.clone()));
        }
        if with_etag {
            if let Some(ref etag) = cached_etag {
                if !etag.is_empty() {
                    h.push(("If-None-Match".into(), etag.clone()));
                }
            }
        }
        h
    };

    let do_fetch = |with_etag: bool| -> Result<(u16, Vec<u8>, Option<String>), String> {
        let owned = make_headers(with_etag);
        let refs: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        crate::http_client::get_raw_with_headers(&url, &refs)
    };

    match do_fetch(true) {
        Ok((304, _, _)) => {
            // Server says "not modified" — use disk-cached body if valid.
            if let Some(body) = cached_body {
                if serde_json::from_slice::<serde_json::Value>(&body).is_ok() {
                    return Ok(body);
                }
            }
            // Cache missing/corrupt: unconditional refetch without If-None-Match.
            match do_fetch(false) {
                Ok((_, body, etag)) if !body.is_empty() => {
                    let _ = write_packument_cache(package, abbreviated, &body, etag.as_deref());
                    Ok(body)
                }
                _ => Err(format!("Failed to fetch packument for {} after 304 retry", package)),
            }
        }
        Ok((_, body, etag)) => {
            if body.is_empty() {
                if let Some(cached) = cached_body {
                    if serde_json::from_slice::<serde_json::Value>(&cached).is_ok() {
                        return Ok(cached);
                    }
                }
                return Err(format!("Empty packument body for {}", package));
            }
            if serde_json::from_slice::<serde_json::Value>(&body).is_err() {
                if let Some(cached) = cached_body {
                    if serde_json::from_slice::<serde_json::Value>(&cached).is_ok() {
                        return Ok(cached);
                    }
                }
                return Err(format!("Invalid packument JSON for {}", package));
            }
            let _ = write_packument_cache(package, abbreviated, &body, etag.as_deref());
            Ok(body)
        }
        Err(e) => {
            if let Some(body) = cached_body {
                if serde_json::from_slice::<serde_json::Value>(&body).is_ok() {
                    return Ok(body);
                }
            }
            Err(e)
        }
    }
}

/// Fetch a specific version manifest (/<pkg>/latest or /<pkg>/<version>).
/// Routes through the global shared HTTP client — reuses existing TCP connection.
fn fetch_manifest(package: &str, selector: &str) -> Result<serde_json::Value, String> {
    let path = if package.starts_with('@') {
        package.replace('/', "%2F")
    } else {
        package.to_string()
    };
    let base = registry_url();
    let url = format!(
        "{}/{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/'),
        selector.trim_start_matches('/')
    );
    let auth_token = registry_auth_token();
    // Route through http_client (shared Agent) instead of raw ureq::get().
    let body = crate::http_client::get_bytes_with_bearer(&url, auth_token.as_deref())?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

/// Resolve package from pre-resolved index (O(1) lookup!)
/// Returns None if package not in index (will fetch from network)
fn resolve_from_index(package: &str, _version_req: &str) -> Option<crate::package_index::PreResolvedPackage> {
    let index = PACKAGE_INDEX.lock().ok()?;
    index.lookup(package, _version_req).cloned()
}

/// Add package to index after fetching from network (for future O(1) lookups)
pub fn cache_in_index(package: &str, metadata: &serde_json::Value) {
    if let Some(entry) = crate::package_index::build_index_entry(package, metadata) {
        if let Ok(mut index) = PACKAGE_INDEX.lock() {
            index.add_package(entry);
            // Save user index periodically
            let _ = index.save_user_index();
        }
    }
}

/// Fast-path resolve for latest/exact specs using the manifest endpoint
/// (`/<pkg>/latest` or `/<pkg>/<version>`), avoiding large packument downloads.
/// Returns Some((resolved_version, tarball_url, integrity)).
pub fn resolve_tarball_via_manifest(
    package: &str,
    version_req: &str,
) -> Result<Option<(String, String, Option<String>)>, String> {
    // FIRST: Try pre-resolved package index (O(1) lookup!)
    if let Some(resolved) = resolve_from_index(package, version_req) {
        return Ok(Some((resolved.version, resolved.tarball_url, Some(resolved.integrity))));
    }
    
    // SECOND: Try manifest endpoint
    let selector = if version_req.trim().is_empty() || version_req.trim() == "latest" {
        Some("latest".to_string())
    } else if semver::Version::parse(version_req.trim()).is_ok() {
        Some(version_req.trim().to_string())
    } else {
        None
    };

    let Some(selector) = selector else {
        return Ok(None);
    };

    let manifest = fetch_manifest(package, &selector)?;
    let resolved_version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("No version in manifest for {}@{}", package, selector))?
        .to_string();
    let tarball = manifest
        .get("dist")
        .and_then(|d| d.as_object())
        .and_then(|d| d.get("tarball"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("No dist.tarball in manifest for {}@{}", package, selector))?
        .to_string();
    let integrity = manifest
        .get("dist")
        .and_then(|d| d.as_object())
        .and_then(|d| d.get("integrity"))
        .and_then(|i| i.as_str())
        .map(String::from);

    Ok(Some((resolved_version, tarball, integrity)))
}

const DEFAULT_PACKUMENT_CONCURRENCY: usize = 32;  // Increased from 8 to 32 for better parallelism
const MAX_PACKUMENT_CONCURRENCY: usize = 64;
const MIN_PACKUMENT_CONCURRENCY: usize = 1;
const DEFAULT_PARALLELISM_MULTIPLIER: usize = 4;  // Increased from 2 to 4 for more aggressive parallelism
const MIN_PARALLEL_CONCURRENCY: usize = 16;       // Increased from 8 to 16
const MAX_PARALLEL_CONCURRENCY: usize = 64;       // Increased from 32 to 64

fn packument_concurrency() -> usize {
    std::env::var("JHOL_PACKUMENT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(MIN_PACKUMENT_CONCURRENCY, MAX_PACKUMENT_CONCURRENCY))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * DEFAULT_PARALLELISM_MULTIPLIER).clamp(MIN_PARALLEL_CONCURRENCY, MAX_PARALLEL_CONCURRENCY))
                .unwrap_or(DEFAULT_PACKUMENT_CONCURRENCY)
        })
}

/// Parallel manifest fast-path resolve for multiple package requests.
/// Input tuple: (request_id, package_name, version_req).
/// Output tuple: (request_id, resolve_result).
pub fn parallel_resolve_tarballs_via_manifest(
    requests: &[(String, String, String)],
) -> Vec<(String, Result<Option<(String, String, Option<String>)>, String>)> {
    use std::sync::mpsc;
    use std::thread;

    let mut results = Vec::with_capacity(requests.len());
    let concurrency = packument_concurrency();

    for chunk in requests.chunks(concurrency) {
        let (tx, rx) = mpsc::channel();
        for (request_id, package, version_req) in chunk {
            let request_id = request_id.clone();
            let package = package.clone();
            let version_req = version_req.clone();
            let tx = tx.clone();
            thread::spawn(move || {
                let res = resolve_tarball_via_manifest(&package, &version_req);
                let _ = tx.send((request_id, res));
            });
        }
        drop(tx);
        for item in rx {
            results.push(item);
        }
    }

    results
}

/// Lazy JSON parser for efficient metadata access
#[derive(Debug)]
pub struct LazyMetadata {
    raw_bytes: Vec<u8>,
    parsed: Option<Value>,
}

impl LazyMetadata {
    pub fn new(raw_bytes: Vec<u8>) -> Self {
        Self {
            raw_bytes,
            parsed: None,
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.raw_bytes
    }

    pub fn parse(&mut self) -> Result<&Value, String> {
        if self.parsed.is_none() {
            self.parsed = Some(serde_json::from_slice(&self.raw_bytes).map_err(|e| e.to_string())?);
        }
        Ok(self.parsed.as_ref().unwrap())
    }

    pub fn get_versions(&mut self) -> Result<Option<&Value>, String> {
        Ok(self.parse()?.get("versions"))
    }

    pub fn get_dist_tags(&mut self) -> Result<Option<&Value>, String> {
        Ok(self.parse()?.get("dist-tags"))
    }

    pub fn get_name(&mut self) -> Result<Option<&Value>, String> {
        Ok(self.parse()?.get("name"))
    }
}

/// Cached metadata with lazy evaluation
#[derive(Debug)]
pub struct CachedMetadata {
    lazy: LazyMetadata,
    last_access: std::time::Instant,
}

impl CachedMetadata {
    pub fn new(raw_bytes: Vec<u8>) -> Self {
        Self {
            lazy: LazyMetadata::new(raw_bytes),
            last_access: std::time::Instant::now(),
        }
    }

    pub fn access(&mut self) -> Result<&LazyMetadata, String> {
        self.last_access = std::time::Instant::now();
        Ok(&self.lazy)
    }
}

/// LRU cache for parsed metadata
#[derive(Debug)]
struct MetadataCache {
    cache: HashMap<String, CachedMetadata>,
    capacity: usize,
}

impl MetadataCache {
    fn new(capacity: usize) -> Self {
        Self {
            cache: HashMap::with_capacity(capacity),
            capacity,
        }
    }

    fn get(&mut self, key: &str) -> Option<&mut CachedMetadata> {
        self.cache.get_mut(key)
    }

    fn insert(&mut self, key: String, metadata: CachedMetadata) {
        if self.cache.len() >= self.capacity {
            // LRU eviction: remove least recently accessed
            let mut oldest_key = None;
            let mut oldest_time = std::time::Instant::now();
            
            for (k, cached) in &self.cache {
                if cached.last_access < oldest_time {
                    oldest_time = cached.last_access;
                    oldest_key = Some(k.clone());
                }
            }
            
            if let Some(key) = oldest_key {
                self.cache.remove(&key);
            }
        }
        
        self.cache.insert(key, metadata);
    }
}

/// Global metadata cache
static METADATA_CACHE: std::sync::OnceLock<std::sync::Mutex<MetadataCache>> = std::sync::OnceLock::new();

fn get_metadata_cache() -> &'static std::sync::Mutex<MetadataCache> {
    METADATA_CACHE.get_or_init(|| {
        std::sync::Mutex::new(MetadataCache::new(
            std::env::var("JHOL_METADATA_CACHE_SIZE")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1000)
        ))
    })
}

/// Fetch package metadata from registry. Scoped: @scope/pkg -> @scope%2Fpkg.
/// Tries abbreviated packument (Accept: application/vnd.npm.install-v1+json) first;
/// falls back to full if unsupported or incomplete.
pub fn fetch_metadata(package: &str) -> Result<serde_json::Value, String> {
    let cache_key = format!("metadata:{}", package);

    // Try cache first
    {
        let mut cache = get_metadata_cache().lock().unwrap();
        if let Some(cached) = cache.get(&cache_key) {
            let bytes = &cached.access().unwrap().raw_bytes;
            if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(bytes) {
                // Check if abbreviated packument is sufficient
                if parsed.get("versions").and_then(|v| v.as_object()).map(|o| !o.is_empty()).unwrap_or(false) {
                    return Ok(parsed.clone());
                }
            }
        }
    }

    // Fetch from network
    let body = match fetch_packument_with_etag(package, true) {
        Ok(b) => b,
        Err(_) => fetch_packument_with_etag(package, false)?,
    };

    // Parse metadata
    let parsed: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    // Cache in pre-resolved index for future O(1) lookups
    cache_in_index(package, &parsed);

    // Check if abbreviated packument is sufficient
    let has_versions = parsed.get("versions").and_then(|v| v.as_object()).map(|o| !o.is_empty()).unwrap_or(true);

    if has_versions {
        // Cache the metadata
        let mut cache = get_metadata_cache().lock().unwrap();
        cache.insert(cache_key, CachedMetadata::new(body));

        // Return parsed value
        return Ok(parsed);
    }

    // Fall back to full packument
    let body = fetch_packument_with_etag(package, false)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    // Cache the full metadata
    let mut cache = get_metadata_cache().lock().unwrap();
    cache.insert(cache_key, CachedMetadata::new(body));

    Ok(v)
}

/// Fetch metadata for multiple packages in parallel using rayon
pub fn fetch_metadata_parallel(packages: &[String]) -> Vec<(String, Result<serde_json::Value, String>)> {
    use rayon::prelude::*;
    
    packages
        .par_iter()
        .map(|package| {
            let result = fetch_metadata(package);
            (package.clone(), result)
        })
        .collect()
}

/// Fetch package metadata, using an in-memory cache to avoid duplicate requests during a resolve.
pub fn fetch_metadata_cached(
    package: &str,
    cache: &mut HashMap<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    if let Some(cached) = cache.get(package) {
        return Ok(cached.clone());
    }
    let meta = fetch_metadata(package)?;
    cache.insert(package.to_string(), meta.clone());
    Ok(meta)
}

/// Fetch packuments for multiple packages in parallel. Uses shared cache to avoid duplicate fetches.
pub fn parallel_fetch_metadata(
    names: &[String],
    cache: &std::sync::Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
) -> Vec<(String, Result<serde_json::Value, String>)> {
    use std::sync::mpsc;
    use std::thread;
    
    let mut results = Vec::with_capacity(names.len());
    let concurrency = packument_concurrency();
    
    // Pre-filter already cached packages to reduce thread overhead
    let mut uncached_names = Vec::new();
    {
        let guard = cache.lock().unwrap();
        for name in names {
            if !guard.contains_key(name) {
                uncached_names.push(name.clone());
            } else {
                let cached = guard.get(name).unwrap().clone();
                results.push((name.clone(), Ok(cached)));
            }
        }
    }
    
    // Process remaining uncached packages in parallel
    for chunk in uncached_names.chunks(concurrency) {
        let (tx, rx) = mpsc::channel();
        for name in chunk {
            let name = name.clone();
            let tx = tx.clone();
            let cache = std::sync::Arc::clone(cache);
            thread::spawn(move || {
                // Double-check cache after thread spawn
                {
                    let guard = cache.lock().unwrap();
                    if let Some(cached) = guard.get(&name) {
                        let _ = tx.send((name, Ok(cached.clone())));
                        return;
                    }
                }
                
                let res = fetch_metadata(&name);
                if let Ok(ref meta) = res {
                    let mut guard = cache.lock().unwrap();
                    guard.insert(name.clone(), meta.clone());
                }
                let _ = tx.send((name, res));
            });
        }
        drop(tx);
        for (name, res) in rx {
            results.push((name, res));
        }
    }
    results
}

/// Validate that a package exists on the registry. Uses packument GET.
pub fn validate_package_exists(package: &str) -> Result<bool, String> {
    match fetch_metadata(package) {
        Ok(meta) => {
            let has_versions = meta
                .get("versions")
                .and_then(|v| v.as_object())
                .map(|o| !o.is_empty())
                .unwrap_or(false);
            let has_name = meta.get("name").is_some();
            Ok(has_versions || has_name)
        }
        Err(e) => {
            if e.contains("404") || e.to_lowercase().contains("not found") {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// Run validate_package_exists for multiple packages in parallel.
pub fn parallel_validate_packages(packages: &[String], _timeout_secs: u64) -> Vec<(String, bool)> {
    use std::sync::mpsc;
    use std::thread;
    const CONCURRENCY: usize = 8;
    let (tx, rx) = mpsc::channel();
    for chunk in packages.chunks(CONCURRENCY) {
        let chunk: Vec<String> = chunk.to_vec();
        let tx = tx.clone();
        thread::spawn(move || {
            for pkg in chunk {
                let ok = validate_package_exists(&pkg).unwrap_or(false);
                let _ = tx.send((pkg, ok));
            }
        });
    }
    drop(tx);
    rx.into_iter().collect()
}

/// Check if a concrete version satisfies a semver range/spec.
pub fn version_satisfies(spec: &str, version: &str) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return Version::parse(version).is_ok();
    }
    let req = match VersionReq::parse(spec) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let v = match Version::parse(version) {
        Ok(v) => v,
        Err(_) => return false,
    };
    req.matches(&v)
}

/// Resolve a semver range to the maximum satisfying version from a list.
pub fn resolve_range(version_strings: &[String], range: &str) -> Option<String> {
    let range = range.trim();
    if range.is_empty() || range == "*" {
        let mut parsed: Vec<Version> = version_strings
            .iter()
            .filter_map(|s| Version::parse(s).ok())
            .collect();
        parsed.sort();
        return parsed.last().map(|v| v.to_string());
    }
    let req = VersionReq::parse(range).ok()?;
    let mut satisfying: Vec<Version> = version_strings
        .iter()
        .filter_map(|s| Version::parse(s).ok())
        .filter(|v| req.matches(v))
        .collect();
    satisfying.sort();
    satisfying.last().map(|v| v.to_string())
}

/// Resolve version to a concrete semver (e.g. "latest" -> "1.2.3", "^1.0" -> "1.2.3")
pub fn resolve_version(meta: &serde_json::Value, version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() || version == "latest" {
        let dist_tags = meta.get("dist-tags")?.as_object()?;
        return dist_tags.get("latest").and_then(|v| v.as_str()).map(String::from);
    }
    let versions = meta.get("versions")?.as_object()?;
    if versions.contains_key(version) {
        return Some(version.to_string());
    }
    let dist_tags = meta.get("dist-tags").and_then(|t| t.as_object());
    if let Some(tags) = dist_tags {
        if let Some(tag) = tags.get(version) {
            if let Some(s) = tag.as_str() {
                return Some(s.to_string());
            }
        }
    }
    let looks_like_range = version.starts_with('^')
        || version.starts_with('~')
        || version.starts_with('>')
        || version.starts_with('<')
        || version.starts_with('=')
        || version.contains(' ')
        || version == "*";
    if looks_like_range {
        let version_list: Vec<String> = versions.keys().map(String::clone).collect();
        return resolve_range(&version_list, version);
    }
    None
}

/// Get tarball URL for a specific version from metadata.
pub fn get_tarball_url(meta: &serde_json::Value, version: &str) -> Option<String> {
    let versions = meta.get("versions")?.as_object()?;
    let ver_obj = versions.get(version)?.as_object()?;
    let dist = ver_obj.get("dist")?.as_object()?;
    dist.get("tarball")?.as_str().map(String::from)
}

/// Get integrity (SRI) for a specific version from packument, if present.
pub fn get_integrity_for_version(meta: &serde_json::Value, version: &str) -> Option<String> {
    meta.get("versions")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(version))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("dist"))
        .and_then(|d| d.as_object())
        .and_then(|d| d.get("integrity"))
        .and_then(|i| i.as_str())
        .map(String::from)
}

/// Fill the content-addressable store from the registry for a package@version.
pub fn fill_store_from_registry(
    package: &str,
    version: &str,
    cache_dir: &Path,
) -> Result<PathBuf, String> {
    let meta = fetch_metadata(package)?;
    let url = get_tarball_url(&meta, version)
        .ok_or_else(|| format!("No tarball URL for {}@{}", package, version))?;
    let pkg_key = format!("{}@{}", package, version);
    let integrity = get_integrity_for_version(&meta, version);
    download_tarball_to_store(&url, cache_dir, &pkg_key, None, integrity.as_deref())
}

/// Get required dependencies of a specific version from packument.
pub fn get_version_required_dependencies(
    meta: &serde_json::Value,
    version: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let versions = match meta.get("versions").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    let ver_obj = match versions.get(version).and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    if let Some(deps) = ver_obj.get("dependencies").and_then(|d| d.as_object()) {
        for (k, v) in deps {
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), s.to_string());
            }
        }
    }
    out
}

/// Get optional dependencies of a specific version from packument.
pub fn get_version_optional_dependencies(
    meta: &serde_json::Value,
    version: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let versions = match meta.get("versions").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    let ver_obj = match versions.get(version).and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    if let Some(deps) = ver_obj.get("optionalDependencies").and_then(|d| d.as_object()) {
        for (k, v) in deps {
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), s.to_string());
            }
        }
    }
    out
}

pub fn get_version_dependencies(
    meta: &serde_json::Value,
    version: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = get_version_required_dependencies(meta, version);
    out.extend(get_version_optional_dependencies(meta, version));
    out
}

pub fn get_version_peer_dependencies(
    meta: &serde_json::Value,
    version: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let versions = match meta.get("versions").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    let ver_obj = match versions.get(version).and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    if let Some(deps) = ver_obj.get("peerDependencies").and_then(|d| d.as_object()) {
        for (k, v) in deps {
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), s.to_string());
            }
        }
    }
    out
}

/// Enhanced peer dependency resolution that considers peerDependenciesMeta for optional dependencies.
pub fn resolve_peer_dependencies_with_meta(
    meta: &serde_json::Value,
    version: &str,
) -> std::collections::HashMap<String, (String, bool)> {
    let mut out = std::collections::HashMap::new();
    let peer_deps = get_version_peer_dependencies(meta, version);
    let peer_deps_meta = get_version_peer_dependencies_meta(meta, version);
    
    for (name, range) in peer_deps {
        // Check if this peer dependency is optional according to peerDependenciesMeta
        let optional = peer_deps_meta
            .get(&name)
            .and_then(|meta| meta.get("optional"))
            .and_then(|opt| opt.as_bool())
            .unwrap_or(false);
        
        out.insert(name, (range, optional));
    }
    
    out
}

pub fn get_version_peer_dependencies_meta(
    meta: &serde_json::Value,
    version: &str,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut out = std::collections::HashMap::new();
    let versions = match meta.get("versions").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    let ver_obj = match versions.get(version).and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return out,
    };
    if let Some(meta_obj) = ver_obj.get("peerDependenciesMeta").and_then(|d| d.as_object()) {
        for (k, v) in meta_obj {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Download tarball from URL to a file (uses shared HTTP client).
pub fn download_tarball(url: &str, dest: &Path) -> Result<PathBuf, String> {
    let token = registry_auth_token();
    crate::http_client::get_to_file_with_bearer(url, dest, token.as_deref())?;
    Ok(dest.to_path_buf())
}

/// Streaming download with concurrent extraction
/// Downloads tarball while simultaneously extracting to reduce total time
pub fn download_and_extract_streaming(
    url: &str,
    dest_dir: &Path,
    expected_integrity: Option<&str>,
) -> Result<PathBuf, String> {
    use std::io::{Read, Write};
    use flate2::read::GzDecoder;
    use tar::Archive;

    // Create temp file for download
    let tmp = std::env::temp_dir().join(format!("jhol-stream-{}-{}.tgz", 
        std::process::id(), 
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    
    // Download with streaming support
    let token = registry_auth_token();
    crate::http_client::get_to_file_with_bearer(url, &tmp, token.as_deref())?;
    
    // Verify integrity if provided
    if let Some(sri) = expected_integrity {
        if !crate::utils::verify_sri(&tmp, sri) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Integrity check failed during streaming download"));
        }
    }
    
    // Extract while potentially still downloading (in full implementation would use channels)
    // For now, extract after download completes
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    
    let f = File::open(&tmp).map_err(|e| e.to_string())?;
    let dec = GzDecoder::new(std::io::BufReader::new(f));
    let mut archive = Archive::new(dec);
    
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path_str = path.to_string_lossy();
        let parts: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let rel: String = parts[1..].join(std::path::MAIN_SEPARATOR_STR);
        if rel.is_empty() {
            continue;
        }
        let out_path = dest_dir.join(&rel);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            entry.unpack(&out_path).map_err(|e| e.to_string())?;
        }
    }
    
    // Cleanup temp file
    let _ = std::fs::remove_file(&tmp);
    
    Ok(dest_dir.to_path_buf())
}

/// Download tarball to store and return hash. Does not update the store index (for parallel use).
pub fn download_tarball_to_store(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    index_batch: Option<&mut std::collections::HashMap<String, String>>,
    expected_integrity: Option<&str>,
) -> Result<PathBuf, String> {
    let hash = download_tarball_to_store_hash_only(url, cache_dir, pkg_key, expected_integrity)?;
    let store_file = cache_dir.join("store").join(format!("{}.tgz", hash));
    if let Some(batch) = index_batch {
        batch.insert(pkg_key.to_string(), hash);
    } else {
        let mut index = crate::utils::read_store_index();
        index.insert(pkg_key.to_string(), hash);
        crate::utils::write_store_index(&index).map_err(|e| e.to_string())?;
    }
    Ok(store_file)
}

static TMP_DOWNLOAD_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

static BINARY_PACKAGES_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
static BINARY_PACKAGE_INDEX: OnceLock<Option<HashMap<String, String>>> = OnceLock::new();
static BINARY_PACKAGE_DEPS_INDEX: OnceLock<Option<HashMap<String, HashMap<String, String>>>> = OnceLock::new();

fn discover_binary_packages_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("JHOL_BINARY_PACKAGES_DIR") {
        let path = PathBuf::from(dir);
        if path.join("index.json").exists() {
            return Some(path);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("binary-packages");
        if path.join("index.json").exists() {
            return Some(path);
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        let mut cur = exe_path.parent();
        while let Some(dir) = cur {
            let candidate = dir.join("binary-packages");
            if candidate.join("index.json").exists() {
                return Some(candidate);
            }
            cur = dir.parent();
        }
    }

    let repo_candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../binary-packages");
    if repo_candidate.join("index.json").exists() {
        return Some(repo_candidate);
    }

    None
}

fn binary_packages_dir() -> Option<&'static PathBuf> {
    BINARY_PACKAGES_DIR
        .get_or_init(discover_binary_packages_dir)
        .as_ref()
}

fn binary_package_index() -> Option<&'static HashMap<String, String>> {
    BINARY_PACKAGE_INDEX
        .get_or_init(|| {
            let dir = binary_packages_dir()?;
            let index_path = dir.join("index.json");
            let raw = std::fs::read_to_string(index_path).ok()?;
            serde_json::from_str::<HashMap<String, String>>(&raw).ok()
        })
        .as_ref()
}

fn binary_package_deps_index() -> Option<&'static HashMap<String, HashMap<String, String>>> {
    BINARY_PACKAGE_DEPS_INDEX
        .get_or_init(|| {
            let dir = binary_packages_dir()?;
            let path = dir.join("deps.json");
            let raw = std::fs::read_to_string(path).ok()?;
            serde_json::from_str::<HashMap<String, HashMap<String, String>>>(&raw).ok()
        })
        .as_ref()
}

fn parse_exact_package_spec(pkg_key: &str) -> Option<(String, String)> {
    let at = pkg_key.rfind('@')?;
    if at == 0 {
        return None;
    }
    let package = &pkg_key[..at];
    let version = pkg_key[at + 1..].trim();
    if version.is_empty()
        || version.contains('/')
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
    Some((package.to_string(), version.to_string()))
}

fn decode_binary_package_archive(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 52 {
        return Err("binary package too small".to_string());
    }
    if &data[0..4] != b"JHOL" {
        return Err("invalid binary package header".to_string());
    }

    let package_name_len = u16::from_le_bytes([data[8], data[9]]) as usize;
    let version_len = u16::from_le_bytes([data[10], data[11]]) as usize;
    let header_hash = &data[12..44];
    let original_size = u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as usize;
    let compressed_size = u32::from_le_bytes([data[48], data[49], data[50], data[51]]) as usize;

    let compressed_start = 52usize
        .saturating_add(package_name_len)
        .saturating_add(version_len);
    let compressed_end = compressed_start.saturating_add(compressed_size);
    if compressed_end > data.len() {
        return Err("binary package payload truncated".to_string());
    }

    let mut decoder = flate2::read::GzDecoder::new(&data[compressed_start..compressed_end]);
    let mut tarball = Vec::with_capacity(original_size);
    decoder
        .read_to_end(&mut tarball)
        .map_err(|e| format!("binary package decompression failed: {}", e))?;

    let digest = Sha256::digest(&tarball);
    if digest.as_slice() != header_hash {
        return Err("binary package content hash mismatch".to_string());
    }

    Ok(tarball)
}

fn load_binary_tarball(package: &str, version: &str) -> Result<Option<Vec<u8>>, String> {
    let dir = match binary_packages_dir() {
        Some(dir) => dir,
        None => return Ok(None),
    };
    let index = match binary_package_index() {
        Some(index) => index,
        None => return Ok(None),
    };

    let key = format!("{}@{}", package, version);
    let Some(hash) = index.get(&key) else {
        return Ok(None);
    };

    let archive_path = dir.join(format!("{}.jhol", hash));
    if !archive_path.exists() {
        return Ok(None);
    }

    let data = std::fs::read(&archive_path)
        .map_err(|e| format!("failed to read binary package {}: {}", archive_path.display(), e))?;
    let tarball = decode_binary_package_archive(&data)?;
    Ok(Some(tarball))
}


pub fn binary_package_dependencies(package: &str, version: &str) -> Option<HashMap<String, String>> {
    let spec_key = format!("{}@{}", package, version);
    if let Some(index) = binary_package_deps_index() {
        if let Some(deps) = index.get(&spec_key) {
            return Some(deps.clone());
        }
    }

    let tarball = load_binary_tarball(package, version).ok().flatten()?;
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(tarball));
    let mut archive = tar::Archive::new(dec);
    for entry in archive.entries().ok()? {
        let mut entry = entry.ok()?;
        let path = entry.path().ok()?;
        let path_str = path.to_string_lossy();
        if path_str == "package/package.json" || path_str.ends_with("/package.json") {
            let mut buf = Vec::new();
            if entry.read_to_end(&mut buf).is_err() {
                return None;
            }
            let v: serde_json::Value = serde_json::from_slice(&buf).ok()?;
            let mut deps = HashMap::new();
            if let Some(obj) = v.get("dependencies").and_then(|d| d.as_object()) {
                for (k, vv) in obj {
                    if let Some(spec) = vv.as_str() {
                        deps.insert(k.clone(), spec.to_string());
                    }
                }
            }
            return Some(deps);
        }
    }
    None
}

fn shared_binary_unpacked_path(hash: &str) -> Option<PathBuf> {
    binary_packages_dir().map(|dir| dir.join(".unpacked").join(hash))
}

fn ensure_shared_binary_unpacked(hash: &str, tarball: &[u8]) -> Result<Option<PathBuf>, String> {
    let Some(shared_unpacked) = shared_binary_unpacked_path(hash) else {
        return Ok(None);
    };

    if shared_unpacked.exists() {
        return Ok(Some(shared_unpacked));
    }

    if let Some(parent) = shared_unpacked.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let tmp = shared_unpacked.with_extension(format!("tmp-{}", std::process::id()));
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    if let Err(err) = extract_tarball_bytes_to_dir(tarball, &tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(err);
    }

    if std::fs::rename(&tmp, &shared_unpacked).is_err() {
        if !shared_unpacked.exists() {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("failed to materialize shared binary unpacked cache".to_string());
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    Ok(Some(shared_unpacked))
}

fn link_or_copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        std::fs::remove_dir_all(dst).map_err(|e| e.to_string())?;
    }

    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(src, dst).is_ok() {
            return Ok(());
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(src, dst).is_ok() {
            return Ok(());
        }
    }

    copy_dir_recursive(src, dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}


fn version_matches_all_constraints(version: &semver::Version, constraints: &[String]) -> bool {
    let vs = version.to_string();
    constraints.iter().all(|spec| {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed == "latest" || trimmed == "*" {
            return true;
        }
        if let Ok(req) = semver::VersionReq::parse(trimmed) {
            return req.matches(version);
        }
        version_satisfies(trimmed, &vs)
    })
}

/// Best (highest semver) version available in local binary package index for a package.
pub fn best_binary_version(package: &str) -> Option<String> {
    best_binary_version_matching(package, &[])
}

/// Best binary version satisfying all constraints (semver ranges / exacts).
pub fn best_binary_version_matching(package: &str, constraints: &[String]) -> Option<String> {
    let index = binary_package_index()?;
    let mut best: Option<semver::Version> = None;
    for key in index.keys() {
        let at = key.rfind('@')?;
        if at == 0 {
            continue;
        }
        let name = &key[..at];
        if name != package {
            continue;
        }
        let ver = &key[at + 1..];
        let Ok(parsed) = semver::Version::parse(ver) else {
            continue;
        };
        if !version_matches_all_constraints(&parsed, constraints) {
            continue;
        }
        if best.as_ref().map(|b| &parsed > b).unwrap_or(true) {
            best = Some(parsed);
        }
    }
    best.map(|v| v.to_string())
}

/// Download tarball to store and return hash. Does not update the store index.
pub fn download_tarball_to_store_hash_only(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
) -> Result<String, String> {
    let store_dir = cache_dir.join("store");
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;

    if let Some((package, version)) = parse_exact_package_spec(pkg_key) {
        if let Ok(Some(tarball)) = load_binary_tarball(&package, &version) {
            let hash = format!("{:x}", Sha256::digest(&tarball));

            if let Some(sri) = expected_integrity {
                if !crate::utils::verify_sri_bytes(&tarball, sri) {
                    return Err(format!("Integrity check failed for {}", pkg_key));
                }
            }

            // Pre-populate unpacked store to avoid a second decompression pass in cold installs.
            let unpacked = cache_dir.join("store_unpacked").join(&hash);
            if !unpacked.exists() {
                let mut unpacked_ready = false;
                if let Some(shared_unpacked) = ensure_shared_binary_unpacked(&hash, &tarball)? {
                    if shared_unpacked.exists() {
                        unpacked_ready = link_or_copy_dir(&shared_unpacked, &unpacked).is_ok();
                    }
                }
                if !unpacked_ready {
                    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
                    if let Err(e) = extract_tarball_bytes_to_dir(&tarball, &unpacked) {
                        let _ = std::fs::remove_dir_all(&unpacked);
                        return Err(e);
                    }
                }
            }

            let store_file = store_dir.join(format!("{}.tgz", hash));
            if !store_file.exists() {
                std::fs::write(&store_file, &tarball).map_err(|e| e.to_string())?;
            }
            return Ok(hash);
        }
    }

    let n = TMP_DOWNLOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = cache_dir.join(format!("tmp-{}-{}.tgz", std::process::id(), n));
    download_tarball(url, &tmp).map_err(|e| format!("download: {}", e))?;
    if let Some(sri) = expected_integrity {
        if !crate::utils::verify_sri(&tmp, sri) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Integrity check failed for {}", pkg_key));
        }
    }
    let hash = crate::utils::content_hash(&tmp).map_err(|e| e.to_string())?;
    let store_file = store_dir.join(format!("{}.tgz", hash));
    std::fs::rename(&tmp, &store_file)
        .or_else(|_| std::fs::copy(&tmp, &store_file).map(|_| ()))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    Ok(hash)
}

/// Ensure the tarball is unpacked in store_unpacked/<hash>; return that path.
pub fn ensure_unpacked_in_store(tarball_path: &Path, cache_dir: &Path) -> Result<PathBuf, String> {
    let hash = tarball_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid store path".to_string())?;
    let store_unpacked_base = cache_dir.join("store_unpacked");
    let unpacked = store_unpacked_base.join(hash);
    if !unpacked.exists() {
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        extract_tarball_to_dir(tarball_path, &unpacked)?;
    }
    Ok(unpacked)
}


fn extract_tarball_bytes_to_dir(tarball_data: &[u8], dest_dir: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let dec = GzDecoder::new(std::io::Cursor::new(tarball_data));
    let mut archive = Archive::new(dec);

    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path_str = path.to_string_lossy();
        let parts: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let rel: String = parts[1..].join(std::path::MAIN_SEPARATOR_STR);
        if rel.is_empty() {
            continue;
        }
        let out_path = dest_dir.join(&rel);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out_path.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            entry.unpack(&out_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Extract .tgz into dest_dir, stripping one top-level directory from tarball.
pub fn extract_tarball_to_dir(tarball_path: &Path, dest_dir: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let f = File::open(tarball_path).map_err(|e| e.to_string())?;
    let dec = GzDecoder::new(BufReader::new(f));
    let mut archive = Archive::new(dec);

    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path_str = path.to_string_lossy();
        let parts: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let rel: String = parts[1..].join(std::path::MAIN_SEPARATOR_STR);
        if rel.is_empty() {
            continue;
        }
        let out_path = dest_dir.join(&rel);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out_path.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            entry.unpack(&out_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Extract .tgz to node_modules/<package_name>. Strips one top-level directory.
pub fn extract_tarball(
    tarball_path: &Path,
    node_modules_dir: &Path,
    package_name: &str,
) -> Result<(), String> {
    let dest = node_modules_dir.join(package_name);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    extract_tarball_to_dir(tarball_path, &dest)
}

/// Install a single package via native registry (fetch metadata, download tarball, extract).
/// Returns Ok(()) on success, Err on failure (caller can fall back to npm/bun).
pub fn install_package_native(
    package: &str,
    node_modules: &Path,
    cache_dir: &Path,
    options: &crate::install::InstallOptions,
) -> Result<(), String> {
    let meta = fetch_metadata(package)?;
    let (base_name, version_req) = if package.contains('@') && !package.starts_with('@') {
        let mut parts = package.splitn(2, '@');
        let base = parts.next().unwrap_or(package);
        let ver = parts.next().unwrap_or("latest");
        (base, ver)
    } else if package.starts_with('@') {
        let idx = package.rfind('@').unwrap_or(0);
        if idx > 0 {
            (package[..idx].trim_end_matches('@'), package[idx + 1..].trim())
        } else {
            (package, "latest")
        }
    } else {
        (package, "latest")
    };
    let version =
        resolve_version(&meta, version_req).ok_or_else(|| format!("could not resolve version {}", version_req))?;
    let tarball_url = get_tarball_url(&meta, &version).ok_or("no tarball in metadata")?;

    let pkg_key = format!("{}@{}", base_name, version);
    let store_dir = cache_dir.join("store");
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;

    let tmp = cache_dir.join(format!("tmp-{}.tgz", std::process::id()));
    download_tarball(&tarball_url, &tmp).map_err(|e| format!("download: {}", e))?;
    let hash = crate::utils::content_hash(&tmp).map_err(|e| e.to_string())?;
    let store_file = store_dir.join(format!("{}.tgz", hash));
    std::fs::rename(&tmp, &store_file)
        .or_else(|_| std::fs::copy(&tmp, &store_file).map(|_| ()))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    let mut index = crate::utils::read_store_index();
    index.insert(pkg_key, hash.clone());
    crate::utils::write_store_index(&index).map_err(|e| e.to_string())?;

    let store_file = store_dir.join(format!("{}.tgz", hash));
    extract_tarball(&store_file, node_modules, base_name)?;
    if !options.quiet {
        crate::utils::log(&format!("Installed {}@{} (native)", base_name, version));
    }
    Ok(())
}
