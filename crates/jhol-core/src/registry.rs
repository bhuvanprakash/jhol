//! Native npm registry client: fetch metadata and tarballs via HTTP.
//! All HTTP calls go through `crate::http_client` (shared Agent = TCP connection pool).
//! Optimized with pre-resolved package index for instant resolution of common packages.

use semver::{Version, VersionReq};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, RwLock};

/// Cache for parsed semver requirements — avoids re-parsing common specs
/// like "^1.0.0", ">=2.0.0", "*", etc. during resolution.
pub(crate) fn cached_version_req(spec: &str) -> Option<VersionReq> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<VersionReq>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().ok()?;
    if let Some(cached) = guard.get(spec) {
        return cached.clone();
    }
    let parsed = VersionReq::parse(spec).ok();
    guard.insert(spec.to_string(), parsed.clone());
    parsed
}

/// True when `spec` is a bare, exact version pin (e.g. `4.1.6`, `v4.1.6`,
/// `=4.1.6`). npm treats these as EXACT matches; `semver::VersionReq` parses a
/// bare `4.1.6` as `^4.1.6` (caret), which wrongly matches 4.2.11. Any helper
/// that must honour npm semantics for legacy pins must check this first.
pub fn is_exact_version_pin(spec: &str) -> bool {
    let s = spec.trim();
    if s.is_empty() {
        return false;
    }
    let bare = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('=').map(|x| x.trim()))
        .unwrap_or(s);
    // A pin is exact iff it is a full three-part version (or has a prerelease/
    // build) with no operator, wildcard, hyphen range, or partial component.
    if bare.contains('x') || bare.contains('*') || bare.contains('-') {
        return false;
    }
    if let Ok(v) = semver::Version::parse(bare) {
        let _ = v;
        // Reject partial pins like `1.2` or `1` (npm treats those as ranges,
        // not exact).
        let parts: Vec<&str> = bare.split('.').collect();
        return parts.len() >= 3;
    }
    false
}

/// Global packument read-cache for O(1) lookup without holding a lock.
/// Updated by fetch_metadata and fetch_metadata_parallel after each network fetch.
/// This is the fast path: predictor prefetches -> cache populated -> warm installs hit this.
fn packument_read_cache() -> &'static RwLock<HashMap<String, serde_json::Value>> {
    static CACHE: OnceLock<RwLock<HashMap<String, serde_json::Value>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::with_capacity(500)))
}

/// Check if a packument is already in the read cache (O(1), non-blocking read).
pub fn in_packument_cache(package: &str) -> bool {
    packument_read_cache()
        .read()
        .ok()
        .map_or(false, |g| g.contains_key(package))
}

/// Look up a packument from the read cache without blocking.
pub fn get_cached_packument(package: &str) -> Option<serde_json::Value> {
    packument_read_cache().read().ok()?.get(package).cloned()
}

/// Insert a packument into the read cache (called after fetch_metadata returns).
pub fn cache_packument(package: &str, meta: &serde_json::Value) {
    if let Ok(mut guard) = packument_read_cache().write() {
        guard.insert(package.to_string(), meta.clone());
    }
}

/// Extract package name without version specifier.
/// Scoped `@scope/pkg@1.0.0` remains `@scope/pkg`.
pub fn base_name(package_spec: &str) -> String {
    if package_spec.starts_with('@') {
        package_spec[1..]
            .find('@')
            .map(|index| package_spec[..index + 1].to_owned())
            .unwrap_or_else(|| package_spec.to_owned())
    } else {
        package_spec
            .find('@')
            .map(|index| package_spec[..index].to_owned())
            .unwrap_or_else(|| package_spec.to_owned())
    }
}

lazy_static::lazy_static! {
    /// Global package index for O(1) resolution
    pub static ref PACKAGE_INDEX: Arc<Mutex<crate::package_index::PackageIndex>> = {
        let index_path = PathBuf::from(crate::utils::get_cache_dir()).join("package-index");
        Arc::new(Mutex::new(crate::package_index::PackageIndex::new(index_path)))
    };
}

fn registry_url() -> String {
    crate::config::effective_registry_url(Path::new("."))
}

pub fn registry_auth_token() -> Option<String> {
    crate::config::registry_auth_token(Path::new("."))
}

fn packument_cache_dir() -> PathBuf {
    PathBuf::from(crate::utils::get_cache_dir()).join("packuments")
}

fn packument_cache_key(package: &str, abbreviated: bool) -> String {
    let h = xxhash_rust::xxh3::xxh3_64(package.as_bytes()).wrapping_add(if abbreviated {
        1u64
    } else {
        0u64
    });
    format!("{:016x}", h)
}

fn packument_cache_path(package: &str, abbreviated: bool) -> PathBuf {
    let key = packument_cache_key(package, abbreviated);
    packument_cache_dir().join(format!("{}.json", key))
}

fn read_packument_cache(package: &str, abbreviated: bool) -> (Option<Vec<u8>>, Option<String>) {
    let body_path = packument_cache_path(package, abbreviated);
    let raw = match std::fs::read(&body_path) {
        Ok(d) => d,
        Err(_) => return (None, None),
    };
    // Single-file format: optional etag on first line, rest is body.
    // If no newline, entire file is the body with no etag.
    if let Some(pos) = raw.iter().position(|&b| b == b'\n') {
        let etag = String::from_utf8_lossy(&raw[..pos]).trim().to_string();
        let body = raw[pos + 1..].to_vec();
        (Some(body), Some(etag))
    } else {
        (Some(raw), None)
    }
}

