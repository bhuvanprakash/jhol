//! Native npm registry client: fetch metadata and tarballs via HTTP.
//! All HTTP calls go through `crate::http_client` (shared Agent = TCP connection pool).

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};

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
    let mut hasher = Sha256::new();
    hasher.update(package.as_bytes());
    hasher.update(if abbreviated { b"abbr" } else { b"full" });
    format!("{:x}", hasher.finalize())
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
                _ => Err(format!("Empty packument body for {} after 304 retry", package)),
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

fn encoded_package_path(package: &str) -> String {
    if package.starts_with('@') {
        package.replace('/', "%2F")
    } else {
        package.to_string()
    }
}

/// Fetch a specific version manifest (/<pkg>/latest or /<pkg>/<version>).
/// Routes through the global shared HTTP client — reuses existing TCP connection.
fn fetch_manifest(package: &str, selector: &str) -> Result<serde_json::Value, String> {
    let path = encoded_package_path(package);
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

/// Fast-path resolve for latest/exact specs using the manifest endpoint
/// (`/<pkg>/latest` or `/<pkg>/<version>`), avoiding large packument downloads.
/// Returns Some((resolved_version, tarball_url, integrity)).
pub fn resolve_tarball_via_manifest(
    package: &str,
    version_req: &str,
) -> Result<Option<(String, String, Option<String>)>, String> {
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

fn packument_concurrency() -> usize {
    std::env::var("JHOL_PACKUMENT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 64))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 2).clamp(8, 32))
                .unwrap_or(8)
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

/// Fetch package metadata from registry. Scoped: @scope/pkg -> @scope%2Fpkg.
/// Tries abbreviated packument (Accept: application/vnd.npm.install-v1+json) first;
/// falls back to full if unsupported or incomplete.
pub fn fetch_metadata(package: &str) -> Result<serde_json::Value, String> {
    let body = match fetch_packument_with_etag(package, true) {
        Ok(b) => b,
        Err(_) => fetch_packument_with_etag(package, false)?,
    };
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if v.get("versions").and_then(|v| v.as_object()).map(|o| o.is_empty()).unwrap_or(true) {
        let body = fetch_packument_with_etag(package, false)?;
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        return Ok(v);
    }
    Ok(v)
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
    for chunk in names.chunks(concurrency) {
        let (tx, rx) = mpsc::channel();
        for name in chunk {
            let name = name.clone();
            let tx = tx.clone();
            let cache = std::sync::Arc::clone(cache);
            thread::spawn(move || {
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

/// Download tarball to store and return hash. Does not update the store index.
pub fn download_tarball_to_store_hash_only(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
) -> Result<String, String> {
    let store_dir = cache_dir.join("store");
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;
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
        println!("Installed {}@{} (native)", base_name, version);
    }
    Ok(())
}