fn write_packument_cache(
    package: &str,
    abbreviated: bool,
    body: &[u8],
    etag: Option<&str>,
) -> Result<(), String> {
    let body_path = packument_cache_path(package, abbreviated);
    if let Some(parent) = body_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = Vec::with_capacity(body.len() + 128);
    if let Some(etag) = etag {
        out.extend_from_slice(etag.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(body);
    std::fs::write(&body_path, out).map_err(|e| e.to_string())?;
    Ok(())
}

/// Returns true when `JHOL_OFFLINE=1`/`true` (or config offline) is set. In
/// offline mode the registry layer must NEVER touch the network — missing
/// entries fail fast with a clear error instead of hanging on a socket.
pub fn offline_mode() -> bool {
    let env = std::env::var("JHOL_OFFLINE").unwrap_or_default();
    if env == "1" || env.eq_ignore_ascii_case("true") {
        return true;
    }
    crate::config::global_config()
        .map(|c| c.offline.unwrap_or(false))
        .unwrap_or(false)
}

/// Fetch packument bytes via the shared HTTP client (connection pooling).
/// Supports ETags for conditional requests and abbreviated format.
fn fetch_packument_with_etag(package: &str, abbreviated: bool) -> Result<Vec<u8>, String> {
    if offline_mode() {
        return Err(format!(
            "offline mode: packument for {} not in cache (no network allowed)",
            package
        ));
    }
    let path = if package.starts_with('@') {
        package.replace('/', "%2F")
    } else {
        package.to_string()
    };
    let base = registry_url();
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
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
            h.push((
                "Accept".into(),
                "application/vnd.npm.install-v1+json".into(),
            ));
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
        let refs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
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
                _ => Err(format!(
                    "Failed to fetch packument for {} after 304 retry",
                    package
                )),
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
fn resolve_from_index(
    package: &str,
    _version_req: &str,
) -> Option<crate::package_index::PreResolvedPackage> {
    let index = PACKAGE_INDEX.lock().ok()?;
    index.lookup(package, _version_req).cloned()
}

/// Synthesize a full packument-shape JSON object from a bundled-index entry.
///
/// The bundled-index.json (37 KB binary-embedded) carries 50 top packages with their
/// tarball URL, integrity hash, dependencies, and version list. For constraints that
/// match the bundled "latest" (the 90%+ case for `^X.Y.Z` and `latest`), this returns
/// a synthesized packument that the resolver consumes as if it came from npm — zero
/// network. For constraints that don't match, returns None and the resolver falls
/// through to the existing network path.
///
/// All fields consumed by `expand_with_transitive_dependencies`,
/// `resolve_version_for_constraints`, `version_supported_on_current_platform`,
/// `get_version_required_dependencies`, and `get_version_optional_dependencies` are
/// included, so the synthesized packument is a drop-in replacement at the
/// `fetch_metadata` boundary.
pub(crate) fn synthesize_packument_from_bundled_index(
    package: &str,
    constraint: &str,
) -> Option<serde_json::Value> {
    let entry = resolve_from_index(package, constraint)?;

    let mut versions = serde_json::Map::new();
    let mut saw_satisfying = false;
    for ver in &entry.versions {
        let mut version_obj = serde_json::Map::new();
        version_obj.insert(
            "name".to_string(),
            serde_json::Value::String(entry.name.clone()),
        );
        version_obj.insert(
            "version".to_string(),
            serde_json::Value::String(ver.clone()),
        );
        // Dependencies are only known for the latest bundled version. For other
        // bundled versions we cannot know their real deps from the index, so we
        // leave them empty — those versions are only selectable when explicitly
        // requested and their transitive deps come from a full network packument.
        if *ver == entry.version {
            version_obj.insert(
                "dependencies".to_string(),
                serde_json::to_value(&entry.dependencies).unwrap_or(serde_json::json!({})),
            );
        }
        version_obj.insert("engines".to_string(), serde_json::json!({"node": ">=10"}));
        let mut dist = serde_json::Map::new();
        dist.insert(
            "tarball".to_string(),
            serde_json::Value::String(entry.tarball_url.clone()),
        );
        dist.insert(
            "integrity".to_string(),
            serde_json::Value::String(entry.integrity.clone()),
        );
        version_obj.insert("dist".to_string(), serde_json::Value::Object(dist));

        if crate::registry::version_satisfies(constraint, ver) {
            saw_satisfying = true;
        }

        versions.insert(ver.clone(), serde_json::Value::Object(version_obj));
    }

    // The synthesized packument is only a drop-in replacement when at least one
    // bundled version actually satisfies the requested constraint. If the bundle
    // cannot serve the request (e.g. the constraint requires a version that is
    // not in the bundled list), return None so the caller falls through to a
    // full network packument — a partial synth would cause a false "unresolved".
    if !saw_satisfying {
        return None;
    }

    let mut dist_tags = serde_json::Map::new();
    dist_tags.insert(
        "latest".to_string(),
        serde_json::Value::String(entry.version.clone()),
    );

    Some(serde_json::json!({
        "name": entry.name,
        "versions": versions,
        "dist-tags": dist_tags,
        "_bundled_index_synth": true,
    }))
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
        return Ok(Some((
            resolved.version,
            resolved.tarball_url,
            Some(resolved.integrity),
        )));
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

const DEFAULT_PACKUMENT_CONCURRENCY: usize = 32; // Increased from 8 to 32 for better parallelism
const MAX_PACKUMENT_CONCURRENCY: usize = 64;
const MIN_PACKUMENT_CONCURRENCY: usize = 1;
const DEFAULT_PARALLELISM_MULTIPLIER: usize = 4; // Increased from 2 to 4 for more aggressive parallelism
const MIN_PARALLEL_CONCURRENCY: usize = 16; // Increased from 8 to 16
const MAX_PARALLEL_CONCURRENCY: usize = 64; // Increased from 32 to 64

fn packument_concurrency() -> usize {
    crate::utils::cached_env_var("JHOL_PACKUMENT_CONCURRENCY")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(MIN_PACKUMENT_CONCURRENCY, MAX_PACKUMENT_CONCURRENCY))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| {
                    (n.get() * DEFAULT_PARALLELISM_MULTIPLIER)
                        .clamp(MIN_PARALLEL_CONCURRENCY, MAX_PARALLEL_CONCURRENCY)
                })
                .unwrap_or(DEFAULT_PACKUMENT_CONCURRENCY)
        })
}

/// Parallel manifest fast-path resolve for multiple package requests.
/// Input tuple: (request_id, package_name, version_req).
/// Output tuple: (request_id, resolve_result).
pub fn parallel_resolve_tarballs_via_manifest(
    requests: &[(String, String, String)],
) -> Vec<(
    String,
    Result<Option<(String, String, Option<String>)>, String>,
)> {
    use rayon::prelude::*;

    // rayon preserves order with indexed() — no mpsc channel, no thread::spawn overhead.
    // Zero thread creation per install; pool is reused across all installs.
    let results: Vec<(
        String,
        Result<Option<(String, String, Option<String>)>, String>,
    )> = requests
        .par_iter()
        .map(|(request_id, package, version_req)| {
            let res = resolve_tarball_via_manifest(package, version_req);
            (request_id.clone(), res)
        })
        .collect();

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
        Ok(self
            .parsed
            .as_ref()
            .expect("lazy_metadata: parsed should be Some after successful parse"))
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
static METADATA_CACHE: std::sync::OnceLock<std::sync::Mutex<MetadataCache>> =
    std::sync::OnceLock::new();

fn get_metadata_cache() -> &'static std::sync::Mutex<MetadataCache> {
    METADATA_CACHE.get_or_init(|| {
        std::sync::Mutex::new(MetadataCache::new(
            std::env::var("JHOL_METADATA_CACHE_SIZE")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1000),
        ))
    })
}

/// Fetch package metadata from registry. Scoped: @scope/pkg -> @scope%2Fpkg.
/// Tries abbreviated packument (Accept: application/vnd.npm.install-v1+json) first;
/// falls back to full if unsupported or incomplete.
/// Soul Path: also checks daemon cache for pre-fetched packuments.
pub fn fetch_metadata(package: &str) -> Result<serde_json::Value, String> {
    let cache_key = format!("metadata:{}", package);

    // Try cache first
    {
        let mut cache = get_metadata_cache()
            .lock()
            .expect("metadata_cache: mutex lock failed (poisoned)");
        if let Some(cached) = cache.get(&cache_key) {
            let bytes = &cached
                .access()
                .expect("lazy_metadata: failed to access cached data")
                .raw_bytes;
            if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(bytes) {
                if parsed
                    .get("versions")
                    .and_then(|v| v.as_object())
                    .map(|o| !o.is_empty())
                    .unwrap_or(false)
                {
                    return Ok(parsed.clone());
                }
            }
        }
    }

    // Soul Path: try daemon shared memory arena
    if let Some(cached_bytes) = crate::daemon_cache::DaemonCache::lookup(package, true) {
        if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&cached_bytes) {
            if parsed
                .get("versions")
                .and_then(|v| v.as_object())
                .map(|o| !o.is_empty())
                .unwrap_or(false)
            {
                // warm in-mem cache
                let mut cache = get_metadata_cache()
                    .lock()
                    .expect("metadata_cache: mutex lock failed (poisoned)");
                cache.insert(cache_key, CachedMetadata::new(cached_bytes));
                return Ok(parsed);
            }
        }
    }

    // ============================================================
    // VORTEX CACHE PLANE: layered zero-network fast path
    // Layer 1: bundled-index synthesis (top 50 packages, zero network)
    // Layer 2: lattice_cache + manifest endpoint (~5KB vs ~50KB packument)
    // ============================================================

    // Layer 1: bundled-index synthesis. Only used when a concrete constraint is
    // known AND satisfiable by the bundled version list (see
    // `fetch_metadata_for_constraint`). With an empty constraint we cannot know
    // what the caller needs, and returning a partial packument would break
    // ranged resolution downstream — so we fall through to the full packument.

    // Layer 2: lattice_cache + manifest endpoint
    if let Some(version) = crate::lattice_cache::lookup_lattice(package, "latest") {
        if let Ok(manifest) = fetch_manifest(package, &version).and_then(|m| {
            if m.get("dependencies").is_some() || m.get("version").is_some() {
                Ok(m)
            } else {
                Err("minimal manifest".to_string())
            }
        }) {
            let resolved_version = manifest
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or(&version)
                .to_string();
            let name = manifest
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(package)
                .to_string();
            let deps: serde_json::Map<String, Value> = manifest
                .get("dependencies")
                .and_then(|d| d.as_object())
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                Value::String(v.as_str().unwrap_or("").to_string()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let opt_deps: serde_json::Map<String, Value> = manifest
                .get("optionalDependencies")
                .and_then(|d| d.as_object())
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                Value::String(v.as_str().unwrap_or("").to_string()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let tarball_url = manifest
                .get("dist")
                .and_then(|d| d.as_object())
                .and_then(|d| d.get("tarball"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let integrity = manifest
                .get("dist")
                .and_then(|d| d.as_object())
                .and_then(|d| d.get("integrity"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut version_obj = serde_json::Map::new();
            version_obj.insert("name".to_string(), Value::String(name.clone()));
            version_obj.insert(
                "version".to_string(),
                Value::String(resolved_version.clone()),
            );
            version_obj.insert("dependencies".to_string(), Value::Object(deps.clone()));
            version_obj.insert("optionalDependencies".to_string(), Value::Object(opt_deps));
            version_obj.insert("engines".to_string(), serde_json::json!({"node": ">=10"}));
            let mut dist = serde_json::Map::new();
            dist.insert("tarball".to_string(), Value::String(tarball_url));
            if !integrity.is_empty() {
                dist.insert("integrity".to_string(), Value::String(integrity));
            }
            version_obj.insert("dist".to_string(), Value::Object(dist));

            let mut versions = serde_json::Map::new();
            versions.insert(resolved_version.clone(), Value::Object(version_obj));
            let mut dist_tags = serde_json::Map::new();
            dist_tags.insert(
                "latest".to_string(),
                Value::String(resolved_version.clone()),
            );

            if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() && package == "axios" {
                eprintln!(
                    "[MANIFEST-SYNTH] {}@{} deps: {:?}",
                    package, resolved_version, deps
                );
            }

            let synth = serde_json::json!({
                "name": name,
                "versions": versions,
                "dist-tags": dist_tags,
                "_vortex_manifest_synth": true,
            });
            crate::lattice_cache::remember(package, "latest", &resolved_version);
            return Ok(synth);
        }
    }

    // VORTEX Layer 3: persistent packument disk cache.
    // After network fetches, packuments are saved to disk by write_packument_cache.
    // Check the disk cache BEFORE network to avoid RTT for packages we've seen before.
    // Try abbreviated first (smaller, faster), fall back to full packument.
    for abbreviated in &[true, false] {
        let (cached_body, _cached_etag) = read_packument_cache(package, *abbreviated);
        if let Some(body) = cached_body.filter(|b| !b.is_empty()) {
            if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) {
                if parsed
                    .get("versions")
                    .and_then(|v| v.as_object())
                    .map(|o| !o.is_empty())
                    .unwrap_or(false)
                {
                    // Cache in LRU for faster subsequent in-process access
                    let mut cache = get_metadata_cache()
                        .lock()
                        .expect("metadata_cache: mutex lock failed (poisoned)");
                    cache.insert(
                        format!("metadata:{}", package),
                        CachedMetadata::new(body.clone()),
                    );
                    cache_packument(package, &parsed);
                    return Ok(parsed);
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
    let has_versions = parsed
        .get("versions")
        .and_then(|v| v.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(true);

    if has_versions {
        // Cache in pre-resolved index + packument read cache + pre-warm index
        let mut cache = get_metadata_cache()
            .lock()
            .expect("metadata_cache: mutex lock failed (poisoned)");
        cache.insert(cache_key, CachedMetadata::new(body.clone()));
        // Populate packument read cache for fast Markov predictor path
        cache_packument(package, &parsed);
        // Soul Path: propagate to daemon arena for cross-process reuse
        crate::daemon_cache::DaemonCache::record(package, true, body);
        return Ok(parsed);
    }

    // Fall back to full packument
    let body = fetch_packument_with_etag(package, false)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    let mut cache = get_metadata_cache()
        .lock()
        .expect("metadata_cache: mutex lock failed (poisoned)");
    cache.insert(cache_key, CachedMetadata::new(body.clone()));
    cache_packument(package, &v);
    crate::daemon_cache::DaemonCache::record(package, false, body);

    Ok(v)
}

/// Fetch metadata for a package that must satisfy a concrete version constraint.
///
/// Unlike `fetch_metadata` (constraint-agnostic), this first checks the bundled
/// index with the REAL constraint: if a bundled version list contains a version
/// satisfying `constraint`, a synthesized packument is returned (zero network).
/// If the bundle cannot serve the constraint (version not bundled), it falls
/// through to `fetch_metadata`, which returns the full network packument — so a
/// partial synth can never cause a false "unresolved".
pub fn fetch_metadata_for_constraint(
    package: &str,
    constraint: &str,
) -> Result<serde_json::Value, String> {
    if let Some(synth) = synthesize_packument_from_bundled_index(package, constraint) {
        return Ok(synth);
    }
    fetch_metadata_full_packument(package)
}

/// Fetch the complete network packument, bypassing every zero-network synth
/// layer (bundled-index and manifest synth). Used as an escalation when a
/// constraint cannot be satisfied by a single-version synth — a partial
/// packument would otherwise cause a false "unresolved" for ranged requests
/// like `^18.3.1` when the bundled/manifest "latest" does not match.
pub fn fetch_metadata_full_packument(package: &str) -> Result<serde_json::Value, String> {
    let cache_key = format!("metadata:{}", package);

    // Persistent disk cache first (real, complete packuments only)
    for abbreviated in &[true, false] {
        let (cached_body, _cached_etag) = read_packument_cache(package, *abbreviated);
        if let Some(body) = cached_body.filter(|b| !b.is_empty()) {
            if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) {
                let is_synth = parsed.get("_vortex_manifest_synth").is_some()
                    || parsed.get("_bundled_index_synth").is_some();
                if !is_synth
                    && parsed
                        .get("versions")
                        .and_then(|v| v.as_object())
                        .map(|o| !o.is_empty())
                        .unwrap_or(false)
                {
                    let mut cache = get_metadata_cache()
                        .lock()
                        .expect("metadata_cache: mutex lock failed (poisoned)");
                    cache.insert(
                        format!("metadata:{}", package),
                        CachedMetadata::new(body.clone()),
                    );
                    cache_packument(package, &parsed);
                    return Ok(parsed);
                }
            }
        }
    }

    // Soul Path: try daemon shared memory arena for a complete packument
    if let Some(cached_bytes) = crate::daemon_cache::DaemonCache::lookup(package, true) {
        if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&cached_bytes) {
            if parsed
                .get("versions")
                .and_then(|v| v.as_object())
                .map(|o| !o.is_empty())
                .unwrap_or(false)
            {
                let mut cache = get_metadata_cache()
                    .lock()
                    .expect("metadata_cache: mutex lock failed (poisoned)");
                cache.insert(cache_key, CachedMetadata::new(cached_bytes));
                return Ok(parsed);
            }
        }
    }

    // Network (no synth fallbacks)
    let body = match fetch_packument_with_etag(package, true) {
        Ok(b) => b,
        Err(_) => fetch_packument_with_etag(package, false)?,
    };
    let parsed: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    cache_in_index(package, &parsed);
    let mut cache = get_metadata_cache()
        .lock()
        .expect("metadata_cache: mutex lock failed (poisoned)");
    cache.insert(cache_key, CachedMetadata::new(body.clone()));
    cache_packument(package, &parsed);
    crate::daemon_cache::DaemonCache::record(package, true, body);

    Ok(parsed)
}

/// Fetch metadata for multiple packages in parallel using rayon
pub fn fetch_metadata_parallel(
    packages: &[String],
) -> Vec<(String, Result<serde_json::Value, String>)> {
    use rayon::prelude::*;

    // Check packument read cache first (O(1), non-blocking)
    let mut cached: Vec<(String, Result<serde_json::Value, String>)> = Vec::new();
    let mut to_fetch: Vec<String> = Vec::new();
    for pkg in packages {
        if let Some(meta) = get_cached_packument(pkg) {
            cached.push((pkg.clone(), Ok(meta)));
        } else {
            to_fetch.push(pkg.clone());
        }
    }

    if to_fetch.is_empty() {
        return cached;
    }

    // Network fetch for uncached packages via rayon
    let fetched: Vec<(String, Result<serde_json::Value, String>)> = to_fetch
        .par_iter()
        .map(|(package)| {
            let result = fetch_metadata(package);
            (package.clone(), result)
        })
        .collect();

    // Merge in input order via position index
    let pos: HashMap<&str, usize> = packages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i))
        .collect();
    let mut all = cached;
    all.extend(fetched);
    all.sort_by(|a, b| {
        pos.get(a.0.as_str())
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(&pos.get(b.0.as_str()).copied().unwrap_or(usize::MAX))
    });
    all
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
    // Cache in zero-copy metadata mmap for future cross-project hits
    if let Some(versions) = meta.get("versions").and_then(|v| v.as_object()) {
        for (ver, pkg_data) in versions {
            if let Some(obj) = pkg_data.as_object() {
                let val: serde_json::Value = serde_json::Value::Object(obj.clone());
                crate::zero_copy_metadata::cache_packument_metadata(package, ver, &val);
            }
        }
    }
    Ok(meta)
}

/// Fetch packuments for multiple packages in parallel. Uses shared cache to avoid duplicate fetches.
/// Fetch packuments for multiple packages in parallel using rayon.
///
/// Uses the global rayon threadpool (64 persistent threads, zero spawn overhead per call).
/// Packuments are checked against disk cache before network, then published to the
/// shared in-process cache and zero-copy mmap store. Results are returned in input order.
pub fn parallel_fetch_metadata(
    names: &[String],
    cache: &std::sync::Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
) -> Vec<(String, Result<serde_json::Value, String>)> {
    use rayon::prelude::*;

    let mut cached_results = Vec::new();
    let mut to_fetch: Vec<String> = Vec::new();

    // Separate cached vs uncached in one lock acquisition
    {
        let guard = cache
            .lock()
            .expect("metadata_cache: mutex lock failed (poisoned)");
        for name in names {
            if let Some(cached) = guard.get(name) {
                cached_results.push((name.clone(), Ok(cached.clone())));
            } else {
                to_fetch.push(name.clone());
            }
        }
    }

    if to_fetch.is_empty() {
        return cached_results;
    }

    // VORTEX Layer 3: check disk packument cache BEFORE rayon spawn.
    // If packument is on disk, we get it without any network RTT.
    let mut disk_hits: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    for name in &to_fetch {
        for abbreviated in &[true, false] {
            if let (Some(body), _) = read_packument_cache(name, *abbreviated) {
                if body.is_empty() {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) {
                    if parsed
                        .get("versions")
                        .and_then(|v| v.as_object())
                        .map(|o| !o.is_empty())
                        .unwrap_or(false)
                    {
                        disk_hits.insert(name.clone(), parsed);
                        break;
                    }
                }
            }
        }
    }

    // Add disk hits to results and remove from to-fetch
    let mut still_need_network: Vec<String> = Vec::new();
    for name in &to_fetch {
        if let Some(meta) = disk_hits.get(name) {
            cached_results.push((name.clone(), Ok(meta.clone())));
        } else {
            still_need_network.push(name.clone());
        }
    }

    // Publish disk hits to in-process cache
    {
        let mut guard = cache.lock().expect("metadata_cache: poisoned");
        for (name, meta) in &disk_hits {
            guard.insert(name.clone(), meta.clone());
        }
    }

    if still_need_network.is_empty() {
        let mut results = cached_results;
        let name_pos: std::collections::HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        results.sort_by(|a, b| {
            let pa = name_pos.get(a.0.as_str()).copied().unwrap_or(usize::MAX);
            let pb = name_pos.get(b.0.as_str()).copied().unwrap_or(usize::MAX);
            pa.cmp(&pb)
        });
        return results;
    }

    // Network fetch for remaining packages via global rayon pool (zero spawn overhead)
    let fetched: Vec<(String, Result<serde_json::Value, String>)> = still_need_network
        .into_par_iter()
        .map(|name| {
            let res = fetch_metadata(&name);
            if let Ok(ref meta) = res {
                let is_synth = meta.get("_vortex_manifest_synth").is_some()
                    || meta.get("_bundled_index_synth").is_some();
                // Never persist synth packuments to the disk cache: a
                // single-version synth would later be read back by
                // fetch_metadata_full_packument and shadow the real (complete)
                // packument, causing false "unresolved" for ranged constraints.
                if !is_synth {
                    let body = serde_json::to_vec(meta).unwrap_or_default();
                    write_packument_cache(&name, false, &body, None);
                }
                if let Some(versions) = meta.get("versions").and_then(|v| v.as_object()) {
                    for (ver, pkg_data) in versions {
                        if let Some(obj) = pkg_data.as_object() {
                            let val = serde_json::Value::Object(obj.clone());
                            crate::zero_copy_metadata::cache_packument_metadata(&name, ver, &val);
                        }
                    }
                }
            }
            (name, res)
        })
        .collect();

    // Publish network results to in-process cache
    for (name, ref res) in &fetched {
        if let Ok(meta) = res {
            if let Ok(mut guard) = cache.lock() {
                guard.insert(name.clone(), meta.clone());
            }
        }
    }

    let mut results = cached_results;
    results.extend(fetched);
    let name_pos: std::collections::HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    results.sort_by(|a, b| {
        let pa = name_pos.get(a.0.as_str()).copied().unwrap_or(usize::MAX);
        let pb = name_pos.get(b.0.as_str()).copied().unwrap_or(usize::MAX);
        pa.cmp(&pb)
    });
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

/// Validate multiple packages in parallel using rayon.
pub fn parallel_validate_packages(packages: &[String], _timeout_secs: u64) -> Vec<(String, bool)> {
    use rayon::prelude::*;
    packages
        .par_iter()
        .map(|pkg| (pkg.clone(), validate_package_exists(pkg).unwrap_or(false)))
        .collect()
}

/// Check if a concrete version satisfies a semver range/spec.
pub fn version_satisfies(spec: &str, version: &str) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return Version::parse(version).is_ok();
    }
    // npm semantics: a bare version pin (e.g. `4.1.6`) is EXACT, NOT caret.
    // `semver::VersionReq::parse("4.1.6")` behaves like `^4.1.6` and wrongly
    // satisfies 4.2.11. Honour the exact pin before building a VersionReq.
    if is_exact_version_pin(spec) {
        return {
            let v = Version::parse(version);
            match v {
                Ok(v) => {
                    let bare = spec
                        .trim()
                        .strip_prefix('v')
                        .or_else(|| spec.trim().strip_prefix('=').map(|x| x.trim()))
                        .unwrap_or(spec.trim());
                    match Version::parse(bare) {
                        Ok(pin) => v == pin,
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            }
        };
    }
    // npm supports `||` for OR-combined ranges (e.g. `^3.0.0 || ^4.0.0`); the
    // `semver` crate's VersionReq::parse rejects it, so split on `||` and accept
    // if any alternative matches.
    if spec.contains("||") {
        return spec.split("||").any(|part| {
            let part = part.trim();
            if part.is_empty() {
                return false;
            }
            match cached_version_req(part) {
                Some(req) => Version::parse(version).map_or(false, |v| req.matches(&v)),
                None => false,
            }
        });
    }
    let Some(req) = cached_version_req(spec) else {
        return false;
    };
    let v = match Version::parse(version) {
        Ok(v) => v,
        Err(_) => return false,
    };
    req.matches(&v)
}

/// True if any version in the packument satisfies at least one of the given
/// constraints. Used to detect when a single-version synth packument cannot
/// satisfy a ranged request (e.g. latest is 2.x but the constraint pins <2.0.0).
pub fn meta_has_satisfying_version(meta: &serde_json::Value, constraints: &[String]) -> bool {
    if constraints.is_empty() {
        return true;
    }
    let Some(versions) = meta.get("versions").and_then(|v| v.as_object()) else {
        return false;
    };
    versions
        .keys()
        .any(|v| constraints.iter().any(|c| version_satisfies(c, v)))
}

/// True if the packument contains the exact given version.
pub fn meta_contains_version(meta: &serde_json::Value, version: &str) -> bool {
    meta.get("versions")
        .and_then(|v| v.as_object())
        .map(|o| o.contains_key(version))
        .unwrap_or(false)
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
        .filter(|v| {
            if is_exact_version_pin(range) {
                version_satisfies(range, &v.to_string())
            } else {
                req.matches(v)
            }
        })
        .collect();
    satisfying.sort();
    satisfying.last().map(|v| v.to_string())
}

/// Same as resolve_range but takes pre-parsed, sorted (ascending) version list.
/// Avoids re-parsing and re-sorting when caller already has parsed versions.
pub fn resolve_range_from_parsed(
    parsed: &[(semver::Version, &str)],
    range: &str,
) -> Option<String> {
    let range = range.trim();
    if range.is_empty() || range == "*" {
        return parsed.last().map(|(_, s)| s.to_string());
    }
    let req = semver::VersionReq::parse(range).ok()?;
    for (ver, ver_str) in parsed.iter().rev() {
        if is_exact_version_pin(range) {
            if version_satisfies(range, &ver.to_string()) {
                return Some(ver_str.to_string());
            }
        } else if req.matches(ver) {
            return Some(ver_str.to_string());
        }
    }
    None
}

/// Resolve version to a concrete semver (e.g. "latest" -> "1.2.3", "^1.0" -> "1.2.3")
pub fn resolve_version(meta: &serde_json::Value, version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() || version == "latest" {
        let dist_tags = meta.get("dist-tags")?.as_object()?;
        return dist_tags
            .get("latest")
            .and_then(|v| v.as_str())
            .map(String::from);
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
    if let Some(deps) = ver_obj
        .get("optionalDependencies")
        .and_then(|d| d.as_object())
    {
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
    if let Some(meta_obj) = ver_obj
        .get("peerDependenciesMeta")
        .and_then(|d| d.as_object())
    {
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
    use flate2::read::GzDecoder;
    use std::io::{Read, Write};
    use tar::Archive;

    // Create temp file for download
    let tmp = std::env::temp_dir().join(format!(
        "jhol-stream-{}-{}.tgz",
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
static BINARY_PACKAGE_DEPS_INDEX: OnceLock<Option<HashMap<String, HashMap<String, String>>>> =
    OnceLock::new();

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

/// Split a `name@version` or `@scope/name@version` spec into (name, version).
/// PRW mode embeds no tarballs, so on-disk lookups use the parts.
pub fn split_binary_spec(spec: &str) -> Option<(String, String)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let at = spec.rfind('@')?;
    if at == 0 {
        return None;
    }
    let name = &spec[..at];
    let version = &spec[at + 1..];
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some((name.to_string(), version.to_string()))
}

pub fn load_binary_tarball(package: &str, version: &str) -> Result<Option<Vec<u8>>, String> {
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

    let data = std::fs::read(&archive_path).map_err(|e| {
        format!(
            "failed to read binary package {}: {}",
            archive_path.display(),
            e
        )
    })?;
    let tarball = decode_binary_package_archive(&data)?;
    Ok(Some(tarball))
}

pub fn binary_package_dependencies(
    _package: &str,
    _version: &str,
) -> Option<HashMap<String, String>> {
    // PRW: bundle-backed dependency lookup is retired. Transitive dependency
    // discovery now happens through the parallel network resolution wave +
    // persistent mirror, not from embedded/stored tarballs.
    None
}

/// Parse dependencies from raw gzip tarball bytes (decompress, find package.json, read deps).
fn read_deps_from_tarball_bytes(tgz: &[u8]) -> Option<HashMap<String, String>> {
    let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(tgz));
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

    if shared_unpacked.exists() && store_extract_complete(&shared_unpacked) {
        return Ok(Some(shared_unpacked));
    }
    remove_store_dir(&shared_unpacked);

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
    mark_store_extract_complete(&tmp);

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
    remove_store_dir(dst);

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
///
/// PRW: the embedded/local bundle is intentionally gone. Resolution now comes
/// from parallel network packument fetches (see `parallel_resolve_tarballs_via_manifest`)
/// + the persistent mirror. This returns `None` so the resolver always takes the
/// network/mirror path — no stored bytes.
pub fn best_binary_version_matching(_package: &str, _constraints: &[String]) -> Option<String> {
    None
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

            // JAGR-S: store tarball bytes in append-only log
            let _ = crate::jagrs::store_tarball(&tarball);

            // Pre-populate sharded unpacked store to avoid a second decompression pass in cold installs.
            let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
            let unpacked = cache_dir.join("store_unpacked").join(shard).join(&hash);
            if !store_extract_complete(&unpacked) {
                remove_store_dir(&unpacked);
                let mut unpacked_ready = false;
                if let Some(shared_unpacked) = ensure_shared_binary_unpacked(&hash, &tarball)? {
                    if store_extract_complete(&shared_unpacked) {
                        unpacked_ready = link_or_copy_dir(&shared_unpacked, &unpacked).is_ok();
                    }
                }
                if !unpacked_ready {
                    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
                    if let Err(e) = extract_tarball_bytes_to_dir(&tarball, &unpacked) {
                        remove_store_dir(&unpacked);
                        return Err(e);
                    }
                    mark_store_extract_complete(&unpacked);
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

    // JAGR-S: store tarball bytes + alias; full unpack once (Soul Path content plane)
    if let Ok(data) = std::fs::read(&tmp) {
        let _ = crate::jagrs::store_tarball(&data);
        if let Some((package, version)) = parse_exact_package_spec(pkg_key) {
            let _ = crate::jagrs::store_alias(&package, &version, &hash);
        }
        let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
        let unpacked = cache_dir.join("store_unpacked").join(shard).join(&hash);
        if !store_extract_complete(&unpacked) {
            remove_store_dir(&unpacked);
            if let Err(e) = std::fs::create_dir_all(&unpacked)
                .map_err(|e| e.to_string())
                .and_then(|_| extract_tarball_bytes_to_dir(&data, &unpacked))
            {
                remove_store_dir(&unpacked);
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            mark_store_extract_complete(&unpacked);
        }
    }

    let store_file = store_dir.join(format!("{}.tgz", hash));
    std::fs::rename(&tmp, &store_file)
        .or_else(|_| std::fs::copy(&tmp, &store_file).map(|_| ()))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    Ok(hash)
}

/// Decompress gzip data with libdeflate. Uses the gzip ISIZE trailer as the
/// initial output size and grows on InsufficientSpace.
fn decompress_gzip_libdeflate(gz: &[u8]) -> Result<Vec<u8>, String> {
    let mut decompressor = libdeflater::Decompressor::new();
    let initial_size = if gz.len() >= 4 {
        let last4 = &gz[gz.len() - 4..];
        u32::from_le_bytes([last4[0], last4[1], last4[2], last4[3]]) as usize
    } else {
        gz.len().saturating_mul(4)
    };
    let mut out = vec![0u8; initial_size.max(4096)];
    loop {
        match decompressor.gzip_decompress(gz, &mut out) {
            Ok(n) => {
                out.truncate(n);
                return Ok(out);
            }
            Err(libdeflater::DecompressionError::InsufficientSpace) => {
                out.resize(out.len().saturating_mul(2), 0);
            }
            Err(libdeflater::DecompressionError::BadData) => {
                return Err("libdeflate: bad gzip data".to_string());
            }
        }
    }
}

/// Extract a pre-decompressed tar archive to a directory, stripping the top-level
/// directory entry (package/... -> ...).
fn extract_tar_from_bytes(decompressed: &[u8], dest_dir: &Path) -> Result<(), String> {
    use std::io::Cursor;
    let mut archive = tar::Archive::new(Cursor::new(decompressed));
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path_str = path.to_string_lossy();
        let parts: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() <= 1 {
            continue;
        }
        let rel = parts[1..].join(std::path::MAIN_SEPARATOR_STR);
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

/// marker file name proving a store extraction completed (every file on disk).
/// Guards against partial extracts (kill mid-tar, stale manifest-only caches)
/// that would otherwise shadow the real content forever because the store
/// populate paths short-circuit on `dir.exists()`.
const STORE_COMPLETE_MARKER: &str = ".jhol-extract-complete";

/// Write the completion marker after a successful full extraction.
fn mark_store_extract_complete(dest_dir: &Path) {
    let _ = std::fs::write(dest_dir.join(STORE_COMPLETE_MARKER), "ok");
}

/// True when a store dir was fully extracted (marker present) OR contains real
/// content beyond an empty shell. A dir holding only `package.json` (the first
/// tar entry — the classic partial-extract signature) is treated as incomplete,
/// even if runtime relink `node_modules/` symlink dirs were added afterwards.
pub fn store_extract_complete(dest_dir: &Path) -> bool {
    if dest_dir.join(STORE_COMPLETE_MARKER).exists() {
        return true;
    }
    // Legacy mark-free dirs: a real body file (regular file beyond package.json)
    // proves the tarball was fully materialized. The relink `node_modules/`
    // sibling is a symlink dir and must not satisfy this.
    let pkg_json = dest_dir.join("package.json");
    if !pkg_json.exists() {
        return false;
    }
    if let Ok(rd) = std::fs::read_dir(dest_dir) {
        for ent in rd.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if name == "package.json" || name == STORE_COMPLETE_MARKER {
                continue;
            }
            if ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return true;
            }
        }
        return false;
    }
    false
}

/// Single-pass cold download → unpack.
///
/// Downloads the tarball, verifies SRI integrity inline, computes the SHA-256
/// content hash, decompresses with libdeflate, extracts directly to
/// store_unpacked/<shard>/<hash>/, and keeps the original .tgz in store/.
/// Returns (hash, unpacked_path).
pub fn download_unpack_to_store_unpacked(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
) -> Result<(String, PathBuf), String> {
    // Prefer pre-built binary package if available (exact spec only).
    if let Some((package, version)) = parse_exact_package_spec(pkg_key) {
        if let Ok(Some(tarball)) = load_binary_tarball(&package, &version) {
            if let Some(sri) = expected_integrity {
                if !crate::utils::verify_sri_bytes(&tarball, sri) {
                    return Err(format!("Integrity check failed for {}", pkg_key));
                }
            }
            let hash = format!("{:x}", Sha256::digest(&tarball));
            let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
            let unpacked = cache_dir.join("store_unpacked").join(shard).join(&hash);
            if !store_extract_complete(&unpacked) {
                remove_store_dir(&unpacked);
                std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
                extract_tar_from_bytes(&tarball, &unpacked)?;
                mark_store_extract_complete(&unpacked);
            }
            let store_file = cache_dir.join("store").join(format!("{}.tgz", hash));
            if !store_file.exists() {
                let _ = std::fs::write(&store_file, &tarball);
            }
            return Ok((hash, unpacked));
        }
    }

    let token = registry_auth_token();
    let bytes = crate::http_client::get_bytes_with_bearer(url, token.as_deref())
        .map_err(|e| format!("download: {}", e))?;

    if let Some(sri) = expected_integrity {
        if !crate::utils::verify_sri_bytes(&bytes, sri) {
            return Err(format!("Integrity check failed for {}", pkg_key));
        }
    }

    let hash = format!("{:x}", Sha256::digest(&bytes));
    let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
    let unpacked = cache_dir.join("store_unpacked").join(shard).join(&hash);
    if !store_extract_complete(&unpacked) {
        remove_store_dir(&unpacked);
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        let decompressed = decompress_gzip_libdeflate(&bytes)?;
        extract_tar_from_bytes(&decompressed, &unpacked)?;
        mark_store_extract_complete(&unpacked);
    }

    let store_file = cache_dir.join("store").join(format!("{}.tgz", hash));
    if !store_file.exists() {
        if let Some(parent) = store_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&store_file, &bytes);
    }

    Ok((hash, unpacked))
}

/// Public wrapper for marking a completed store extraction.
pub fn mark_store_extract_complete_pub(dest_dir: &Path) {
    mark_store_extract_complete(dest_dir);
}

/// Public wrapper for removing a store dir or broken symlink.
pub fn remove_store_dir_pub(path: &Path) {
    remove_store_dir(path);
}

/// Remove a store dir (or broken symlink) so a fresh extract can replace it.
fn remove_store_dir(path: &Path) {
    use std::fs;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// Ensure the tarball is unpacked in sharded store_unpacked/ab/<hash>; return that path.
pub fn ensure_unpacked_in_store(tarball_path: &Path, cache_dir: &Path) -> Result<PathBuf, String> {
    let hash = tarball_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid store path".to_string())?;
    let shard = if hash.len() >= 2 { &hash[..2] } else { "xx" };
    let store_unpacked_base = cache_dir.join("store_unpacked").join(shard);
    let unpacked = store_unpacked_base.join(hash);
    if !store_extract_complete(&unpacked) {
        remove_store_dir(&unpacked);
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        extract_tarball_to_dir(tarball_path, &unpacked)?;
        mark_store_extract_complete(&unpacked);
    }
    Ok(unpacked)
}

/// Public full extract from in-memory tarball bytes (Soul Path / JAGR-S).
pub fn extract_tarball_bytes_public(tarball_data: &[u8], dest_dir: &Path) -> Result<(), String> {
    extract_tarball_bytes_to_dir(tarball_data, dest_dir)
}

fn extract_tarball_bytes_to_dir(tarball_data: &[u8], dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let decompressed = decompress_gzip_libdeflate(tarball_data)?;
    extract_tar_from_bytes(&decompressed, dest_dir)
}

/// Extract .tgz into dest_dir, stripping one top-level directory from tarball.
pub fn extract_tarball_to_dir(tarball_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let data = std::fs::read(tarball_path).map_err(|e| e.to_string())?;
    let decompressed = decompress_gzip_libdeflate(&data)?;
    extract_tar_from_bytes(&decompressed, dest_dir)
}

/// Extract .tgz to node_modules/<package_name>. Strips one top-level directory.
pub fn extract_tarball(
    tarball_path: &Path,
    node_modules_dir: &Path,
    package_name: &str,
) -> Result<(), String> {
    let package_name = crate::utils::sanitize_package_name(package_name)
        .ok_or_else(|| format!("Invalid package name: {}", package_name))?;
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
            (
                package[..idx].trim_end_matches('@'),
                package[idx + 1..].trim(),
            )
        } else {
            (package, "latest")
        }
    } else {
        (package, "latest")
    };
    let version = resolve_version(&meta, version_req)
        .ok_or_else(|| format!("could not resolve version {}", version_req))?;
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

// ── Bundled (embedded) tarball cache ────────────────────────────────────────
// The build script generates bundled_packages_data.rs which embeds .jhol
// archives for 200 popular packages.  We wrap in a private module to avoid
// import conflicts with this file's own `use std::collections::HashMap`.
mod bundled_data {
    include!(concat!(env!("OUT_DIR"), "/bundled_packages_data.rs"));
}

/// Per-package decode cache: once a bundled .jhol is decoded to gzip tarball
/// bytes, the result is cached so the install phase doesn't re-decode.
/// This is the key optimisation that eliminates the redundant second
/// decompression described in the single-pass expansion extraction plan.
static BUNDLED_DECODE_CACHE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();

/// Return the decoded gzip tarball bytes for a bundled package `name@version`.
/// The result is cached per-package so that expansion and install share the
/// same decompressed bytes — the install path never re-decodes.
pub fn get_bundled_tarball(spec: &str) -> Option<Vec<u8>> {
    // Fast path: cached from a previous decode (e.g. during expansion).
    let cache = BUNDLED_DECODE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(spec) {
        return Some(cached.clone());
    }
    // Cold path: binary-search the sorted static array, decode one .jhol.
    let entries = bundled_data::get_bundled_package_data();
    let idx = entries
        .binary_search_by(|(key, _)| key.as_bytes().cmp(spec.as_bytes()))
        .ok()?;
    let (_, archive) = entries[idx];
    let bytes = decode_binary_package_archive(archive).ok()?;
    // Cache so the next caller (install phase) doesn't re-decode.
    cache
        .lock()
        .unwrap()
        .insert(spec.to_string(), bytes.clone());
    Some(bytes)
}

/// Expose the decode cache for the install phase to check without calling
/// `get_bundled_tarball` again (avoids the HashMap lookup inside the cache).
pub fn get_bundled_cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    BUNDLED_DECODE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pre-seed the store index with all 200 bundled package hashes at startup.
/// Uses the pre-computed hashes from build time so no .jhol decoding is needed.
/// This lets `get_cached_tarball_fast` immediately find bundled packages
/// without falling through to the materialisation path.
pub fn preseed_store_index() {
    let mut index = crate::utils::read_store_index();
    let mut changed = false;
    // PRW mode: the embedded hash table is empty. Seed from the on-disk
    // binary-packages index.json instead so cached lookups hit immediately.
    if let Some(dir) = binary_packages_dir() {
        let index_path = dir.join("index.json");
        if let Ok(raw) = std::fs::read_to_string(&index_path) {
            if let Ok(spec_map) = serde_json::from_str::<HashMap<String, String>>(&raw) {
                for (spec, hash) in spec_map {
                    if !index.contains_key(&spec) {
                        index.insert(spec, hash);
                        changed = true;
                    }
                }
            }
        }
    }
    let hashes = bundled_data::get_bundled_hashes();
    for (spec, hash) in hashes {
        if !index.contains_key(*spec) {
            index.insert(spec.to_string(), hash.to_string());
            changed = true;
        }
    }
    if changed {
        let _ = crate::utils::write_store_index(&index);
    }
}

/// Check if a spec key exists in the build-time bundled hash table.
/// O(log n) binary search — roughly matches how `get_bundled_tarball` would find it.
pub fn is_bundled_spec(spec: &str) -> bool {
    let hashes = bundled_data::get_bundled_hashes();
    hashes
        .binary_search_by(|(key, _)| key.as_bytes().cmp(spec.as_bytes()))
        .is_ok()
}
