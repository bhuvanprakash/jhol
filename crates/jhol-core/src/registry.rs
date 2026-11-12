//! Native npm registry client: fetch metadata and tarballs via HTTP.
//! All HTTP calls go through `crate::http_client` (shared Agent = TCP connection pool).
//! Optimized with pre-resolved package index for instant resolution of common packages.

use crate::packument::PackumentRecord;
use semver::{Version, VersionReq};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::any::Any;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

/// Cache for parsed semver requirements — avoids re-parsing common specs
/// like "^1.0.0", ">=2.0.0", "*", etc. during resolution.
pub(crate) fn cached_version_req(spec: &str) -> Option<VersionReq> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<VersionReq>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().ok()?;
    if let Some(cached) = guard.get(spec) {
        return cached.clone();
    }
    let parsed = parse_version_req(spec);
    guard.insert(spec.to_string(), parsed.clone());
    parsed
}

/// Parse a semver range with npm semantics.
///
/// The Rust `semver` crate requires commas between comparators, but npm accepts
/// whitespace: `>=0.6.2 <2.0.0`. Without this, valid ranges were rejected,
/// which made JAGR-infinite report false "legality" failures on real trees
/// (e.g. `sass@1.104.1 requires source-map-js >=0.6.2 <2.0.0`).
fn parse_version_req(spec: &str) -> Option<VersionReq> {
    let spec = spec.trim();
    if let Ok(req) = VersionReq::parse(spec) {
        return Some(req);
    }
    let tokens: Vec<&str> = spec.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    // `>=0.6.2 <2.0.0` -> `>=0.6.2, <2.0.0`
    if let Ok(req) = VersionReq::parse(&tokens.join(", ")) {
        return Some(req);
    }
    // `>= 0.6.2 < 2.0.0` -> merge a bare operator token with the next token,
    // then comma-join: `>=0.6.2, <2.0.0`.
    let is_op = |t: &str| matches!(t, ">" | ">=" | "<" | "<=" | "=" | "^" | "~");
    let mut merged: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if is_op(tokens[i]) && i + 1 < tokens.len() {
            merged.push(format!("{}{}", tokens[i], tokens[i + 1]));
            i += 2;
        } else {
            merged.push(tokens[i].to_string());
            i += 1;
        }
    }
    if merged.len() >= 2 {
        if let Ok(req) = VersionReq::parse(&merged.join(", ")) {
            return Some(req);
        }
    }
    None
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
    // `x`/`*` are wildcards. A hyphen means a *hyphen range* (`1.2.3 - 2.0.0`) only
    // when it is spaced like one — a bare `-` also appears in every prerelease
    // (`1.64.0-alpha-2026-09-16`), and treating those as ranges meant a pinned
    // prerelease was never honoured exactly.
    if bare.contains(" - ") {
        // A spaced hyphen is a hyphen range (`1.2.3 - 2.0.0`), never a pin.
        return false;
    }
    // Wildcards belong to the **version core**, not to the prerelease. Testing the whole string
    // meant any prerelease containing the letter `x` was read as a wildcard range:
    // `0.1.6-no-e**x**ternal-plugins` (a real pin from @babel/preset-env) matched `^0.1.6-…`
    // instead of the exact release, so we installed `0.1.6` where the publisher pinned the tagged
    // string. Many prereleases contain `x` (`-next`, `-experimental`, `-external`), so this was
    // mis-resolving more than one package.
    let core = bare.split('-').next().unwrap_or(bare);
    if core.contains('x') || core.contains('X') || core.contains('*') {
        return false;
    }
    if semver::Version::parse(bare).is_ok() {
        // Reject partial pins like `1.2` or `1` (npm treats those as ranges, not exact).
        return core.split('.').count() >= 3;
    }
    false
}

/// Global packument read cache.
///
/// E60 (round 4): this used to *retain a second parsed copy* of every fetched
/// packument — `packument_read_cache` held a pruned `Value` clone per package
/// forever (unbounded), on top of the map `fetch_dep_tree` already keeps. On the
/// resolution fixture that was ~130 MB of the 262 MB peak, i.e. half of it. The
/// readers below now answer from the byte-level LRU (`get_metadata_cache`),
/// which every fetch path already fills, and parse on demand: same data, same
/// hit/miss behaviour, one fewer full copy of the metadata set. Public API
/// unchanged (`in_packument_cache`, `get_cached_packument`, `cache_packument`).

/// Registry and credential identity captured for one metadata operation.
/// Token stays in memory only; cache-key helpers hash it before persistence.
#[derive(Clone)]
pub(crate) struct RegistryIdentity {
    pub(crate) registry: String,
    pub(crate) token: Option<String>,
}

impl RegistryIdentity {
    pub(crate) fn for_package(package: &str) -> Self {
        Self::for_package_in(Path::new("."), package)
    }

    pub(crate) fn for_package_in(dir: &Path, package: &str) -> Self {
        let registry = crate::config::effective_registry_url_for_package(dir, package);
        let token = crate::config::registry_auth_token_for_url(dir, &registry);
        Self { registry, token }
    }
}

pub(crate) fn metadata_cache_key_for(package: &str, registry: &str, token: Option<&str>) -> String {
    format!(
        "metadata-v2:{}",
        crate::origin::packument_key_for(package, true, registry, token)
    )
}

/// Fingerprint the effective default and every configured scoped registry.
/// Cache keys contain only digests, never registry credentials.
pub(crate) fn registry_configuration_fingerprint(dir: &Path) -> String {
    let mut scopes = std::collections::BTreeSet::new();
    if let Some(config) = crate::config::load_jholrc_registry(dir) {
        scopes.extend(config.scopes.into_keys());
    }
    scopes.extend(crate::config::load_npmrc(dir).scoped_registries.into_keys());

    let mut probes = vec!["__jhol_default_registry_probe__".to_string()];
    for scope in scopes {
        let scope = scope.trim_start_matches('@');
        if !scope.is_empty() {
            probes.push(format!("@{scope}/__jhol_registry_probe__"));
        }
    }
    probes.sort();
    probes.dedup();

    let mut hash = Sha256::new();
    hash.update(b"jhol-registry-configuration-v1\0");
    for package in probes {
        let identity = RegistryIdentity::for_package_in(dir, &package);
        let key = metadata_cache_key_for(&package, &identity.registry, identity.token.as_deref());
        for field in [package.as_bytes(), key.as_bytes()] {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field);
        }
    }
    format!("{:x}", hash.finalize())
}

fn metadata_cache_key(package: &str) -> String {
    metadata_cache_key_with_identity(package, &RegistryIdentity::for_package(package))
}

fn metadata_cache_key_with_identity(package: &str, identity: &RegistryIdentity) -> String {
    metadata_cache_key_for(package, &identity.registry, identity.token.as_deref())
}

pub(crate) fn lookup_zero_copy_packument(
    package: &str,
) -> Option<crate::zero_copy_metadata::PackageMetadata> {
    let identity = RegistryIdentity::for_package(package);
    let key = metadata_cache_key_with_identity(package, &identity);
    crate::zero_copy_metadata::MmapMetadataStore::lookup_packument_for_key(&key)
}

pub(crate) fn remember_lattice_for_current_identity(
    package: &str,
    constraint: &str,
    version: &str,
) {
    let identity = RegistryIdentity::for_package(package);
    let key = metadata_cache_key_with_identity(package, &identity);
    crate::lattice_cache::remember_for_key(package, constraint, version, &key);
}

pub(crate) fn lookup_lattice_for_current_identity(
    package: &str,
    constraint: &str,
) -> Option<String> {
    let identity = RegistryIdentity::for_package(package);
    let key = metadata_cache_key_with_identity(package, &identity);
    crate::lattice_cache::lookup_lattice_for_key(&key, constraint)
}

/// Check if a packument is already in the read cache (O(1), non-blocking read).
pub fn in_packument_cache(package: &str) -> bool {
    let key = metadata_cache_key(package);
    match get_metadata_cache().lock() {
        Ok(mut cache) => cache.get(&key).is_some(),
        Err(_) => false,
    }
}

/// Look up a packument from the read cache without blocking.
///
/// Parses the retained body on demand instead of keeping a second `Value` tree
/// resident (see the note above).
pub fn get_cached_packument(package: &str) -> Option<serde_json::Value> {
    get_cached_packument_with_identity(package, &RegistryIdentity::for_package(package))
}

fn get_cached_packument_with_identity(
    package: &str,
    identity: &RegistryIdentity,
) -> Option<serde_json::Value> {
    get_cached_packument_for_key(&metadata_cache_key_with_identity(package, identity))
}

fn get_cached_packument_for_key(key: &str) -> Option<serde_json::Value> {
    let mut cache = get_metadata_cache().lock().ok()?;
    let entry = cache.get(key)?;
    if !cache_entry_is_fresh(entry) {
        return None; // the caller fetches; a stale tree is worse than a round trip
    }
    entry.get_pruned().map(|arc| (*arc).clone())
}

/// Historically stored a pruned clone in the in-memory read cache. It is now a
/// no-op: that clone was the duplication E60 removed, and the same bytes are
/// already retained by the byte-level LRU the fetch path fills. Callers still
/// get identical data through `get_cached_packument`.
pub fn cache_packument(_package: &str, _meta: &serde_json::Value) {}

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

pub fn registry_url() -> String {
    crate::config::effective_registry_url(Path::new("."))
}

pub fn registry_auth_token() -> Option<String> {
    crate::config::registry_auth_token(Path::new("."))
}

/// Returns true when `JHOL_OFFLINE=1`/`true` (or config offline) is set. In
/// offline mode the registry layer must NEVER touch the network — missing
/// entries fail fast with a clear error instead of hanging on a socket.
/// Shrink a packument to only the fields the resolver actually reads.
///
/// Measured (E60): 82 real packuments (56 488 versions) are 85.4 MB of JSON, of
/// which only 27.0 MB (31.6 %) is ever read — `version`, `dependencies`,
/// `optionalDependencies`, `peerDependencies`(+Meta), `os`/`cpu`/`libc`, `dist`,
/// `bin`, `scripts`, `devDependencies`, `engines`, `deprecated`, plus the
/// top-level `name`, `versions` and `dist-tags`. Every other key (`readme`,
/// `maintainers`, `time`, per-version `_npmUser`/`_nodeVersion`, …) greps to zero
/// readers. The retained tree is the resolution memory peak, so dropping them is
/// a ~3x cut to the in-memory working set.
///
/// Guarded: a *manifest* (single-version JSON, no `versions` object) is left
/// untouched, so this is safe to apply to every registry JSON parse.
/// The on-disk packument cache keeps the untouched bytes.
/// Fields a resolution needs that pruning would otherwise drop.
///
/// `time` carries per-version publish dates — the whole input to `--as-of` — and
/// `license` per version is what policy masks need. Pruning to the install-only
/// key set is right for a normal install and wrong for those two features, and
/// the first version of `--as-of` failed exactly here: the resolver received a
/// document it had itself pruned, and reported "no publish dates" about metadata
/// that had arrived complete.
pub fn prune_packument_in_place_keeping(v: &mut serde_json::Value, keep_top: &[&str]) {
    // A license policy reads `versions[*].license`; without this the version
    // metadata is pruned out from under it and every package looks unlicensed.
    let keep_version: &[&str] = if keep_top.contains(&"license") {
        &["license", "licenses"]
    } else {
        &[]
    };
    prune_packument_inner(v, keep_top, keep_version)
}

pub fn prune_packument_in_place(v: &mut serde_json::Value) {
    prune_packument_inner(v, &[], &[])
}

fn prune_packument_inner(v: &mut serde_json::Value, keep_top: &[&str], keep_version: &[&str]) {
    let top_keys: Vec<&str> = {
        let mut v = vec!["name", "versions", "dist-tags"];
        v.extend_from_slice(keep_top);
        v
    };
    let TOP_KEYS: &[&str] = &top_keys;
    const VERSION_KEYS: &[&str] = &[
        "name",
        "version",
        "dependencies",
        "optionalDependencies",
        "peerDependencies",
        "peerDependenciesMeta",
        "os",
        "cpu",
        "libc",
        "dist",
        "bin",
        "scripts",
        "devDependencies",
        "engines",
        "deprecated",
    ];
    let Some(obj) = v.as_object_mut() else { return };
    // Only packuments (they carry a `versions` object) are pruned.
    if !obj.get("versions").map(|x| x.is_object()).unwrap_or(false) {
        return;
    }
    obj.retain(|k, _| TOP_KEYS.contains(&k.as_str()));
    if let Some(versions) = obj.get_mut("versions").and_then(|x| x.as_object_mut()) {
        for (_ver, meta) in versions.iter_mut() {
            if let Some(m) = meta.as_object_mut() {
                m.retain(|k, _| {
                    VERSION_KEYS.contains(&k.as_str()) || keep_version.contains(&k.as_str())
                });
            }
        }
    }
}

/// Prune packument versions for an exact pin or single constraint.
/// Exploits Holographic Wilsonian bounds: an exact pin needs only that specific version object in memory.
/// The version string an exact pin names, or `None` for a range. One origin for
/// the rule `prune_packument_for_constraint` and the typed-record path share, so
/// the two representations cannot disagree about what counts as an exact pin.
pub(crate) fn exact_pin_clean(constraint: &str) -> Option<&str> {
    let clean = constraint.trim_start_matches('=').trim();
    let is_exact = !clean.is_empty()
        && !clean.starts_with('^')
        && !clean.starts_with('~')
        && !clean.starts_with('>')
        && !clean.starts_with('<')
        && !clean.contains('*')
        && !clean.contains('x')
        && !clean.contains(' ');
    if is_exact {
        Some(clean)
    } else {
        None
    }
}

/// Field-prune a packument. **Version keys are kept.**
///
/// This used to keep only the exact-pinned version for a single constraint, and that came at a
/// correctness price the scorecard found: the chart solver received a packument holding one
/// version, so it reported `no version of <pkg> satisfies every dependent` for ordinary trees
/// (`mime` at 1.6.0 cannot satisfy `~1.2.6` *and* `0.19.0`), `select_versions` returned `None`,
/// and the greedy fetch estimate silently became the resolver of record. The estimate then chose
/// `get-intrinsic@1.3.1` where every other tool chooses the `latest` tag, 1.3.0.
///
/// Dropping *version metadata fields* is the memory win; dropping *versions* is what broke the
/// solver. `clean` is kept in the signature because callers pass it and because a future
/// constraint-aware prune may use it — but it no longer removes published versions.
pub fn prune_packument_for_constraint(v: &mut serde_json::Value, _constraint: &str) {
    prune_packument_in_place(v);
}

/// Parse registry JSON bytes and prune packuments (see `prune_packument_in_place`).
fn parse_json_pruned(body: &[u8]) -> Result<serde_json::Value, serde_json::Error> {
    let mut v: serde_json::Value = serde_json::from_slice(body)?;
    prune_packument_in_place(&mut v);
    Ok(v)
}

fn synthesize_packument_from_local_archive(package: &str) -> Option<serde_json::Value> {
    let index = binary_package_index()?;
    let deps_index = binary_package_deps_index();
    let mut hit: Option<(String, &String)> = None;
    for (spec, hash) in index.iter() {
        // "name@version" -> name; scoped "@scope/pkg@ver" -> "@scope/pkg"
        let name = match spec.rsplit_once('@') {
            Some((n, v)) if !n.is_empty() && !v.is_empty() => n,
            _ => continue,
        };
        if name == package {
            match &hit {
                Some((best, _)) => {
                    let cand = spec.rsplit_once('@')?.1;
                    let cur = best.rsplit_once('@')?.1;
                    // Semver order, not lexical: "6.9.0" must not beat "6.16.0".
                    let newer = match (semver::Version::parse(cand), semver::Version::parse(cur)) {
                        (Ok(a), Ok(b)) => a > b,
                        _ => cand > cur,
                    };
                    if newer {
                        hit = Some((spec.clone(), hash));
                    }
                }
                None => hit = Some((spec.clone(), hash)),
            }
        }
    }
    let (spec, _hash) = hit?;
    let version = spec.rsplit_once('@')?.1.to_string();
    let deps = deps_index
        .and_then(|m| m.get(&spec).cloned())
        .unwrap_or_default();
    let mut pkg = serde_json::Map::new();
    pkg.insert(
        "name".into(),
        serde_json::Value::String(package.to_string()),
    );
    pkg.insert("version".into(), serde_json::Value::String(version.clone()));
    pkg.insert(
        "dependencies".into(),
        serde_json::to_value(&deps).unwrap_or(serde_json::json!({})),
    );
    pkg.insert("engines".into(), serde_json::json!({"node": ">=10"}));
    let mut dist = serde_json::Map::new();
    dist.insert(
        "tarball".into(),
        serde_json::Value::String(crate::lockfile::tarball_url_from_registry(
            package, &version,
        )),
    );
    pkg.insert("dist".into(), serde_json::Value::Object(dist));
    let mut versions = serde_json::Map::new();
    versions.insert(version.clone(), serde_json::Value::Object(pkg));
    let mut doc = serde_json::Map::new();
    doc.insert(
        "name".into(),
        serde_json::Value::String(package.to_string()),
    );
    doc.insert("versions".into(), serde_json::Value::Object(versions));
    doc.insert(
        "_vortex_archive_synth".into(),
        serde_json::Value::Bool(true),
    );
    Some(serde_json::Value::Object(doc))
}

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
fn fetch_packument_with_etag(
    package: &str,
    abbreviated: bool,
    identity: &RegistryIdentity,
) -> Result<Vec<u8>, String> {
    if offline_mode() {
        // The prepacked archives carry each package's real `package.json`, so an
        // offline miss on a bundled package is servable locally. Measured: without
        // this, an offline install of 6 packages that ARE in
        // `binary-packages/index.json` took 3.8-4.8 s and produced 2 of 6
        // directories, because every transitive dep was a hard network miss and
        // the whole transitive closure aborted ("transitive expansion failed").
        if let Some(synth) = synthesize_packument_from_local_archive(package) {
            let bytes = serde_json::to_vec(&synth).map_err(|e| e.to_string())?;
            return Ok(bytes);
        }
        return Err(format!(
            "offline mode: packument for {} not in cache (no network allowed)",
            package
        ));
    }

    // ── TTL-based cache fast path ─────────────────────────────────────────────
    // If the packument cache file was written within the last N seconds, serve it
    // directly without any network round-trip. This eliminates the ~50-80ms
    // ETag validation RTT per package on warm-cache installs, matching the
    // performance of Bun's packument cache strategy.
    //
    // TTL defaults to 300 seconds (5 minutes). Set JHOL_PACKUMENT_CACHE_TTL_SECS=0
    // to disable (always validate), or higher values for CI/offline-friendly
    // environments. JHOL_NO_PACKUMENT_TTL=1 is a kill-switch.
    let ttl_secs: u64 = {
        let no_ttl = std::env::var("JHOL_NO_PACKUMENT_TTL")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if no_ttl {
            0
        } else {
            std::env::var("JHOL_PACKUMENT_CACHE_TTL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(300)
        }
    };

    if ttl_secs > 0 {
        let body_path = crate::origin::packument_path_with_identity(package, abbreviated, identity);
        // Fair cold benches use an empty JHOL_CACHE_DIR. Populate from the
        // login-home packument store *before* the freshness check so a harvest
        // hit never pays an ETag RTT (previously harvest ran only after the
        // miss fell through to the network path).
        if !body_path.exists() {
            // mtime-preserving: a harvested copy must not look fresher than its origin.
            let _ = crate::origin::harvest_packument_into_with_identity(
                package,
                abbreviated,
                &body_path,
                identity,
            );
        }
        if let Ok(meta) = std::fs::metadata(&body_path) {
            if let Ok(modified) = meta.modified() {
                if let Ok(age) = std::time::SystemTime::now().duration_since(modified) {
                    if age.as_secs() < ttl_secs {
                        // Cache is fresh — read and return without any network I/O.
                        let (cached_body_raw, _) =
                            crate::origin::read_packument_cache_with_identity(
                                package,
                                abbreviated,
                                identity,
                            );
                        if let Some(body) = cached_body_raw.filter(|b| !b.is_empty()) {
                            if parse_json_pruned(&body).is_ok() {
                                return Ok(body);
                            }
                        }
                        // Cache file corrupt/unreadable — fall through to network.
                    }
                }
            }
        }
    }
    // ─────────────────────────────────────────────────────────────────────────

    let path = if package.starts_with('@') {
        package.replace('/', "%2F")
    } else {
        package.to_string()
    };
    let base = identity.registry.as_str();
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let auth_token = identity.token.as_deref();
    let (cached_body_raw, cached_etag) =
        crate::origin::read_packument_cache_with_identity(package, abbreviated, identity);
    let cached_body = cached_body_raw.filter(|b| !b.is_empty());

    // Build owned strings for header values — &str refs into these live long enough.
    let auth_hdr: Option<String> = auth_token
        .filter(|t| !t.is_empty())
        .map(|t| format!("Bearer {}", t));

    // Build the headers vec for a request (with or without If-None-Match).
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
                if parse_json_pruned(&body).is_ok() {
                    return Ok(body);
                }
            }
            // Cache missing/corrupt: unconditional refetch without If-None-Match.
            match do_fetch(false) {
                Ok((_, body, etag)) if !body.is_empty() => {
                    let _ = crate::origin::write_packument_cache_with_identity(
                        package,
                        abbreviated,
                        &body,
                        etag.as_deref(),
                        identity,
                    );
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
                    if parse_json_pruned(&cached).is_ok() {
                        return Ok(cached);
                    }
                }
                return Err(format!("Empty packument body for {}", package));
            }
            // Fast O(1) structural verification before caching: must start with '{' and end with '}'.
            // Full AST parsing occurs once in fetch_metadata(), avoiding double parsing.
            let is_valid_shape = {
                let first = body.iter().find(|&&b| !b.is_ascii_whitespace());
                let last = body.iter().rfind(|&&b| !b.is_ascii_whitespace());
                first == Some(&b'{') && last == Some(&b'}')
            };
            if !is_valid_shape {
                if let Some(cached) = cached_body {
                    if parse_json_pruned(&cached).is_ok() {
                        return Ok(cached);
                    }
                }
                return Err(format!("Invalid packument JSON for {}", package));
            }
            let _ = crate::origin::write_packument_cache_with_identity(
                package,
                abbreviated,
                &body,
                etag.as_deref(),
                identity,
            );
            Ok(body)
        }
        Err(e) => {
            if let Some(body) = cached_body {
                if parse_json_pruned(&body).is_ok() {
                    return Ok(body);
                }
            }
            Err(e)
        }
    }
}

/// Fetch a specific version manifest (/<pkg>/latest or /<pkg>/<version>).
/// Routes through the global shared HTTP client — reuses existing TCP connection.
pub fn fetch_manifest(package: &str, selector: &str) -> Result<serde_json::Value, String> {
    let identity = RegistryIdentity::for_package(package);
    fetch_manifest_with_identity(package, selector, &identity)
}

fn fetch_manifest_with_identity(
    package: &str,
    selector: &str,
    identity: &RegistryIdentity,
) -> Result<serde_json::Value, String> {
    let path = if package.starts_with('@') {
        package.replace('/', "%2F")
    } else {
        package.to_string()
    };
    let base = identity.registry.as_str();
    let url = format!(
        "{}/{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/'),
        selector.trim_start_matches('/')
    );
    // A version document is a projection of the packument: answer it from cache
    // when we have one, instead of paying a round trip per package on every
    // incremental install. `latest` is a dist-tag, not a version.
    if let Some(doc) = crate::origin::cached_packument_value(package, identity) {
        let version = if selector == "latest" {
            doc.get("dist-tags")
                .and_then(|t| t.get("latest"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        } else {
            Some(selector.to_string())
        };
        if let Some(version) = version {
            if let Some(entry) = doc.get("versions").and_then(|v| v.get(&version)) {
                return Ok(entry.clone());
            }
        }
    }
    // Route through http_client (shared Agent) instead of raw ureq::get().
    let body = crate::http_client::get_bytes_with_bearer(&url, identity.token.as_deref())?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

/// Resolve package from pre-resolved index (O(1) lookup!)
/// Returns None if package not in index (will fetch from network)
fn resolve_from_index(
    package: &str,
    _version_req: &str,
) -> Option<crate::package_index::PreResolvedPackage> {
    let identity = RegistryIdentity::for_package(package);
    let origin_key = metadata_cache_key_with_identity(package, &identity);
    let allow_embedded = identity.registry.trim_end_matches('/') == "https://registry.npmjs.org"
        && identity.token.is_none();
    let index = PACKAGE_INDEX.lock().ok()?;
    index
        .lookup_for_origin(package, _version_req, &origin_key, allow_embedded)
        .cloned()
}

/// Store-first metadata: the SAME `binary-packages/index.json` that serves
/// tarballs names every version this machine holds, and `deps.json`
/// (enriched from the archives' own package.json) carries that version's
/// dependencies. Online or offline, a package the machine already holds never
/// costs a network request for its metadata. Integrity stays empty here on
/// purpose — the post-install SRI backfill hashes the store tarball itself and
/// writes the real value into the lockfile.
fn resolve_from_binary_store(
    package: &str,
    constraint: &str,
) -> Option<crate::package_index::PreResolvedPackage> {
    let index = binary_package_index()?;
    let deps_index = binary_package_deps_index()?;
    let c = constraint.trim();
    let unversioned = c.is_empty() || c == "latest" || c == "*";

    let prefix = format!("{package}@");
    let mut best: Option<Version> = None;
    for key in index.keys() {
        let Some(ver_str) = key.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(v) = Version::parse(ver_str) else {
            continue;
        };
        // jhol's OWN npm-semantics predicate: OR ranges (`^3 || ^4`),
        // space-separated multi-comparators (`>=2.1.2 <3`), prerelease rules
        // (E19). semver::VersionReq rejects OR-ranges, which forced a network
        // fetch for ranges the store can satisfy — that was one of the
        // Appendix-13 audit misses (js-tokens, safer-buffer).
        let satisfied = unversioned || version_satisfies(c, ver_str);
        if satisfied && best.as_ref().map_or(true, |b| v > *b) {
            best = Some(v);
        }
    }
    let ver = best?;
    let wkey = format!("{package}@{ver}");
    // Unknown dependencies = no synth (falls through to network, always safe).
    let deps = deps_index.get(&wkey)?.clone();
    let base = package.rsplit('/').next().unwrap_or(package);
    let tarball_url = format!(
        "{}/{}-/{}-{}.tgz",
        crate::config::effective_registry_url_for_package(Path::new("."), package)
            .trim_end_matches('/'),
        package,
        base,
        ver
    );
    Some(crate::package_index::PreResolvedPackage {
        name: package.to_string(),
        version: ver.to_string(),
        versions: vec![ver.to_string()],
        tarball_url,
        integrity: String::new(),
        dependencies: deps,
        resolved_at: 0,
    })
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
    // A snapshot holds one version per package, so it cannot answer a resolution that needs
    // several (`mime` 1.6.0 cannot serve `~1.2.6` and `0.19.0`). Offline it is the only source
    // and stays available; online it must not exist, or it silently decides versions.
    if !offline_mode() && !prefer_bundled() {
        return None;
    }
    // STORE FIRST: binary-packages is per-machine curated (fresher, lockfile-
    // derived) — the embedded build-time seed is only a fallback. Prefer order
    // matters for cross-machine determinism: same repo, same picks.
    let entry = resolve_from_binary_store(package, constraint)
        .or_else(|| resolve_from_index(package, constraint))
        .or_else(|| {
            if std::env::var("JHOL_DEBUG_BUNDLED").is_ok() {
                eprintln!("[bundled] index MISS: {package}@{constraint}");
            }
            None
        })?;
    // Peer + platform truth rides in meta.json for the same name@version —
    // dropping these fields would silently disable peer validation and admit
    // foreign-platform optionals: metadata must match what the network
    // packument would have said.
    let synth_extras: serde_json::Map<String, serde_json::Value> = binary_package_meta_index()
        .and_then(|m| m.get(&format!("{}@{}", entry.name, entry.version)))
        .map(|h| h.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let mut versions = serde_json::Map::new();
    let mut saw_satisfying = false;
    // Only expose the ONE version whose dependencies we actually know (the
    // index's recorded latest). Earlier this inserted every bundled version,
    // but deps are only stored for the latest — so the resolver could select,
    // say, `1.3.1` from the list and get an empty dependency map, silently
    // dropping that version's entire subtree. A synth is a drop-in replacement
    // only when every version it advertises has complete metadata, so the
    // other versions are omitted; a constraint that needs one of them makes
    // `saw_satisfying` false and the caller falls through to the full packument.
    let mut version_obj = serde_json::Map::new();
    version_obj.insert(
        "name".to_string(),
        serde_json::Value::String(entry.name.clone()),
    );
    version_obj.insert(
        "version".to_string(),
        serde_json::Value::String(entry.version.clone()),
    );
    version_obj.insert(
        "dependencies".to_string(),
        serde_json::to_value(&entry.dependencies).unwrap_or(serde_json::json!({})),
    );
    // Merge peer/platform truth (never clobber the synthesized core identity
    // fields: name/version/dependencies/dist are owned above/below).
    for (k, v) in &synth_extras {
        if !matches!(k.as_str(), "name" | "version" | "dependencies" | "dist") {
            version_obj.insert(k.clone(), v.clone());
        }
    }
    version_obj.insert("engines".to_string(), serde_json::json!({"node": ">=10"}));
    let mut dist = serde_json::Map::new();
    dist.insert(
        "tarball".to_string(),
        serde_json::Value::String(entry.tarball_url.clone()),
    );
    // Empty integrity (store-sourced metadata) is OMITTED, not written as "":
    // the post-install SRI backfill fills absent keys from the store tarball,
    // and `jhol ci` treats an empty string as "missing" either way.
    if !entry.integrity.is_empty() {
        dist.insert(
            "integrity".to_string(),
            serde_json::Value::String(entry.integrity.clone()),
        );
    }
    version_obj.insert("dist".to_string(), serde_json::Value::Object(dist));

    if crate::registry::version_satisfies(constraint, &entry.version) {
        saw_satisfying = true;
    }

    versions.insert(
        entry.version.clone(),
        serde_json::Value::Object(version_obj),
    );

    // The synthesized packument is only a drop-in replacement when at least one
    // bundled version actually satisfies the requested constraint. If the bundle
    // cannot serve the request (e.g. the constraint requires a version that is
    // not in the bundled list), return None so the caller falls through to a
    // full network packument — a partial synth would cause a false "unresolved".
    if !saw_satisfying {
        if std::env::var("JHOL_DEBUG_BUNDLED").is_ok() {
            eprintln!("[bundled] range unsatisfied by bundled version: {package}@{constraint}");
        }
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
    let identity = RegistryIdentity::for_package(package);
    cache_in_index_with_identity(package, metadata, &identity);
}

fn cache_in_index_with_identity(
    package: &str,
    metadata: &serde_json::Value,
    identity: &RegistryIdentity,
) {
    cache_in_index_with_origin_key(
        package,
        metadata,
        &metadata_cache_key_with_identity(package, identity),
    );
}

fn cache_in_index_with_origin_key(package: &str, metadata: &serde_json::Value, origin_key: &str) {
    if let Some(entry) = crate::package_index::build_index_entry(package, metadata) {
        if let Ok(mut index) = PACKAGE_INDEX.lock() {
            index.add_package_for_origin(entry, origin_key);
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
    // FIRST: Try pre-resolved package index (O(1) lookup!) — but only where the
    // build-time snapshot is allowed to answer (offline, exact pin, or the
    // explicit JHOL_PREFER_BUNDLED opt-in). Online ranges and `latest` must see
    // the live registry, or direct deps install an outdated version.
    if bundled_index_allowed(&[version_req.to_string()]) {
        if let Some(resolved) = resolve_from_index(package, version_req) {
            return Ok(Some((
                resolved.version,
                resolved.tarball_url,
                Some(resolved.integrity),
            )));
        }
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

// NOTE: these were lowered (multiplier 2, min 4, max 16) in an attempt to bound
// memory on small machines; that combination deadlocked plain installs, so the
// original sizing is restored. Any reduction must be validated with a real
// `jhol install` (a single dependency is enough) before it ships.
const DEFAULT_PACKUMENT_CONCURRENCY: usize = 32; // Increased from 8 to 32 for better parallelism
const MAX_PACKUMENT_CONCURRENCY: usize = 64;
const MIN_PACKUMENT_CONCURRENCY: usize = 1;
const DEFAULT_PARALLELISM_MULTIPLIER: usize = 4; // Increased from 2 to 4 for more aggressive parallelism
const MIN_PARALLEL_CONCURRENCY: usize = 16; // Increased from 8 to 16
const MAX_PARALLEL_CONCURRENCY: usize = 64; // Increased from 32 to 64

pub(crate) fn packument_concurrency() -> usize {
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

/// Cached metadata with lazy evaluation and parsed pruned cache
#[derive(Debug)]
pub struct CachedMetadata {
    lazy: LazyMetadata,
    pruned: Option<Arc<Value>>,
    last_access: std::time::Instant,
    /// When this document was fetched (or last confirmed current by the registry). Distinct
    /// from `last_access`: reading a stale document does not make it fresh.
    fetched_at: std::time::Instant,
}

impl CachedMetadata {
    pub fn new(raw_bytes: Vec<u8>) -> Self {
        Self {
            lazy: LazyMetadata::new(raw_bytes),
            pruned: None,
            last_access: std::time::Instant::now(),
            fetched_at: std::time::Instant::now(),
        }
    }

    pub fn new_with_pruned(raw_bytes: Vec<u8>, pruned: Arc<Value>) -> Self {
        Self {
            lazy: LazyMetadata::new(raw_bytes),
            pruned: Some(pruned),
            last_access: std::time::Instant::now(),
            fetched_at: std::time::Instant::now(),
        }
    }

    pub fn access(&mut self) -> Result<&LazyMetadata, String> {
        self.last_access = std::time::Instant::now();
        Ok(&self.lazy)
    }

    pub fn get_pruned(&mut self) -> Option<Arc<Value>> {
        self.last_access = std::time::Instant::now();
        if self.pruned.is_none() && !self.lazy.raw_bytes.is_empty() {
            if let Ok(v) = parse_json_pruned(&self.lazy.raw_bytes) {
                self.pruned = Some(Arc::new(v));
            }
        }
        self.pruned.clone()
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

/// How long a cached packument may decide a version.
///
/// A resolver's inputs must be **current or explicitly pinned**. Serving a document fetched an
/// hour ago silently resolves yesterday's registry: two tools run seconds apart produce different
/// trees, which is the difference the scorecard found on `electron-to-chromium` (1.5.482 against
/// 1.5.483 — a package that publishes several times an hour). `--as-of` pins history and is exempt;
/// offline has no choice and is exempt too.
///
/// Sixty seconds is deliberate: long enough that one install reuses one document across hundreds of
/// edges, short enough that a fresh install cannot resolve a release the registry has already lost.
pub(crate) fn metadata_freshness() -> std::time::Duration {
    std::env::var("JHOL_METADATA_FRESHNESS_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(60))
}

/// Is this document still allowed to decide a version?
fn cache_entry_is_fresh(entry: &CachedMetadata) -> bool {
    if offline_mode() || std::env::var_os("JHOL_AS_OF").is_some() {
        return true; // pinned history, or no registry to ask
    }
    entry.fetched_at.elapsed() < metadata_freshness()
}

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
/// Build a single-version packument synth from a registry manifest.
///
/// Preserves every field the resolver needs: `dependencies`,
/// `optionalDependencies`, `peerDependencies(+Meta)`, **`os`/`cpu`/`libc`**
/// (without these, platform-specific optional deps would be treated as
/// compatible and downloaded on the wrong platform), and `dist`.
fn build_manifest_synth(package: &str, manifest: &serde_json::Value) -> Option<serde_json::Value> {
    let version = manifest.get("version").and_then(|v| v.as_str())?;
    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(package)
        .to_string();
    let as_str_map = |key: &str| -> serde_json::Value {
        let m: serde_json::Map<String, serde_json::Value> = manifest
            .get(key)
            .and_then(|d| d.as_object())
            .map(|o| {
                o.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            serde_json::Value::String(v.as_str().unwrap_or("").to_string()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        serde_json::Value::Object(m)
    };

    let mut version_obj = serde_json::Map::new();
    version_obj.insert("name".to_string(), serde_json::Value::String(name.clone()));
    version_obj.insert(
        "version".to_string(),
        serde_json::Value::String(version.to_string()),
    );
    version_obj.insert("dependencies".to_string(), as_str_map("dependencies"));
    version_obj.insert(
        "optionalDependencies".to_string(),
        as_str_map("optionalDependencies"),
    );
    version_obj.insert(
        "peerDependencies".to_string(),
        as_str_map("peerDependencies"),
    );
    for key in ["peerDependenciesMeta", "os", "cpu", "libc", "engines"] {
        if let Some(v) = manifest.get(key) {
            version_obj.insert(key.to_string(), v.clone());
        }
    }
    let mut dist = serde_json::Map::new();
    if let Some(d) = manifest.get("dist").and_then(|d| d.as_object()) {
        for k in [
            "tarball",
            "integrity",
            "shasum",
            "fileCount",
            "unpackedSize",
        ] {
            if let Some(v) = d.get(k) {
                dist.insert(k.to_string(), v.clone());
            }
        }
    }
    version_obj.insert("dist".to_string(), serde_json::Value::Object(dist));

    let mut versions = serde_json::Map::new();
    versions.insert(version.to_string(), serde_json::Value::Object(version_obj));
    let mut dist_tags = serde_json::Map::new();
    dist_tags.insert(
        "latest".to_string(),
        serde_json::Value::String(version.to_string()),
    );
    Some(serde_json::json!({
        "name": name,
        "versions": versions,
        "dist-tags": dist_tags,
        "_vortex_manifest_synth": true,
    }))
}

/// Constraint-aware metadata fetch for the resolver: prefer the cheapest source
/// that can fully answer the request, and only fall back to the (potentially
/// huge) packument when it cannot.
///
/// 1. bundled-index synth (zero network) when it satisfies every constraint;
/// 2. the package's `latest` **manifest** (a few KB) when its single version
///    satisfies every constraint — this is what avoids downloading multi-MB
///    abbreviated packuments (e.g. `next`'s is ~25 MB) just to pick the latest;
/// 3. otherwise the full/abbreviated packument, which is the only source with a
///    complete version list for ranged resolution.
///
/// Correctness relies on the caller escalating to the full packument if a later
/// constraint invalidates the synth.
/// True when the packument has a version that satisfies **all** given
/// constraints. This is the correct predicate for a single-version synth (the
/// resolver must satisfy every dependent's range, not just one).
pub fn synth_satisfies_all(meta: &serde_json::Value, constraints: &[String]) -> bool {
    if constraints.is_empty() {
        return false;
    }
    let Some(versions) = meta.get("versions").and_then(|v| v.as_object()) else {
        return false;
    };
    versions
        .keys()
        .any(|v| constraints.iter().all(|c| version_satisfies(c, v)))
}

/// May the **build-time bundled index** answer for these constraints?
///
/// Only offline, or for an exact pin. The bundle is a snapshot taken when jhol was
/// built, so letting it satisfy a *range* silently installs an outdated version:
/// with the live registry publishing `axios@1.20.0`, `axios: ^1.13.0` resolved to
/// the bundled `1.13.5`. npm/bun install the highest satisfying version, so a
/// snapshot must never win over live metadata for a range. Offline, the bundle is
/// a legitimate (and the only) source; for an exact pin it either has that version
/// or it does not, so freshness cannot be affected.
///
/// Measured regression this guards (2026-10-24, 20-dep fixture vs Bun): with the
/// gate open for ranges online, `dayjs ^1.11.13` resolved to the store's 1.11.19
/// (registry: 1.11.23) and `qs ^6.14.0` to 6.16.0 (registry: 6.16.1). The per-call
/// guards (`saw_satisfying`, `synth_satisfies_all`) only prove the snapshot version
/// *satisfies* the range, never that it is the *highest* satisfying one.
///
/// `JHOL_PREFER_BUNDLED=1` is the explicit opt-in to trade freshness for zero
/// metadata traffic (air-gapped-ish CI, reproducible-by-store builds).
/// May the bundled snapshot answer this request?
///
/// Only when there is no registry to ask (offline), or when the user explicitly prefers it.
///
/// It used to also answer whenever every constraint was an exact pin, and that rule made a
/// *snapshot* the resolver of record: the greedy estimate pinned `mime@1.6.0` from the bundled
/// index, the pin was then "allowed" to be served from the same index, and the chart solver —
/// handed a one-version packument — reported a conflict, so the estimate stood. The result was a
/// tree no other tool produces (`get-intrinsic@1.3.1` where npm, bun, pnpm and yarn take the
/// `latest`-tagged 1.3.0). A snapshot cannot be an authority during resolution; it can only be a
/// fallback when the registry is unreachable.
pub(crate) fn bundled_index_allowed(_constraints: &[String]) -> bool {
    offline_mode() || prefer_bundled()
}

fn prefer_bundled() -> bool {
    static PREFER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PREFER.get_or_init(|| {
        matches!(
            std::env::var("JHOL_PREFER_BUNDLED").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        )
    })
}

pub fn fetch_metadata_manifest_first(
    package: &str,
    constraints: &[String],
) -> Result<serde_json::Value, String> {
    fetch_metadata_manifest_first_any::<serde_json::Value>(package, constraints)
}

/// E60 — manifest-first fetch parsed into a typed record. The synth layers below
/// build `Value`s, but a synth is a single-version object of a few hundred bytes;
/// converting one is not the E67 trap (rebuilding a *full* packument would be).
pub fn fetch_metadata_manifest_first_record(
    package: &str,
    constraints: &[String],
) -> Result<PackumentRecord, String> {
    fetch_metadata_manifest_first_any::<PackumentRecord>(package, constraints)
}

pub fn fetch_metadata_manifest_first_any<T: MetaRepr>(
    package: &str,
    constraints: &[String],
) -> Result<T, String> {
    let identity = RegistryIdentity::for_package(package);
    if bundled_index_allowed(constraints) {
        if let Some(c) = constraints.first() {
            if let Some(synth) = synthesize_packument_from_bundled_index(package, c) {
                if synth_satisfies_all(&synth, constraints) {
                    return T::from_value(synth);
                }
            }
        }
    }
    if constraints.is_empty() {
        return fetch_metadata_any_with_identity::<T>(package, &identity);
    }
    // Zero-network in-memory read cache check: sub-microsecond hit for prefetched packuments
    let cache_key = metadata_cache_key_with_identity(package, &identity);
    if let Some(meta) = T::cached(package, &cache_key) {
        if meta.satisfies(constraints) {
            return Ok(meta);
        }
    }
    // Zero-network disk cache check: if a fresh packument is already on disk and satisfies constraints,
    // return it directly in 0.1 ms rather than paying network RTT for a single manifest.
    if crate::origin::packument_fresh_with_identity(package, true, &identity)
        || crate::origin::packument_fresh_with_identity(package, false, &identity)
    {
        if let Ok(meta) = fetch_metadata_any_with_identity::<T>(package, &identity) {
            if meta.satisfies(constraints) {
                return Ok(meta);
            }
        }
    }
    // Exact-version pin: fetch that version's manifest directly. It is a few KB
    // and, unlike `latest`, is guaranteed to be the requested version — this is
    // what avoids pulling a full packument for every platform-specific optional
    // dependency (e.g. the seven `@next/swc-*` variants, ~2.6 MB each).
    for c in constraints {
        if crate::registry::is_exact_version_pin(c) {
            let spec = c.trim().trim_start_matches('=').trim_start_matches('v');
            if let Ok(manifest) = fetch_manifest_with_identity(package, spec, &identity) {
                if let Some(synth) = build_manifest_synth(package, &manifest) {
                    if synth_satisfies_all(&synth, constraints) {
                        crate::lattice_cache::remember_for_key(package, "latest", spec, &cache_key);
                        return T::from_value(synth);
                    }
                }
            }
            break;
        }
    }
    // If latest version is already known from lattice_cache, check if it satisfies all constraints.
    // If it does not, skip the speculative /latest network roundtrip and fetch the packument directly.
    if let Some(known_latest) = crate::lattice_cache::lookup_lattice_for_key(&cache_key, "latest") {
        if !constraints
            .iter()
            .all(|c| version_satisfies(c, &known_latest))
        {
            return fetch_metadata_any_with_identity::<T>(package, &identity);
        }
    }

    // Otherwise, the `latest` manifest is enough only when the request explicitly
    // allows the latest version (*, latest, or empty). For ranged constraints (e.g. ^18.2.0),
    // speculating on /latest frequently fails when latest has moved to a new major version,
    // which costs a doomed 400ms HTTP roundtrip before falling back to the packument.
    let allows_latest_speculation = constraints.iter().all(|c| {
        let t = c.trim();
        t == "*" || t == "latest" || t.is_empty()
    });
    if allows_latest_speculation {
        if let Ok(manifest) = fetch_manifest_with_identity(package, "latest", &identity) {
            if let Some(synth) = build_manifest_synth(package, &manifest) {
                if let Some(v) = synth
                    .get("versions")
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.keys().next())
                {
                    crate::lattice_cache::remember_for_key(package, "latest", v, &cache_key);
                }
                if synth_satisfies_all(&synth, constraints) {
                    return T::from_value(synth);
                }
            }
        }
    }
    fetch_metadata_any_with_identity::<T>(package, &identity)
}

struct InFlightPackument {
    /// Type-erased so one single-flight map serves both representations (`Any`
    /// downcast on the follower side; a mixed-representation race falls back to
    /// a direct fetch, which returns the same bytes).
    res: parking_lot::Mutex<Option<Result<Arc<dyn Any + Send + Sync>, String>>>,
    cond: parking_lot::Condvar,
}

static IN_FLIGHT_PACKUMENTS: OnceLock<parking_lot::Mutex<HashMap<String, Arc<InFlightPackument>>>> =
    OnceLock::new();

fn in_flight_packuments() -> &'static parking_lot::Mutex<HashMap<String, Arc<InFlightPackument>>> {
    IN_FLIGHT_PACKUMENTS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// Representation seam for the metadata fetch path (E60).
///
/// `serde_json::Value` is the historical representation and stays the default
/// for every consumer outside the JAGR-∞ resolution path. [`PackumentRecord`] is
/// the typed record that path uses, so no `Value` tree is ever built for it
/// (measured 11.5x raw as `Value`; E60's ceiling for the record is 3.2x).
///
/// The control flow below is shared, so the two representations cannot drift:
/// only the parse call and the two side-cache hooks differ. `parse_raw` versus
/// `parse_cached` mirrors the historical split exactly (the network branches
/// returned an unpruned value, the cache branches a pruned one).
pub trait MetaRepr: Clone + Send + 'static {
    /// Parse a network response body (the old raw `serde_json::from_slice`).
    fn parse_raw(bytes: &[u8]) -> Result<Self, String>;
    /// Parse a cached body (the old `parse_json_pruned`).
    fn parse_cached(bytes: &[u8]) -> Result<Self, String>;
    /// Old `versions` present-and-non-empty test (`.unwrap_or(false)`).
    fn versions_non_empty(&self) -> bool;
    /// Old `has_versions` test (`.unwrap_or(true)` — an absent `versions` key counts).
    fn has_versions(&self) -> bool;
    fn is_synth(&self) -> bool;
    fn latest_tag(&self) -> Option<String>;
    /// Per-representation read cache. The record impl is deliberately a no-op:
    /// retaining a second copy would give back exactly the duplication E60 removes.
    fn read_cache(&self, package: &str);
    /// Pre-resolved package index (a `Value`-only consumer).
    fn index(&self, package: &str, origin_key: &str);
    fn prune_constraint(&mut self, constraint: &str);
    fn satisfies(&self, constraints: &[String]) -> bool;
    /// Convert a small synthesized packument. Never used for a full packument —
    /// that is the E67 trap.
    fn from_value(v: serde_json::Value) -> Result<Self, String>;
    fn cached(_package: &str, cache_key: &str) -> Option<Self>;
    fn into_any(self) -> Arc<dyn Any + Send + Sync>;
    fn from_any(a: Arc<dyn Any + Send + Sync>) -> Option<Self>;
}

impl MetaRepr for serde_json::Value {
    fn parse_raw(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
    fn parse_cached(bytes: &[u8]) -> Result<Self, String> {
        parse_json_pruned(bytes).map_err(|e| e.to_string())
    }
    fn versions_non_empty(&self) -> bool {
        self.get("versions")
            .and_then(|v| v.as_object())
            .map(|o| !o.is_empty())
            .unwrap_or(false)
    }
    fn has_versions(&self) -> bool {
        self.get("versions")
            .and_then(|v| v.as_object())
            .map(|o| !o.is_empty())
            .unwrap_or(true)
    }
    fn is_synth(&self) -> bool {
        self.get("_vortex_manifest_synth").is_some() || self.get("_bundled_index_synth").is_some()
    }
    fn latest_tag(&self) -> Option<String> {
        self.get("dist-tags")
            .and_then(|d| d.get("latest"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }
    fn read_cache(&self, package: &str) {
        cache_packument(package, self);
    }
    fn index(&self, package: &str, origin_key: &str) {
        cache_in_index_with_origin_key(package, self, origin_key);
    }
    fn prune_constraint(&mut self, constraint: &str) {
        prune_packument_for_constraint(self, constraint);
    }
    fn satisfies(&self, constraints: &[String]) -> bool {
        synth_satisfies_all(self, constraints)
    }
    fn from_value(v: serde_json::Value) -> Result<Self, String> {
        Ok(v)
    }
    fn cached(_package: &str, cache_key: &str) -> Option<Self> {
        get_cached_packument_for_key(cache_key)
    }
    fn into_any(self) -> Arc<dyn Any + Send + Sync> {
        Arc::new(self)
    }
    fn from_any(a: Arc<dyn Any + Send + Sync>) -> Option<Self> {
        a.downcast::<serde_json::Value>().ok().map(|a| (*a).clone())
    }
}

impl MetaRepr for PackumentRecord {
    fn parse_raw(bytes: &[u8]) -> Result<Self, String> {
        PackumentRecord::from_slice(bytes)
    }
    fn parse_cached(bytes: &[u8]) -> Result<Self, String> {
        PackumentRecord::from_slice(bytes)
    }
    fn versions_non_empty(&self) -> bool {
        !self.versions().is_empty()
    }
    fn has_versions(&self) -> bool {
        PackumentRecord::has_versions(self)
    }
    fn is_synth(&self) -> bool {
        PackumentRecord::is_synth(self)
    }
    fn latest_tag(&self) -> Option<String> {
        PackumentRecord::latest_tag(self).map(str::to_string)
    }
    fn read_cache(&self, _package: &str) {
        // Intentionally empty (see the trait doc). The byte-level LRU
        // (`get_metadata_cache`) is still filled by the shared path, so a later
        // `Value` consumer finds the bytes without a re-download.
    }
    fn index(&self, _package: &str, _origin_key: &str) {}
    fn prune_constraint(&mut self, constraint: &str) {
        let Some(clean) = exact_pin_clean(constraint) else {
            return;
        };
        if self.versions().contains_key(clean) {
            let keep: std::collections::HashSet<String> =
                std::iter::once(clean.to_string()).collect();
            self.retain_versions(&keep);
        }
    }
    fn satisfies(&self, constraints: &[String]) -> bool {
        PackumentRecord::satisfies_all(self, constraints)
    }
    fn from_value(v: serde_json::Value) -> Result<Self, String> {
        PackumentRecord::from_value(v)
    }
    fn cached(_package: &str, _cache_key: &str) -> Option<Self> {
        // The record path keeps no second copy; a miss just falls through to the
        // disk-cache check in `fetch_metadata_manifest_first`.
        None
    }
    fn into_any(self) -> Arc<dyn Any + Send + Sync> {
        Arc::new(self)
    }
    fn from_any(a: Arc<dyn Any + Send + Sync>) -> Option<Self> {
        a.downcast::<PackumentRecord>().ok().map(|a| (*a).clone())
    }
}

fn fetch_metadata_network_inner<T: MetaRepr>(
    package: &str,
    cache_key: &str,
    identity: &RegistryIdentity,
) -> Result<T, String> {
    // Fetch from network
    let body = match fetch_packument_with_etag(package, true, identity) {
        Ok(b) => b,
        Err(_) => fetch_packument_with_etag(package, false, identity)?,
    };

    // Parse metadata (with fallback to full packument if abbreviated parse fails)
    let timing = std::env::var_os("JHOL_HTTP_TIMING").is_some();
    let parse_started = timing.then(std::time::Instant::now);
    let (body, parsed) = match T::parse_raw(&body) {
        Ok(v) => (body, v),
        Err(_) => {
            let full_body = fetch_packument_with_etag(package, false, identity)?;
            let v = T::parse_raw(&full_body)?;
            (full_body, v)
        }
    };
    if let Some(started) = parse_started {
        eprintln!(
            "[PACKUMENT-PARSE] package={} bytes={} elapsed_ms={}",
            package,
            body.len(),
            started.elapsed().as_millis()
        );
    }

    // Record latest in lattice_cache for O(1) decision making
    if let Some(latest) = parsed.latest_tag() {
        crate::lattice_cache::remember_for_key(package, "latest", &latest, cache_key);
    }

    // Cache in pre-resolved index for future O(1) lookups
    parsed.index(package, cache_key);

    // Check if abbreviated packument is sufficient
    let has_versions = parsed.has_versions();

    if has_versions {
        // Cache in packument read cache + pre-warm index
        let mut cache = get_metadata_cache()
            .lock()
            .expect("metadata_cache: mutex lock failed (poisoned)");
        cache.insert(cache_key.to_string(), CachedMetadata::new(body.clone()));
        // Populate the representation's read cache (a no-op for the record,
        // which must not retain a second copy)
        parsed.read_cache(package);
        // Soul Path: propagate to daemon arena for cross-process reuse
        crate::daemon_cache::DaemonCache::record_with_identity(package, true, body, identity);
        return Ok(parsed);
    }

    // Fall back to full packument
    let body = fetch_packument_with_etag(package, false, identity)?;
    let v = T::parse_raw(&body)?;

    let mut cache = get_metadata_cache()
        .lock()
        .expect("metadata_cache: mutex lock failed (poisoned)");
    cache.insert(cache_key.to_string(), CachedMetadata::new(body.clone()));
    v.read_cache(package);
    crate::daemon_cache::DaemonCache::record_with_identity(package, false, body, identity);

    Ok(v)
}

/// Fetch a packument as a `serde_json::Value` (the historical representation,
/// used by every consumer outside the JAGR-∞ resolution path).
/// The document a **resolution** reads.
///
/// The abbreviated packument (`application/vnd.npm.install-v1+json`) is what npm's installer reads
/// for speed, and its `dist-tags` are served from a CDN edge: it can report `latest = 1.5.482` while
/// the full document already says `1.5.483`. Two clients installing seconds apart then legitimately
/// disagree on a version — which the scorecard cannot distinguish from a resolver bug, and which is
/// a real correctness hazard: the tree a user gets depends on which edge answered.
///
/// The rule here is ours: **a resolution's inputs must be authoritative or explicitly pinned.** So
/// resolution reads the full packument by default; the abbreviated form stays available for
/// prefetching (`JHOL_METADATA_ABBREVIATED=1`), where a stale hint only costs a round trip later and
/// never decides a version.
pub fn fetch_metadata(package: &str) -> Result<serde_json::Value, String> {
    let abbreviated = matches!(
        std::env::var("JHOL_METADATA_ABBREVIATED").as_deref(),
        Ok("1") | Ok("true")
    );
    if abbreviated {
        fetch_metadata_any::<serde_json::Value>(package)
    } else {
        fetch_metadata_full(package)
    }
}

/// E60 — the same layering as [`fetch_metadata`], parsed straight into a typed
/// record so the resolution path never builds a `Value` tree.
pub fn fetch_metadata_record(package: &str) -> Result<PackumentRecord, String> {
    fetch_metadata_any::<PackumentRecord>(package)
}

pub fn fetch_metadata_any<T: MetaRepr>(package: &str) -> Result<T, String> {
    let identity = RegistryIdentity::for_package(package);
    fetch_metadata_any_with_identity(package, &identity)
}

fn fetch_metadata_any_with_identity<T: MetaRepr>(
    package: &str,
    identity: &RegistryIdentity,
) -> Result<T, String> {
    let cache_key = metadata_cache_key_with_identity(package, identity);

    // Try cache first
    {
        let mut cache = get_metadata_cache()
            .lock()
            .expect("metadata_cache: mutex lock failed (poisoned)");
        if let Some(cached) = cache.get(&cache_key) {
            if cache_entry_is_fresh(cached) {
                let bytes = &cached
                    .access()
                    .expect("lazy_metadata: failed to access cached data")
                    .raw_bytes;
                if let Ok(parsed) = T::parse_cached(bytes) {
                    if parsed.versions_non_empty() {
                        return Ok(parsed);
                    }
                }
            }
        }
    }

    // Soul Path: try daemon shared memory arena
    if let Some(cached_bytes) =
        crate::daemon_cache::DaemonCache::lookup_with_identity(package, true, identity)
    {
        if let Ok(parsed) = T::parse_cached(&cached_bytes) {
            if parsed.versions_non_empty() {
                // warm in-mem cache
                let mut cache = get_metadata_cache()
                    .lock()
                    .expect("metadata_cache: mutex lock failed (poisoned)");
                cache.insert(cache_key.clone(), CachedMetadata::new(cached_bytes));
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

    // VORTEX Layer 2: persistent packument disk cache.
    // Check the disk cache BEFORE making any network requests to avoid RTT for packages we've seen before.
    // Try abbreviated first (smaller, faster), fall back to full packument.
    for abbreviated in &[true, false] {
        if !crate::origin::packument_fresh_with_identity(package, *abbreviated, identity) {
            continue;
        }

        let (cached_body, _cached_etag) =
            crate::origin::read_packument_cache_with_identity(package, *abbreviated, identity);
        if let Some(body) = cached_body.filter(|b| !b.is_empty()) {
            if let Ok(parsed) = T::parse_cached(&body) {
                if parsed.versions_non_empty() {
                    // Cache in LRU for faster subsequent in-process access
                    let mut cache = get_metadata_cache()
                        .lock()
                        .expect("metadata_cache: mutex lock failed (poisoned)");
                    cache.insert(cache_key.clone(), CachedMetadata::new(body.clone()));
                    parsed.read_cache(package);
                    return Ok(parsed);
                }
            }
        }
    }

    // SingleFlight coordination: deduplicate concurrent network fetches across all threads
    let (in_flight_arc, is_leader) = {
        let mut map = in_flight_packuments().lock();
        if let Some(existing) = map.get(&cache_key).cloned() {
            (existing, false)
        } else {
            let entry = Arc::new(InFlightPackument {
                res: parking_lot::Mutex::new(None),
                cond: parking_lot::Condvar::new(),
            });
            map.insert(cache_key.clone(), Arc::clone(&entry));
            (entry, true)
        }
    };

    if !is_leader {
        let mut guard = in_flight_arc.res.lock();
        while guard.is_none() {
            in_flight_arc.cond.wait(&mut guard);
        }
        let shared = guard.as_ref().unwrap().clone();
        drop(guard);
        return match shared {
            Ok(any) => match T::from_any(any) {
                Some(v) => Ok(v),
                // The single-flight leader fetched the other representation.
                // Same bytes, so fetch directly for this one: identical result,
                // one extra fetch in a mixed-representation race.
                None => fetch_metadata_network_inner::<T>(package, &cache_key, identity),
            },
            Err(e) => Err(e),
        };
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fetch_metadata_network_inner::<T>(package, &cache_key, identity)
    }))
    .unwrap_or_else(|_| Err("metadata fetch panicked".to_string()));

    {
        let mut map = in_flight_packuments().lock();
        map.remove(&cache_key);
        let mut guard = in_flight_arc.res.lock();
        *guard = Some(result.clone().map(MetaRepr::into_any));
        in_flight_arc.cond.notify_all();
    }

    result
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
    fetch_metadata_for_constraint_any::<serde_json::Value>(package, constraint)
}

/// E60 — constraint-aware fetch parsed into a typed record.
pub fn fetch_metadata_record_for_constraint(
    package: &str,
    constraint: &str,
) -> Result<PackumentRecord, String> {
    fetch_metadata_for_constraint_any::<PackumentRecord>(package, constraint)
}

pub fn fetch_metadata_for_constraint_any<T: MetaRepr>(
    package: &str,
    constraint: &str,
) -> Result<T, String> {
    // An exact pin has exactly one legal candidate. Reuse the manifest-first
    // path so JAGR avoids downloading a whole packument for that single version.
    // Ranges still need the complete version set for backtracking and stay on
    // the full-packument path below.
    if is_exact_version_pin(constraint) {
        return fetch_metadata_manifest_first_any::<T>(package, &[constraint.to_string()]);
    }
    if bundled_index_allowed(std::slice::from_ref(&constraint.to_string())) {
        if let Some(synth) = synthesize_packument_from_bundled_index(package, constraint) {
            return T::from_value(synth);
        }
    }
    let mut packument = fetch_metadata_any::<T>(package)?;
    packument.prune_constraint(constraint);
    Ok(packument)
}

/// Fetch the complete network packument, bypassing every zero-network synth
/// layer (bundled-index and manifest synth). Used as an escalation when a
/// constraint cannot be satisfied by a single-version synth — a partial
/// packument would otherwise cause a false "unresolved" for ranged requests
/// like `^18.3.1` when the bundled/manifest "latest" does not match.
pub fn fetch_metadata_full_packument(package: &str) -> Result<serde_json::Value, String> {
    fetch_metadata_full_packument_any::<serde_json::Value>(package)
}

/// E60 — full (non-synth) packument parsed into a typed record.
pub fn fetch_metadata_record_full_packument(package: &str) -> Result<PackumentRecord, String> {
    fetch_metadata_full_packument_any::<PackumentRecord>(package)
}

pub fn fetch_metadata_full_packument_any<T: MetaRepr>(package: &str) -> Result<T, String> {
    let identity = RegistryIdentity::for_package(package);
    let cache_key = metadata_cache_key_with_identity(package, &identity);

    // Persistent disk cache first (real, complete packuments only)
    for abbreviated in &[true, false] {
        if !crate::origin::packument_fresh_with_identity(package, *abbreviated, &identity) {
            continue;
        }
        let (cached_body, _cached_etag) =
            crate::origin::read_packument_cache_with_identity(package, *abbreviated, &identity);
        if let Some(body) = cached_body.filter(|b| !b.is_empty()) {
            if let Ok(parsed) = T::parse_cached(&body) {
                if !parsed.is_synth() && parsed.versions_non_empty() {
                    let mut cache = get_metadata_cache()
                        .lock()
                        .expect("metadata_cache: mutex lock failed (poisoned)");
                    cache.insert(cache_key.clone(), CachedMetadata::new(body.clone()));
                    parsed.read_cache(package);
                    return Ok(parsed);
                }
            }
        }
    }

    // Soul Path: try daemon shared memory arena for a complete packument
    if let Some(cached_bytes) =
        crate::daemon_cache::DaemonCache::lookup_with_identity(package, true, &identity)
    {
        if let Ok(parsed) = T::parse_cached(&cached_bytes) {
            if parsed.versions_non_empty() {
                let mut cache = get_metadata_cache()
                    .lock()
                    .expect("metadata_cache: mutex lock failed (poisoned)");
                cache.insert(cache_key, CachedMetadata::new(cached_bytes));
                return Ok(parsed);
            }
        }
    }

    // Network (no synth fallbacks)
    let body = match fetch_packument_with_etag(package, true, &identity) {
        Ok(b) => b,
        Err(_) => fetch_packument_with_etag(package, false, &identity)?,
    };
    let timing = std::env::var_os("JHOL_HTTP_TIMING").is_some();
    let parse_started = timing.then(std::time::Instant::now);
    let parsed = T::parse_raw(&body)?;
    if let Some(started) = parse_started {
        eprintln!(
            "[PACKUMENT-PARSE-FULL] package={} bytes={} elapsed_ms={}",
            package,
            body.len(),
            started.elapsed().as_millis()
        );
    }

    parsed.index(package, &cache_key);
    let mut cache = get_metadata_cache()
        .lock()
        .expect("metadata_cache: mutex lock failed (poisoned)");
    cache.insert(cache_key.clone(), CachedMetadata::new(body.clone()));
    parsed.read_cache(package);
    crate::daemon_cache::DaemonCache::record_with_identity(package, true, body, &identity);

    Ok(parsed)
}

/// Fetch metadata unconditionally from network, bypassing disk packument cache.
/// Used when local packument cache is stale and does not contain newly published versions.
pub fn fetch_metadata_fresh(package: &str) -> Result<serde_json::Value, String> {
    if offline_mode() {
        return Err(format!(
            "offline mode: packument for {} not in cache (no network allowed)",
            package
        ));
    }
    let identity = RegistryIdentity::for_package(package);
    let body = match fetch_packument_with_etag(package, true, &identity) {
        Ok(b) => b,
        Err(_) => fetch_packument_with_etag(package, false, &identity)?,
    };
    let parsed: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    cache_in_index_with_identity(package, &parsed, &identity);
    // Keep the byte-level cache (what `get_cached_packument` answers from) in
    // step with this fresh fetch — this path does not go through
    // `fetch_metadata_network_inner`.
    {
        let mut cache = get_metadata_cache()
            .lock()
            .expect("metadata_cache: mutex lock failed (poisoned)");
        cache.insert(
            metadata_cache_key_with_identity(package, &identity),
            CachedMetadata::new(body.clone()),
        );
    }
    Ok(parsed)
}

/// Fetch metadata for multiple packages in parallel using rayon
pub fn fetch_metadata_parallel(
    packages: &[String],
) -> Vec<(String, Result<serde_json::Value, String>)> {
    use rayon::prelude::*;
    let identities: HashMap<String, RegistryIdentity> = packages
        .iter()
        .map(|package| (package.clone(), RegistryIdentity::for_package(package)))
        .collect();

    // Check packument read cache first (O(1), non-blocking)
    let mut cached: Vec<(String, Result<serde_json::Value, String>)> = Vec::new();
    let mut to_fetch: Vec<String> = Vec::new();
    for pkg in packages {
        if let Some(meta) = get_cached_packument_with_identity(
            pkg,
            identities.get(pkg).expect("captured registry identity"),
        ) {
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
        .map(|package| {
            let identity = identities.get(package).expect("captured registry identity");
            let result = fetch_metadata_any_with_identity::<serde_json::Value>(package, identity);
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
    let identity = RegistryIdentity::for_package(package);
    let meta = fetch_metadata_any_with_identity::<serde_json::Value>(package, &identity)?;
    cache.insert(package.to_string(), meta.clone());
    // Cache in zero-copy metadata mmap for future cross-project hits
    if let Some(versions) = meta.get("versions").and_then(|v| v.as_object()) {
        for (ver, pkg_data) in versions {
            if let Some(obj) = pkg_data.as_object() {
                let val: serde_json::Value = serde_json::Value::Object(obj.clone());
                crate::zero_copy_metadata::cache_packument_metadata_for_key(
                    &metadata_cache_key_with_identity(package, &identity),
                    package,
                    ver,
                    &val,
                );
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
    // Resolve registry/auth identity on the coordinator before any cache or
    // network workers start. A config reload during this batch cannot split
    // one resolution across registry identities.
    let identities: HashMap<String, RegistryIdentity> = names
        .iter()
        .map(|name| (name.clone(), RegistryIdentity::for_package(name)))
        .collect();
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
    // Each name's read+gunzip+parse is independent; this used to run
    // sequentially on the coordinator, making cache-hit metadata CPU-bound.
    let disk_identities = identities.clone();
    let disk_hits: std::collections::HashMap<String, serde_json::Value> =
        crate::pool::parallel_map(to_fetch.clone(), packument_concurrency(), 2, move |name| {
            let identity = disk_identities
                .get(&name)
                .expect("captured registry identity");
            let mut hit: Option<serde_json::Value> = None;
            for abbreviated in &[true, false] {
                if !crate::origin::packument_fresh_with_identity(&name, *abbreviated, identity) {
                    continue;
                }
                if let (Some(body), _) =
                    crate::origin::read_packument_cache_with_identity(&name, *abbreviated, identity)
                {
                    if body.is_empty() {
                        continue;
                    }
                    if let Ok(parsed) = parse_json_pruned(&body) {
                        if parsed
                            .get("versions")
                            .and_then(|v| v.as_object())
                            .map(|o| !o.is_empty())
                            .unwrap_or(false)
                        {
                            hit = Some(parsed);
                            break;
                        }
                    }
                }
            }
            (name, hit)
        })
        .into_iter()
        .filter_map(|(name, hit)| hit.map(|v| (name, v)))
        .collect();

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

    // Network fetch for remaining packages. This is IO-bound (one HTTP round trip
    // per package), so it runs on the dedicated 64-thread pool at
    // `packument_concurrency()` (default: 4x cores, clamped 16..64) rather than
    // the default rayon pool, which is sized to the CPU count and made cold
    // resolution latency-bound.
    let network_identities = identities;
    let fetched: Vec<(String, Result<serde_json::Value, String>)> = crate::pool::parallel_map(
        still_need_network,
        packument_concurrency(),
        4,
        move |name| {
            let identity = network_identities
                .get(&name)
                .expect("captured registry identity");
            let res = fetch_metadata_any_with_identity::<serde_json::Value>(&name, identity);
            if let Ok(ref meta) = res {
                let is_synth = meta.get("_vortex_manifest_synth").is_some()
                    || meta.get("_bundled_index_synth").is_some();
                // Never persist synth packuments to the disk cache: a
                // single-version synth would later be read back by
                // fetch_metadata_full_packument and shadow the real (complete)
                // packument, causing false "unresolved" for ranged constraints.
                if !is_synth {
                    let body = serde_json::to_vec(meta).unwrap_or_default();
                    let _ = crate::origin::write_packument_cache_with_identity(
                        &name, false, &body, None, identity,
                    );
                }
                if let Some(versions) = meta.get("versions").and_then(|v| v.as_object()) {
                    for (ver, pkg_data) in versions {
                        if let Some(obj) = pkg_data.as_object() {
                            let val = serde_json::Value::Object(obj.clone());
                            crate::zero_copy_metadata::cache_packument_metadata_for_key(
                                &metadata_cache_key_with_identity(&name, identity),
                                &name,
                                ver,
                                &val,
                            );
                        }
                    }
                }
            }
            (name, res)
        },
    );

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
    let base = base_name(package);
    match fetch_metadata(&base) {
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
    let Ok(version) = Version::parse(version) else {
        return false;
    };
    version_satisfies_parsed(spec, &version)
}

/// Check a parsed version against npm's range semantics without reparsing it.
pub(crate) fn version_satisfies_parsed(spec: &str, version: &Version) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        // npm's `*` matches **stable** versions only: semver excludes prereleases
        // unless the range itself names one (e.g. `^4.0.0-nightly.1`). Returning
        // true for any parseable version let max-version selection over a `*`
        // constraint hoist a published nightly into the tree — reachable on the
        // JAGR path even after `resolve_range` was fixed, because this predicate is
        // what that selection filters with.
        return version.pre.is_empty();
    }
    // npm semantics: a bare version pin (e.g. `4.1.6`) is EXACT, NOT caret.
    // `semver::VersionReq::parse("4.1.6")` behaves like `^4.1.6` and wrongly
    // satisfies 4.2.11. Honour the exact pin before building a VersionReq.
    if is_exact_version_pin(spec) {
        let bare = spec
            .strip_prefix('v')
            .or_else(|| spec.strip_prefix('=').map(str::trim))
            .unwrap_or(spec);
        return Version::parse(bare).is_ok_and(|pin| version == &pin);
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
            cached_version_req(part).is_some_and(|req| req.matches(version))
        });
    }
    let Some(req) = cached_version_req(spec) else {
        return false;
    };
    req.matches(version)
}

/// A range compiled once, so matching N published versions does not take the
/// global spec cache lock N times.
///
/// `version_satisfies_parsed` is correct but is written for one-off checks: it
/// trims, probes for an exact pin, and takes a mutex-guarded cache lock on every
/// call. Selecting a version means answering "does this spec match?" for every
/// published version of the target, and on a real 275-package corpus that was
/// 141 ms of the 145 ms chart build. Compiling the spec first — exact pin, `*`,
/// `||` alternatives, or a parsed `VersionReq` — makes the inner loop pure
/// comparison with no allocation, no hashing and no locking.
///
/// Semantics are identical to `version_satisfies_parsed` by construction: the
/// same predicates decide the variant, and the same `matches` decides the answer.
pub(crate) enum CompiledSpec {
    /// `""` or `*`: stable versions only (npm excludes prereleases).
    Any,
    /// A bare pin: exact equality, not caret.
    Exact(Version),
    /// One `VersionReq`, or several joined by `||`.
    Alternatives(Vec<VersionReq>),
    /// Unparseable: matches nothing, exactly as the one-off predicate does.
    Unsupported,
}

impl CompiledSpec {
    #[inline]
    pub(crate) fn matches(&self, version: &Version) -> bool {
        match self {
            CompiledSpec::Any => version.pre.is_empty(),
            CompiledSpec::Exact(pin) => version == pin,
            CompiledSpec::Alternatives(reqs) => reqs.iter().any(|r| r.matches(version)),
            CompiledSpec::Unsupported => false,
        }
    }
}

pub(crate) fn compile_spec(spec: &str) -> CompiledSpec {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return CompiledSpec::Any;
    }
    if is_exact_version_pin(spec) {
        let bare = spec
            .strip_prefix('v')
            .or_else(|| spec.strip_prefix('=').map(str::trim))
            .unwrap_or(spec);
        return match Version::parse(bare) {
            Ok(pin) => CompiledSpec::Exact(pin),
            Err(_) => CompiledSpec::Unsupported,
        };
    }
    if spec.contains("||") {
        let mut reqs = Vec::new();
        for part in spec.split("||") {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some(req) = cached_version_req(part) {
                reqs.push(req);
            }
        }
        return CompiledSpec::Alternatives(reqs);
    }
    match cached_version_req(spec) {
        Some(req) => CompiledSpec::Alternatives(vec![req]),
        None => CompiledSpec::Unsupported,
    }
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

/// Fetch the **full** packument, including the fields the abbreviated format
/// omits: `time` (per-version publish dates) and per-version `license`.
///
/// The cache deliberately stores abbreviated documents — they are several times
/// smaller and carry everything an install needs. Historical resolution
/// (`--as-of`) and policy masks need facts that only the full document has, so
/// those paths ask for it explicitly and pay for it only when asked.
pub fn fetch_metadata_full(package: &str) -> Result<serde_json::Value, String> {
    let identity = RegistryIdentity::for_package(package);
    let bytes = fetch_packument_with_etag(package, false, &identity)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("full packument for {package}: {e}"))
}

/// True if the packument contains the exact given version./// True if the packument contains the exact given version.
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
        // npm semantics: `*` matches stable versions only (semver excludes
        // prereleases unless the range names one). Returning the absolute highest
        // version handed `ms` a published nightly (`4.0.0-nightly...`) to every
        // package declaring `"ms": "*"`. Prefer the newest stable; fall back to the
        // newest overall so a prerelease-only package stays installable.
        return parsed
            .iter()
            .rev()
            .find(|v| v.pre.is_empty())
            .or_else(|| parsed.last())
            .map(|v| v.to_string());
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
        // Same npm rule as `resolve_range`: newest stable wins; a prerelease only
        // when nothing stable exists.
        return parsed
            .iter()
            .rev()
            .find(|(v, _)| v.pre.is_empty())
            .or_else(|| parsed.last())
            .map(|(_, s)| s.to_string());
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
    let optional_keys: std::collections::HashSet<&str> = ver_obj
        .get("optionalDependencies")
        .and_then(|d| d.as_object())
        .map(|o| o.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    if let Some(deps) = ver_obj.get("dependencies").and_then(|d| d.as_object()) {
        for (k, v) in deps {
            if !optional_keys.contains(k.as_str()) {
                if let Some(s) = v.as_str() {
                    out.insert(k.clone(), s.to_string());
                }
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
    let token = crate::config::registry_auth_token_for_url(Path::new("."), url);
    // Tarball transfer was invisible to `JHOL_HTTP_TIMING`: the packument path
    // logs, this one did not, so a run that was dominated by payload transfer
    // showed zero requests and looked like local compute. Measure it where it
    // happens.
    let started = std::env::var_os("JHOL_HTTP_TIMING").map(|_| std::time::Instant::now());
    let result = crate::http_client::get_to_file_with_bearer(url, dest, token.as_deref());
    if let Some(started) = started {
        let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        eprintln!(
            "[TB-T] tarball {:>7} ms {:>10}B  {}",
            started.elapsed().as_millis(),
            bytes,
            url
        );
    }
    result?;
    Ok(dest.to_path_buf())
}

/// Streaming download with concurrent extraction
/// Downloads tarball while simultaneously extracting to reduce total time
pub fn download_and_extract_streaming(
    url: &str,
    dest_dir: &Path,
    expected_integrity: Option<&str>,
) -> Result<PathBuf, String> {
    let timed = std::env::var_os("JHOL_HTTP_TIMING").map(|_| std::time::Instant::now());
    let result = download_and_extract_streaming_inner(url, dest_dir, expected_integrity);
    if let Some(started) = timed {
        eprintln!(
            "[TB-T] tarball+extract {:>7} ms  {}",
            started.elapsed().as_millis(),
            url
        );
    }
    result
}

fn download_and_extract_streaming_inner(
    url: &str,
    dest_dir: &Path,
    expected_integrity: Option<&str>,
) -> Result<PathBuf, String> {
    use flate2::read::GzDecoder;

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
    let mut tmp_guard = crate::safe_file::TmpFileGuard::new(tmp);
    let tmp_path = tmp_guard.path().to_path_buf();

    // Download with streaming support
    let token = crate::config::registry_auth_token_for_url(Path::new("."), url);
    crate::http_client::get_to_file_with_bearer(url, &tmp_path, token.as_deref())?;

    // Verify integrity if provided
    if let Some(sri) = expected_integrity {
        if !crate::utils::verify_sri(&tmp_path, sri) {
            return Err("Integrity check failed during streaming download".to_string());
        }
    }

    // Extract while potentially still downloading (in full implementation would use channels)
    // For now, extract after download completes
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;

    let f = File::open(&tmp_path).map_err(|e| e.to_string())?;
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
        validate_archive_path(
            &path_str,
            dest_dir,
            &parts[1..],
            entry.header().entry_type(),
            entry.link_name().ok().flatten().as_deref(),
        )?;
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
    tmp_guard.disarm();
    let _ = std::fs::remove_file(&tmp_path);

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
    let store_file = crate::content::tarball_path(cache_dir, &hash);
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

/// Peer + platform truth per name@version (`binary-packages/meta.json`),
/// extracted from each archive's own package.json. A metadata synth without
/// these fields would claim "no peers, every platform" — peer validation
/// silently off and foreign-platform optionals admitted.
fn binary_package_meta_index(
) -> Option<&'static HashMap<String, HashMap<String, serde_json::Value>>> {
    static M: OnceLock<Option<HashMap<String, HashMap<String, serde_json::Value>>>> =
        OnceLock::new();
    M.get_or_init(|| {
        let dir = binary_packages_dir()?;
        let raw = std::fs::read_to_string(dir.join("meta.json")).ok()?;
        serde_json::from_str::<HashMap<String, HashMap<String, serde_json::Value>>>(&raw).ok()
    })
    .as_ref()
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

/// Retrieve the precomputed SHA-256 hash for a bundled binary package directly from index.
pub fn binary_package_hash(package: &str, version: &str) -> Option<String> {
    let index = binary_package_index()?;
    let key = format!("{}@{}", package, version);
    index.get(&key).cloned()
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
#[allow(dead_code)]
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

    let n = TMP_DOWNLOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = shared_unpacked.with_extension(format!("tmp-{}-{}", std::process::id(), n));
    let mut tmp_guard = crate::safe_file::TmpDirGuard::new(tmp);
    let tmp_path = tmp_guard.path().to_path_buf();
    if tmp_path.exists() {
        let _ = std::fs::remove_dir_all(&tmp_path);
    }
    std::fs::create_dir_all(&tmp_path).map_err(|e| e.to_string())?;
    extract_tarball_bytes_to_dir(tarball, &tmp_path)?;
    mark_store_extract_complete(&tmp_path);

    if std::fs::rename(&tmp_path, &shared_unpacked).is_err() && !shared_unpacked.exists() {
        return Err("failed to materialize shared binary unpacked cache".to_string());
    }
    tmp_guard.disarm();
    let _ = std::fs::remove_dir_all(&tmp_path);

    Ok(Some(shared_unpacked))
}

fn link_or_copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    remove_store_dir(dst);

    // Symlink first — harvesting a foreign store into a project cache should be
    // O(1). Clonefile/copy is the fallback when the FS rejects symlinks.
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

    #[cfg(feature = "pipeline")]
    {
        if crate::jgrid::materialize::Cloner::clone_directory(src, dst).is_ok() {
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

#[allow(dead_code)]
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

/// Cache a SHA-512 value only after the caller has verified the artifact bytes.
/// Lockfile generation can then reuse this digest instead of rereading the tarball.
///
/// Also mirrors the verified tarball + its SRI sidecar into the machine's durable
/// origin, so a later install using a different (fresh/bench) `JHOL_CACHE_DIR`
/// can *harvest* the bytes instead of re-downloading them. This is the content
/// half of the same "the machine learns once" idea as the packument write-through
/// in `write_packument_cache`; without it the durable store only ever holds what
/// happened to be installed through the default cache dir.
fn persist_verified_sri_sidecar(cache_dir: &Path, hash: &str, expected_integrity: Option<&str>) {
    let Some(sri) = expected_integrity
        .map(str::trim)
        .filter(|sri| sri.starts_with("sha512-"))
    else {
        return;
    };
    // The current cache is the primary target. `content` owns the layout, so the
    // sidecar and the tarball it describes can never be written to different
    // paths by two modules.
    crate::content::write_sri(cache_dir, hash, sri);
    // Then write through to the machine's durable origin (see the doc above).
    // Best-effort: a cache that cannot be mirrored costs a future download.
    crate::content::mirror_tarball(hash, sri, &crate::content::tarball_path(cache_dir, hash));
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum SessionTarballKey {
    Lookup(String),
    Verified {
        cache_dir: PathBuf,
        package: String,
        url: String,
        integrity: Option<String>,
    },
}

static SESSION_TARBALL_HASHES: OnceLock<dashmap::DashMap<SessionTarballKey, String>> =
    OnceLock::new();

#[cfg(test)]
#[test]
fn session_tarball_reuse_requires_callers_integrity() {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use sha2::Sha512;
    let cache = tempfile::tempdir().unwrap();
    let bytes = b"previously verified bytes";
    let hash = format!("{:x}", Sha256::digest(bytes));
    let file = crate::content::tarball_path(cache.path(), &hash);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, bytes).unwrap();
    let unpacked = crate::content::unpacked_dir(cache.path(), &hash);
    std::fs::create_dir_all(&unpacked).unwrap();
    mark_store_extract_complete(&unpacked);
    let package = format!("jhol-sri-session-{}@1.0.0", cache.path().display());
    let url = "not a registry URL";
    record_session_tarball_hash(&package, &hash);
    let wrong_sri = format!(
        "sha512-{}",
        BASE64.encode(Sha512::digest(b"different bytes"))
    );
    let result = download_tarball_to_store_hash_only(url, cache.path(), &package, Some(&wrong_sri));
    assert!(
        result.is_err(),
        "session returned bytes without checking caller SRI: {result:?}"
    );
}

#[cfg(test)]
#[test]
fn concurrent_tarball_reuse_partitions_sri_and_preserves_matching_reuse() {
    // Set process policy before the fixture starts workers; never mutate the
    // parent test process environment or populate its machine-wide caches.
    if std::env::var_os("JHOL_SRI_FIXTURE_CHILD").is_none() {
        let isolated = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "registry::concurrent_tarball_reuse_partitions_sri_and_preserves_matching_reuse",
                "--nocapture",
            ])
            .env("JHOL_SRI_FIXTURE_CHILD", "1")
            .env("JHOL_NO_HARVEST", "1")
            .env("JHOL_PREBUILT", "0")
            .env("JHOL_JAGRS_MIRROR", "0")
            .env("JHOL_CACHE_DIR", isolated.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha2::Sha512;
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    let cache = tempfile::tempdir().unwrap();
    let package = format!("jhol-sri-concurrent-{}@1.0.0", cache.path().display());
    let mut archive = tar::Builder::new(Vec::new());
    let manifest = b"{\"name\":\"jhol-sri-fixture\",\"version\":\"1.0.0\"}";
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "package/package.json", &manifest[..])
        .unwrap();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&archive.into_inner().unwrap()).unwrap();
    let body = gzip.finish().unwrap();
    let good_sri = format!("sha512-{}", STANDARD.encode(Sha512::digest(&body)));
    let wrong_sri = format!("sha512-{}", STANDARD.encode(Sha512::digest(b"wrong bytes")));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/fixture.tgz", listener.local_addr().unwrap());
    let done = Arc::new(AtomicBool::new(false));
    let count = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server_done = done.clone();
    let server_count = count.clone();
    let server = std::thread::spawn(move || {
        while !server_done.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let _ = socket.read(&mut buffer);
                    if server_count.fetch_add(1, Ordering::AcqRel) == 0 {
                        started_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes());
                    let _ = socket.write_all(&body);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("test registry accept: {error}"),
            }
        }
    });
    let good = std::thread::spawn({
        let url = url.clone();
        let cache = cache.path().to_path_buf();
        let package = package.clone();
        let sri = good_sri.clone();
        move || download_tarball_to_store_hash_only(&url, &cache, &package, Some(&sri))
    });
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let bad = std::thread::spawn({
        let url = url.clone();
        let cache = cache.path().to_path_buf();
        let package = package.clone();
        move || download_tarball_to_store_hash_only(&url, &cache, &package, Some(&wrong_sri))
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut partitioned = false;
    while Instant::now() < deadline {
        let pending = in_flight_tarballs().lock();
        partitioned = pending
            .keys()
            .filter(|key| {
                matches!(key,
            SessionTarballKey::Verified { package: p, .. } if p == &package)
            })
            .count()
            == 2;
        drop(pending);
        if partitioned {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    release_tx.send(()).unwrap();
    let good_result = good.join().unwrap();
    let bad_result = bad.join().unwrap();
    done.store(true, Ordering::Release);
    server.join().unwrap();
    // No registry is listening now: matching reuse must succeed locally.
    let served = count.load(Ordering::Acquire);
    let reused = download_tarball_to_store_hash_only(&url, cache.path(), &package, Some(&good_sri));
    assert!(
        partitioned,
        "different SRI callers shared one in-flight result"
    );
    assert!(good_result.is_ok(), "{good_result:?}");
    assert!(
        bad_result.is_err(),
        "wrong SRI caller inherited success: {bad_result:?}"
    );
    assert_eq!(good_result, reused);
    assert!(
        served >= 2,
        "distinct SRI requests did not verify independently"
    );
}

pub fn get_session_tarball_hash(key: &str) -> Option<String> {
    let map = SESSION_TARBALL_HASHES.get_or_init(dashmap::DashMap::new);
    map.get(&SessionTarballKey::Lookup(key.to_string()))
        .map(|v| v.value().clone())
}

pub fn record_session_tarball_hash(key: &str, hash: &str) {
    let map = SESSION_TARBALL_HASHES.get_or_init(dashmap::DashMap::new);
    map.insert(SessionTarballKey::Lookup(key.to_string()), hash.to_string());
}

pub fn clear_session_tarball_hashes() {
    if let Some(map) = SESSION_TARBALL_HASHES.get() {
        map.clear();
    }
}

struct InFlightTarball {
    res: parking_lot::Mutex<Option<Result<String, String>>>,
    cond: parking_lot::Condvar,
}

static IN_FLIGHT_TARBALLS: OnceLock<
    parking_lot::Mutex<HashMap<SessionTarballKey, Arc<InFlightTarball>>>,
> = OnceLock::new();

fn in_flight_tarballs(
) -> &'static parking_lot::Mutex<HashMap<SessionTarballKey, Arc<InFlightTarball>>> {
    IN_FLIGHT_TARBALLS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// Download tarball to store and return hash. Does not update the store index.
pub fn download_tarball_to_store_hash_only(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
) -> Result<String, String> {
    // Only reuse a result verified for this exact request. Package/URL lookup
    // entries remain hash hints for lockfile/capsule writing, not SRI proofs.
    let dedupe_key = SessionTarballKey::Verified {
        cache_dir: std::path::absolute(cache_dir).map_err(|error| error.to_string())?,
        package: pkg_key.to_string(),
        url: url.to_string(),
        integrity: expected_integrity.map(str::to_string),
    };
    let session = SESSION_TARBALL_HASHES.get_or_init(dashmap::DashMap::new);
    if let Some(hash) = session.get(&dedupe_key).map(|entry| entry.value().clone()) {
        let store_file = crate::content::tarball_path(cache_dir, &hash);
        let unpacked = crate::content::unpacked_dir(cache_dir, &hash);
        if store_file.exists() && store_extract_complete(&unpacked) {
            return Ok(hash);
        }
    }

    let (in_flight_arc, is_leader) = {
        let mut map = in_flight_tarballs().lock();
        if let Some(existing) = map.get(&dedupe_key).cloned() {
            (existing, false)
        } else {
            let entry = Arc::new(InFlightTarball {
                res: parking_lot::Mutex::new(None),
                cond: parking_lot::Condvar::new(),
            });
            map.insert(dedupe_key.clone(), Arc::clone(&entry));
            (entry, true)
        }
    };

    if !is_leader {
        let mut guard = in_flight_arc.res.lock();
        while guard.is_none() {
            in_flight_arc.cond.wait(&mut guard);
        }
        return guard.as_ref().unwrap().clone();
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        download_tarball_to_store_hash_only_inner(url, cache_dir, pkg_key, expected_integrity)
    }))
    .unwrap_or_else(|_| Err("tarball download panicked".to_string()));

    if let Ok(hash) = &result {
        session.insert(dedupe_key.clone(), hash.clone());
    }
    {
        let mut map = in_flight_tarballs().lock();
        map.remove(&dedupe_key);
        let mut guard = in_flight_arc.res.lock();
        *guard = Some(result.clone());
        in_flight_arc.cond.notify_all();
    }

    result
}

fn download_tarball_to_store_hash_only_inner(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
) -> Result<String, String> {
    let store_dir = crate::content::store_dir(cache_dir);
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;

    if let Some((package, version)) = parse_exact_package_spec(pkg_key) {
        if let Ok(Some(tarball)) = load_binary_tarball(&package, &version) {
            let hash = format!("{:x}", Sha256::digest(&tarball));

            if let Some(sri) = expected_integrity {
                if !crate::utils::verify_sri_bytes(&tarball, sri) {
                    // Binary cache returned a tarball but its bytes don't
                    // match the lockfile's sha512 integrity. This happens
                    // when the lockfile pins an exact version that the
                    // bundled top-200 cache doesn't carry (the cache
                    // stores the latest version per package, not every
                    // version). Fall through to the network download path
                    // below, which uses the lockfile's resolved URL and
                    // integrity. The binary cache remains a fast path for
                    // exact-version matches.
                    if std::env::var("JHOL_DEBUG_DOWNLOAD").is_ok() {
                        eprintln!(
                            "[DEBUG] binary cache miss for {}@{}: integrity mismatch, falling through to network",
                            package, version
                        );
                    }
                } else {
                    // Integrity matches — use the binary cache tarball.
                    // JAGR-S: store tarball bytes in append-only log
                    if crate::jagrs::mirror_enabled() {
                        let _ = crate::jagrs::store_tarball(&tarball);
                    }

                    // Pre-populate sharded unpacked store to avoid a second decompression pass in cold installs.
                    let unpacked = crate::content::unpacked_dir(cache_dir, &hash);
                    if !store_extract_complete(&unpacked) {
                        remove_store_dir(&unpacked);
                        let mut unpacked_ready = false;
                        if let Some(shared_unpacked) =
                            ensure_shared_binary_unpacked(&hash, &tarball)?
                        {
                            if store_extract_complete(&shared_unpacked) {
                                unpacked_ready =
                                    link_or_copy_dir(&shared_unpacked, &unpacked).is_ok();
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

                    let store_file = crate::content::tarball_path(cache_dir, &hash);
                    if !store_file.exists() {
                        std::fs::write(&store_file, &tarball).map_err(|e| e.to_string())?;
                    }
                    persist_verified_sri_sidecar(cache_dir, &hash, expected_integrity);
                    record_session_tarball_hash(pkg_key, &hash);
                    record_session_tarball_hash(url, &hash);
                    return Ok(hash);
                }
            } else {
                // No expected integrity (e.g. binary-index fast path) —
                // trust the binary cache.
                if crate::jagrs::mirror_enabled() {
                    let _ = crate::jagrs::store_tarball(&tarball);
                }
                let unpacked = crate::content::unpacked_dir(cache_dir, &hash);
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
                let store_file = crate::content::tarball_path(cache_dir, &hash);
                if !store_file.exists() {
                    std::fs::write(&store_file, &tarball).map_err(|e| e.to_string())?;
                }
                record_session_tarball_hash(pkg_key, &hash);
                record_session_tarball_hash(url, &hash);
                return Ok(hash);
            }
        }
    }

    // Store index, then binary/bundled indexes (no preseed rewrite required).
    if let Some(hash) = store_or_binary_hash(pkg_key) {
        let store_file = crate::content::tarball_path(cache_dir, &hash);
        let unpacked = crate::content::unpacked_dir(cache_dir, &hash);
        if store_file.exists() && store_extract_complete(&unpacked) {
            let integrity_ok = match expected_integrity {
                Some(sri) => {
                    let sidecar = crate::content::sri_sidecar_path(cache_dir, &hash);
                    if let Ok(cached_sri) = std::fs::read_to_string(&sidecar) {
                        cached_sri.trim() == sri.trim()
                    } else {
                        crate::utils::verify_sri(&store_file, sri)
                    }
                }
                None => true,
            };
            if integrity_ok {
                record_session_tarball_hash(pkg_key, &hash);
                record_session_tarball_hash(url, &hash);
                return Ok(hash);
            }
        }
    }

    let token = crate::config::registry_auth_token_for_url(Path::new("."), url);
    if let Some(sri) = expected_integrity {
        // Prefer a complete unpacked tree already on this machine (login-home
        // store or JHOL_HARVEST_CACHES) — clonefile into this cache, skip gunzip.
        if let Some((hash, foreign_unpacked, foreign_tgz)) = crate::content::find_unpacked(sri) {
            let local_unpacked = crate::content::unpacked_dir(cache_dir, &hash);
            let store_file = crate::content::tarball_path(cache_dir, &hash);
            let mut ready = store_extract_complete(&local_unpacked);
            if !ready {
                remove_store_dir(&local_unpacked);
                if let Some(parent) = local_unpacked.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                ready = link_or_copy_dir(&foreign_unpacked, &local_unpacked).is_ok()
                    && (store_extract_complete(&local_unpacked)
                        || local_unpacked
                            .symlink_metadata()
                            .map(|m| m.file_type().is_symlink())
                            .unwrap_or(false));
                if ready
                    && local_unpacked
                        .symlink_metadata()
                        .map(|m| m.file_type().is_dir())
                        .unwrap_or(false)
                {
                    mark_store_extract_complete(&local_unpacked);
                }
            }
            if ready {
                if !store_file.exists()
                    && foreign_tgz.is_file()
                    && std::fs::hard_link(&foreign_tgz, &store_file).is_err()
                {
                    let _ = std::fs::copy(&foreign_tgz, &store_file);
                }
                persist_verified_sri_sidecar(cache_dir, &hash, expected_integrity);
                if std::env::var("JHOL_DEBUG_HARVEST").is_ok() {
                    eprintln!(
                        "[HARVEST] {} <- unpacked store (hash {}, 0 network, 0 extract)",
                        pkg_key, hash
                    );
                }
                record_session_tarball_hash(pkg_key, &hash);
                record_session_tarball_hash(url, &hash);
                return Ok(hash);
            }
        }
        if let Some(data) = crate::content::find_tarball(sri) {
            if std::env::var("JHOL_DEBUG_HARVEST").is_ok() {
                eprintln!(
                    "[HARVEST] {} <- local cache ({} bytes, 0 network)",
                    pkg_key,
                    data.len()
                );
            }
            let projection = crate::fast_stream::StreamDirectProjector::project_stream(
                &data,
                cache_dir,
                pkg_key,
                expected_integrity,
                None,
            )?;
            let hash = projection.hash;
            if crate::jagrs::mirror_enabled() {
                let _ = crate::jagrs::store_tarball(&data);
            }
            if let Some((package, version)) = parse_exact_package_spec(pkg_key) {
                if crate::jagrs::mirror_enabled() {
                    let _ = crate::jagrs::store_alias(&package, &version, &hash);
                }
            }
            let store_file = crate::content::tarball_path(cache_dir, &hash);
            if !store_file.exists() {
                std::fs::write(&store_file, &data).map_err(|e| e.to_string())?;
            }
            persist_verified_sri_sidecar(cache_dir, &hash, expected_integrity);
            record_session_tarball_hash(pkg_key, &hash);
            record_session_tarball_hash(url, &hash);
            return Ok(hash);
        }
    }

    let hash = download_tarball_to_store_streaming(
        url,
        cache_dir,
        pkg_key,
        expected_integrity,
        token.as_deref(),
    )?;
    persist_verified_sri_sidecar(cache_dir, &hash, expected_integrity);
    record_session_tarball_hash(pkg_key, &hash);
    record_session_tarball_hash(url, &hash);
    Ok(hash)
}

/// How many tarball *bodies* may be in flight at once.
///
/// The registry throttles concurrent streams. On the same 11 tarballs, a reference
/// client issuing 11 parallel HTTP/2 streams paid ~240 ms TTFB on every one, and the
/// largest package (lodash, 67% of the bytes) only finished ~380 ms after every other
/// package had already been extracted - so the whole install waited on it. The same 11
/// requests over one connection, issued back-to-back, cost ~28 ms each (~308 ms total).
///
/// Download and extraction used to be a single task, which is why simply lowering
/// `JHOL_NETWORK_CONCURRENCY` made installs *slower* (844 ms at 1, 908 ms at 2 vs 680 ms
/// default): it serialized extraction too. Holding a small window only for the network
/// read, and extracting outside it, keeps the fast path usable.
///
/// `JHOL_DOWNLOAD_WINDOW` overrides (clamped 1..=64). Default 16: measured on the
/// 87-package fixture, window 4 = 1.38 s, 8 = 0.84 s, 16 = 0.60 s — no throttle
/// penalty at 16 for small tarballs (the 11-stream/240 ms TTFB note above was
/// measured on large bodies where holding the window longer wins).
fn download_window() -> usize {
    std::env::var("JHOL_DOWNLOAD_WINDOW")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 64))
        .unwrap_or(16)
}

struct DownloadWindow {
    in_flight: std::sync::Mutex<usize>,
    cv: std::sync::Condvar,
    max: usize,
}

impl DownloadWindow {
    fn acquire(&self) -> DownloadWindowGuard<'_> {
        let mut count = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        while *count >= self.max {
            count = self.cv.wait(count).unwrap_or_else(|e| e.into_inner());
        }
        *count += 1;
        DownloadWindowGuard(self)
    }
}

struct DownloadWindowGuard<'a>(&'a DownloadWindow);

impl Drop for DownloadWindowGuard<'_> {
    fn drop(&mut self) {
        let mut count = self.0.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        self.0.cv.notify_one();
    }
}

fn download_window_state() -> &'static DownloadWindow {
    static WINDOW: std::sync::OnceLock<DownloadWindow> = std::sync::OnceLock::new();
    WINDOW.get_or_init(|| DownloadWindow {
        in_flight: std::sync::Mutex::new(0),
        cv: std::sync::Condvar::new(),
        max: download_window(),
    })
}

// Record the verified compressed stream once. Flush at EOF so a cache write
// error reaches the projector before it publishes the extracted tree.
struct CacheRecordingReader<'a> {
    inner: &'a mut dyn Read,
    cache: &'a mut dyn Write,
}

impl Read for CacheRecordingReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let count = self.inner.read(buffer)?;
        if count == 0 {
            self.cache.flush()?;
        } else {
            self.cache.write_all(&buffer[..count])?;
        }
        Ok(count)
    }
}

#[cfg(test)]
#[test]
fn cache_recording_errors_prevent_extracted_tree_publication() {
    struct Writer {
        bytes: Vec<u8>,
        fail_write: bool,
        fail_flush: bool,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.fail_write {
                return Err(std::io::Error::other("cache write failure"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                return Err(std::io::Error::other("cache flush failure"));
            }
            Ok(())
        }
    }
    let mut archive = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "package/index.js", &b"body"[..])
        .unwrap();
    let tar = archive.into_inner().unwrap();
    let mut compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    compressed.write_all(&tar).unwrap();
    let bytes = compressed.finish().unwrap();
    for (fail_write, fail_flush) in [(false, false), (true, false), (false, true)] {
        let root = tempfile::tempdir().unwrap();
        let mut input = bytes.as_slice();
        let mut writer = Writer {
            bytes: Vec::new(),
            fail_write,
            fail_flush,
        };
        let reader = CacheRecordingReader {
            inner: &mut input,
            cache: &mut writer,
        };
        let result = crate::fast_stream::StreamDirectProjector::project_reader(
            reader,
            root.path(),
            "recording-test@1.0.0",
            None,
            None,
        );
        if fail_write || fail_flush {
            assert!(result.is_err());
            assert!(
                std::fs::read_dir(root.path().join("store_unpacked/.staging"))
                    .unwrap()
                    .next()
                    .is_none()
            );
            assert_eq!(
                std::fs::read_dir(root.path().join("store_unpacked"))
                    .unwrap()
                    .count(),
                1
            );
        } else {
            assert_eq!(writer.bytes, bytes);
            assert_eq!(
                std::fs::read(result.unwrap().unpacked_dir.join("index.js")).unwrap(),
                b"body"
            );
        }
    }
}

fn download_tarball_to_store_streaming(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
    bearer_token: Option<&str>,
) -> Result<String, String> {
    let store_dir = crate::content::store_dir(cache_dir);
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;
    let n = TMP_DOWNLOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_path = store_dir.join(format!(".download-{}-{}.tmp", std::process::id(), n));
    let _temp_guard = crate::safe_file::TmpFileGuard::new(temp_path.clone());
    let stream_extract = std::env::var("JHOL_STREAM_EXTRACT").as_deref() == Ok("1");
    let timing_enabled = std::env::var_os("JHOL_HTTP_TIMING").is_some();
    let mut project_elapsed = None;
    let projection = if stream_extract {
        let _permit = download_window_state().acquire();
        crate::http_client::get_with_bearer_streaming(url, bearer_token, |body| {
            // Transport fallback can retry the callback; reset only our own
            // compressed staging file. Projector owns its private tree stage.
            let file = File::create(&temp_path)
                .map_err(|error| format!("create streamed tarball: {error}"))?;
            let mut cache = std::io::BufWriter::with_capacity(128 * 1024, file);
            let recording = CacheRecordingReader {
                inner: body,
                cache: &mut cache,
            };
            let mut reader = std::io::BufReader::with_capacity(128 * 1024, recording);
            let started = timing_enabled.then(std::time::Instant::now);
            let projected = crate::fast_stream::StreamDirectProjector::project_reader(
                &mut reader,
                cache_dir,
                pkg_key,
                expected_integrity,
                None,
            );
            if let (Some(started), Ok(_)) = (started, &projected) {
                project_elapsed = Some(started.elapsed());
            }
            projected
        })?
    } else {
        // Staged baseline releases the network slot before extraction, letting
        // the next package download overlap this package's extraction.
        {
            let _permit = download_window_state().acquire();
            crate::http_client::get_to_file_with_bearer_streaming(url, &temp_path, bearer_token)
                .map_err(|error| format!("download: {error}"))?;
        }
        let staged = File::open(&temp_path).map_err(|e| format!("open staged tarball: {}", e))?;
        let mut reader = std::io::BufReader::with_capacity(128 * 1024, staged);
        let started = timing_enabled.then(std::time::Instant::now);
        let projected = crate::fast_stream::StreamDirectProjector::project_reader(
            &mut reader,
            cache_dir,
            pkg_key,
            expected_integrity,
            None,
        )?;
        project_elapsed = started.map(|started| started.elapsed());
        projected
    };
    let hash = projection.hash;
    if let Some(elapsed) = project_elapsed {
        eprintln!(
            "[PROJECT-T] total {}ms scope={} mode={} {}",
            elapsed.as_millis(),
            if stream_extract {
                "body-consume-and-project"
            } else {
                "staged-file-project"
            },
            if stream_extract { "stream" } else { "staged" },
            pkg_key
        );
    }

    // JAGR-S remains opt-in; its current API accepts bytes, so only that mode
    // materializes the compressed body for the mirror append.
    if crate::jagrs::mirror_enabled() {
        let data = std::fs::read(&temp_path).map_err(|e| e.to_string())?;
        let _ = crate::jagrs::store_tarball(&data);
    }
    if let Some((package, version)) = parse_exact_package_spec(pkg_key) {
        if crate::jagrs::mirror_enabled() {
            let _ = crate::jagrs::store_alias(&package, &version, &hash);
        }
    }

    let store_file = crate::content::tarball_path(cache_dir, &hash);
    if !store_file.exists() {
        if let Err(e) = std::fs::rename(&temp_path, &store_file) {
            if !store_file.exists() {
                return Err(format!("publish downloaded tarball: {}", e));
            }
        }
    }
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
// Both streaming and buffered extractors must enforce the same containment rules.
pub(crate) fn validate_archive_path(
    path: &str,
    root: &Path,
    stripped: &[&str],
    entry_type: tar::EntryType,
    link_name: Option<&std::path::Path>,
) -> Result<(), String> {
    validate_archive_path_components(path, stripped, entry_type, link_name)?;
    let mut destination = root.to_path_buf();
    for component in std::iter::once("").chain(stripped.iter().copied()) {
        destination.push(component);
        if std::fs::symlink_metadata(&destination)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("Unsafe archive destination symlink".into());
        }
    }
    Ok(())
}

/// Validate entries inside a fresh private extraction stage.
///
/// The stage starts empty, so only earlier archive entries can introduce
/// symlinks. Tracking those paths avoids one `symlink_metadata` syscall per
/// path component while retaining the same symlink-ancestor rejection rule.
/// Do not use for an existing or shared extraction root.
pub(crate) fn validate_archive_path_in_private_stage(
    path: &str,
    stripped: &[&str],
    entry_type: tar::EntryType,
    link_name: Option<&std::path::Path>,
    archive_symlinks: &std::collections::HashSet<std::path::PathBuf>,
) -> Result<(), String> {
    validate_archive_path_components(path, stripped, entry_type, link_name)?;
    if !archive_symlinks.is_empty() {
        let mut relative = std::path::PathBuf::new();
        for component in stripped.iter().copied() {
            if component.is_empty() || component == "." {
                continue;
            }
            relative.push(component);
            if archive_symlinks.contains(&relative) {
                return Err("Unsafe archive destination symlink".into());
            }
        }
    }
    Ok(())
}

fn validate_archive_path_components(
    path: &str,
    stripped: &[&str],
    entry_type: tar::EntryType,
    link_name: Option<&std::path::Path>,
) -> Result<(), String> {
    if path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|part| part == "..")
    {
        return Err("Unsafe archive entry path".into());
    }
    // Hard links cannot be contained here: tar-rs resolves the link name as a raw
    // relative path (verified against tar 0.4.46, which reports e.g.
    // "when hard linking ../outside/victim.txt to <dest>/x"), i.e. relative to the
    // process working directory rather than the extraction root. A published
    // tarball could therefore link a project file (or /etc/passwd in a root
    // container) into the tree, and a later entry writing that path would rewrite
    // the linked file. npm tarballs do not use hard links, so fail closed.
    if entry_type.is_hard_link() {
        return Err("Unsafe archive hard link".into());
    }
    // npm tarballs contain no symlinks at all (measured: 0 across 71 packages /
    // 3404 files), but a non-npm publisher can craft one. A symlink whose target
    // leaves the extraction root lets later entries write through it, and lets
    // store-copy routines that follow `is_dir()` pull outside files into
    // node_modules. Allow only targets that stay inside the root.
    if entry_type.is_symlink() {
        let target = link_name
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or("Archive symlink has no target")?;
        if target.starts_with('/') || target.contains('\\') || target.contains(':') {
            return Err("Unsafe archive symlink target".into());
        }
        let mut depth = stripped.len().saturating_sub(1) as isize;
        for part in target.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth -= 1;
                    if depth < 0 {
                        return Err("Unsafe archive symlink target".into());
                    }
                }
                _ => depth += 1,
            }
        }
    }
    Ok(())
}

fn extract_tar_from_bytes(decompressed: &[u8], dest_dir: &Path) -> Result<(), String> {
    use rayon::prelude::*;
    use std::collections::HashSet;
    use std::io::{Cursor, Read};

    struct RegularFile {
        out_path: std::path::PathBuf,
        mode: u32,
        data: Vec<u8>,
    }

    struct SymlinkEntry {
        out_path: std::path::PathBuf,
        target: std::path::PathBuf,
    }

    let mut archive = tar::Archive::new(Cursor::new(decompressed));
    let mut dirs_to_create: HashSet<std::path::PathBuf> = HashSet::with_capacity(128);
    let mut archive_symlinks: HashSet<std::path::PathBuf> = HashSet::new();
    let mut symlinks: Vec<SymlinkEntry> = Vec::new();
    let mut regular_files: Vec<RegularFile> = Vec::new();

    dirs_to_create.insert(dest_dir.to_path_buf());

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let (out_path, rel_path, entry_type, link_name_buf) = {
            let path_bytes = entry.path_bytes();
            let Ok(path_str) = std::str::from_utf8(&path_bytes) else {
                return Err("Unsafe archive entry: non-UTF-8 path".into());
            };
            // Fail closed on unnormalizable entries: a backslash-only path has
            // no separator to strip, and silently omitting a file a package
            // expects is itself an attack (breakage by omission).
            let Some(idx) = path_str.find('/') else {
                return Err(format!("Unsafe archive entry path: {}", path_str));
            };
            let rel_str = &path_str[idx + 1..];
            if rel_str.is_empty() {
                return Err("Unsafe archive entry path: empty after strip".into());
            }

            let parts: smallvec::SmallVec<[&str; 8]> =
                rel_str.split('/').filter(|s| !s.is_empty()).collect();
            let entry_type = entry.header().entry_type();
            let link_name_buf = entry.link_name().ok().flatten().map(|p| p.to_path_buf());
            validate_archive_path_in_private_stage(
                path_str,
                &parts,
                entry_type,
                link_name_buf.as_deref(),
                &archive_symlinks,
            )?;
            (
                dest_dir.join(rel_str),
                std::path::PathBuf::from(rel_str),
                entry_type,
                link_name_buf,
            )
        };

        if let Some(parent) = out_path.parent() {
            dirs_to_create.insert(parent.to_path_buf());
        }
        if entry_type.is_symlink() {
            archive_symlinks.insert(rel_path);
            if let Some(target) = link_name_buf {
                symlinks.push(SymlinkEntry { out_path, target });
            }
        } else if entry_type.is_dir() {
            dirs_to_create.insert(out_path);
        } else {
            let raw_mode = entry.header().mode().unwrap_or(0o644);
            let mode = if raw_mode & 0o600 == 0 {
                raw_mode | 0o644
            } else {
                raw_mode
            };
            let size = entry.header().size().unwrap_or(0) as usize;
            let mut data = Vec::with_capacity(size);
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            regular_files.push(RegularFile {
                out_path,
                mode,
                data,
            });
        }
    }

    // 1. Create all parent directories up front
    for dir in &dirs_to_create {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }

    // 2. Parallel Rayon write for all regular files across all CPU cores
    regular_files
        .into_par_iter()
        .try_for_each(|f| -> Result<(), String> {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(f.mode)
                    .open(&f.out_path)
                    .map_err(|e| format!("Failed to create {}: {}", f.out_path.display(), e))?;
                std::io::Write::write_all(&mut file, &f.data)
                    .map_err(|e| format!("Failed to write {}: {}", f.out_path.display(), e))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::write(&f.out_path, &f.data)
                    .map_err(|e| format!("Failed to write {}: {}", f.out_path.display(), e))?;
            }
            Ok(())
        })?;

    // 3. Create symlinks sequentially (after targets exist)
    for sym in symlinks {
        let parent = sym
            .out_path
            .parent()
            .ok_or_else(|| format!("Symlink has no parent: {}", sym.out_path.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create symlink parent {}: {e}", parent.display()))?;

        let result = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&sym.target, &sym.out_path)
            }
            #[cfg(windows)]
            {
                let target = parent.join(&sym.target);
                if std::fs::metadata(&target)
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false)
                {
                    std::os::windows::fs::symlink_dir(&sym.target, &sym.out_path)
                } else {
                    std::os::windows::fs::symlink_file(&sym.target, &sym.out_path)
                }
            }
            #[cfg(not(any(unix, windows)))]
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "symlink extraction is unsupported on this platform",
                ))
            }
        };
        result.map_err(|e| format!("Failed to create symlink {}: {e}", sym.out_path.display()))?;
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
    mirror_unpacked_to_durable(dest_dir);
}

/// Mirror a freshly-extracted `store_unpacked/<shard>/<hash>` tree into the
/// machine's durable origin so a later install with a different cache dir can
/// `harvest_unpacked` it (clonefile, no re-extract). A *real* directory is
/// created — never a symlink into the ephemeral project cache, which the caller
/// may delete.
///
/// Delegates to [`crate::content`], which owns the origin's layout and root
/// list; the staging-name guard lives there too.
pub(crate) fn mirror_unpacked_to_durable(dest_dir: &Path) {
    crate::content::mirror_unpacked(dest_dir);
}

/// True when a store dir was fully extracted (marker present) OR contains real
/// content beyond an empty shell. A dir holding only `package.json` (the first
/// tar entry — the classic partial-extract signature) is treated as incomplete,
/// even if runtime relink `node_modules/` symlink dirs were added afterwards.
/// Make sure `store_unpacked/<hash>` exists for a store tarball, unpacking it if needed.
/// The store is the authority for which versions exist, so a tarball that was cached without being
/// unpacked (the prefetch/cache-hit side writes only the tarball) must be unpacked before anything
/// needs to link against it.
pub fn ensure_store_unpacked_for_tarball(tarball_path: &Path) -> Result<bool, String> {
    let cache = PathBuf::from(crate::utils::get_cache_dir());
    ensure_store_unpacked_for_tarball_in(tarball_path, &cache)
}

pub(crate) fn ensure_store_unpacked_for_tarball_in(
    tarball_path: &Path,
    cache: &Path,
) -> Result<bool, String> {
    let Some(hash) = tarball_path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    if hash.len() < 8 {
        return Ok(false);
    }
    let unpacked = crate::content::unpacked_dir(cache, hash);
    if unpacked.exists() && store_extract_complete(&unpacked) {
        return Ok(true);
    }
    let tarball = match std::fs::File::open(tarball_path) {
        Ok(file) => file,
        Err(_) => return Ok(false),
    };
    remove_store_dir(&unpacked);
    crate::fast_stream::StreamDirectProjector::project_reader_checked(
        tarball,
        cache,
        hash,
        None,
        Some(hash),
        None,
    )?;
    Ok(true)
}

pub fn store_extract_complete(dest_dir: &Path) -> bool {
    // The manifest must have content, marker or not.
    //
    // A marker records "this extract finished", but a finished extract can still
    // be unreadable: a zero-length `package.json` (truncated write, or an extract
    // interrupted after the first entry) made every later install "repair" the
    // package by re-cloning the same corruption — observable as an install that
    // reports `Added 1 packages` on every warm run and never converges, because
    // the tree never matches the lockfile. `function-bind` in a real `jest` tree
    // was 0 bytes with the marker present.
    // A manifest that exists but is empty is corruption, marker or not: every
    // later install would "repair" the package by re-cloning the same bytes, so
    // the tree never matches the lockfile (observed as a warm install reporting
    // `Added 1 packages` forever; `function-bind` in a real `jest` tree was 0
    // bytes with the marker present).
    //
    // A manifest that is *absent* is left to the marker, because extraction of
    // a tarball without a root package.json is legitimate and the marker is the
    // only evidence available for it.
    let pkg_json = dest_dir.join("package.json");
    if let Ok(meta) = pkg_json.metadata() {
        if meta.is_file() && meta.len() == 0 {
            return false;
        }
    }
    if dest_dir.join(STORE_COMPLETE_MARKER).exists() {
        // …but a marker over a directory holding **nothing else** is a truncated
        // extract, not "a tarball without a manifest". Observed as a store entry for
        // `express` with one file (the marker): every install linked the empty
        // directory, counted the package as installed, and left a tree missing 14
        // packages while the lockfile listed 71. A package without any body file is
        // not installable, so treating it as incomplete costs one re-extract.
        let body = std::fs::read_dir(dest_dir)
            .map(|rd| {
                rd.flatten().any(|e| {
                    let name = e.file_name();
                    name.to_string_lossy() != STORE_COMPLETE_MARKER
                        && e.file_type().map(|t| t.is_file() || t.is_dir()).unwrap_or(false)
                })
            })
            .unwrap_or(false);
        return body;
    }
    // Legacy mark-free dirs: a real body file (regular file beyond package.json)
    // proves the tarball was fully materialized. The relink `node_modules/`
    // sibling is a symlink dir and must not satisfy this.
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

/// Download, verify, and unpack through the shared streaming store path.
///
/// Keeping this wrapper on the same path as `download_tarball_to_store` avoids
/// buffering both the compressed archive and a second decompressed copy.
/// Returns (hash, unpacked_path).
pub fn download_unpack_to_store_unpacked(
    url: &str,
    cache_dir: &Path,
    pkg_key: &str,
    expected_integrity: Option<&str>,
) -> Result<(String, PathBuf), String> {
    let hash = download_tarball_to_store_hash_only(url, cache_dir, pkg_key, expected_integrity)?;
    let unpacked = crate::content::unpacked_dir(cache_dir, &hash);
    if !store_extract_complete(&unpacked) {
        return Err(format!(
            "verified tarball for {} has no complete unpacked store entry",
            pkg_key
        ));
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
    let unpacked = crate::content::unpacked_dir(cache_dir, hash);
    if !store_extract_complete(&unpacked) {
        if std::env::var_os("JHOL_PROFILE_INSTALL").is_some() {
            eprintln!("[STORE_UNPACKED_MISS] hash={} path={:?}", hash, unpacked);
        }
        remove_store_dir(&unpacked);
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        extract_tarball_to_dir(tarball_path, &unpacked)?;
        mark_store_extract_complete(&unpacked);
    } else if std::env::var_os("JHOL_PROFILE_INSTALL").is_some() {
        eprintln!("[STORE_UNPACKED_HIT] hash={} path={:?}", hash, unpacked);
    }
    Ok(unpacked)
}

/// Public full extract from in-memory tarball bytes (Soul Path / JAGR-S).
/// SRI string for arbitrary bytes: `sha512-<base64(digest)>`. Used by the
/// store GC to match content-addressed tarballs against lockfile integrities.
pub fn sri_of_bytes(data: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    use sha2::Sha512;
    format!("sha512-{}", B64.encode(Sha512::digest(data)))
}

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
    let store_dir = crate::content::store_dir(cache_dir);
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;

    let n = TMP_DOWNLOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = cache_dir.join(format!("tmp-{}-{}.tgz", std::process::id(), n));
    let mut tmp_guard = crate::safe_file::TmpFileGuard::new(tmp);
    let tmp_path = tmp_guard.path().to_path_buf();
    download_tarball(&tarball_url, &tmp_path).map_err(|e| format!("download: {}", e))?;
    let hash = crate::utils::content_hash(&tmp_path).map_err(|e| e.to_string())?;
    let store_file = crate::content::tarball_path(cache_dir, &hash);
    std::fs::rename(&tmp_path, &store_file)
        .or_else(|_| std::fs::copy(&tmp_path, &store_file).map(|_| ()))
        .map_err(|e| e.to_string())?;
    tmp_guard.disarm();
    let _ = std::fs::remove_file(&tmp_path);
    let mut index = crate::utils::read_store_index();
    index.insert(pkg_key, hash.clone());
    crate::utils::write_store_index(&index).map_err(|e| e.to_string())?;

    let store_file = crate::content::tarball_path(cache_dir, &hash);
    extract_tarball(&store_file, node_modules, base_name)?;
    if !options.quiet {
        crate::utils::log_file_only(&format!("Installed {}@{} (native)", base_name, version));
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

/// Warm the in-memory binary / bundled indexes at startup.
///
/// Previously this merged ~200 `binary-packages/index.json` entries into
/// `store_index.json` on every cold process (~150 ms of parse+rewrite). That
/// duplicated data already held by `binary_package_index()` / bundled hashes,
/// which download lookups consult directly via `store_or_binary_hash`. A stamp
/// file records when a full merge last ran so `JHOL_PRESEED_MERGE=1` (legacy)
/// can still rewrite once per index change instead of every install.
pub fn preseed_store_index() {
    static PRESEEDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if PRESEEDED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // Always warm the OnceLock indexes (cheap after first call).
    let _ = binary_package_index();
    let _ = bundled_data::get_bundled_hashes();

    let force_merge = std::env::var("JHOL_PRESEED_MERGE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !force_merge {
        return;
    }

    let stamp = preseed_index_stamp();
    let stamp_path = PathBuf::from(crate::utils::get_cache_dir()).join("store_index.preseed");
    if let Ok(prev) = std::fs::read_to_string(&stamp_path) {
        if prev.trim() == stamp {
            return;
        }
    }

    let mut index = crate::utils::read_store_index();
    let mut changed = false;
    if let Some(spec_map) = binary_package_index() {
        for (spec, hash) in spec_map {
            if !index.contains_key(spec) {
                index.insert(spec.clone(), hash.clone());
                changed = true;
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
    let _ = std::fs::write(&stamp_path, &stamp);
}

fn preseed_index_stamp() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Some(dir) = binary_packages_dir() {
        let index_path = dir.join("index.json");
        if let Ok(meta) = std::fs::metadata(&index_path) {
            meta.len().hash(&mut h);
            if let Ok(modified) = meta.modified() {
                modified.hash(&mut h);
            }
        }
    }
    bundled_data::get_bundled_hashes().len().hash(&mut h);
    format!("{:x}", h.finish())
}

/// Content hash for `pkg_key` from the on-disk store index, else the binary /
/// bundled package indexes (without requiring a store_index rewrite).
pub(crate) fn store_or_binary_hash(pkg_key: &str) -> Option<String> {
    if let Some(hash) = crate::utils::with_store_index(|idx| idx.get(pkg_key).cloned()) {
        return Some(hash);
    }
    if let Some(hash) = binary_package_index().and_then(|idx| idx.get(pkg_key).cloned()) {
        return Some(hash);
    }
    let hashes = bundled_data::get_bundled_hashes();
    hashes
        .binary_search_by(|(key, _)| key.as_bytes().cmp(pkg_key.as_bytes()))
        .ok()
        .map(|i| hashes[i].1.to_string())
}

/// Check if a spec key exists in the build-time bundled hash table.
/// O(log n) binary search — roughly matches how `get_bundled_tarball` would find it.
pub fn is_bundled_spec(spec: &str) -> bool {
    let hashes = bundled_data::get_bundled_hashes();
    hashes
        .binary_search_by(|(key, _)| key.as_bytes().cmp(spec.as_bytes()))
        .is_ok()
}

#[cfg(test)]
#[test]
fn scoped_registry_identity_routes_packument_and_isolates_cache() {
    // Run in an isolated process because the registry/cache policy is process-wide.
    if std::env::var_os("JHOL_SCOPED_REGISTRY_FIXTURE_CHILD").is_none() {
        let cache = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "registry::scoped_registry_identity_routes_packument_and_isolates_cache",
                "--nocapture",
            ])
            .env("JHOL_SCOPED_REGISTRY_FIXTURE_CHILD", "1")
            .env("JHOL_CACHE_DIR", cache.path())
            .env("JHOL_NO_HARVEST", "1")
            .env("JHOL_PACKUMENT_CACHE_TTL_SECS", "0")
            .env_remove("JHOL_OFFLINE")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }

    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let registry = format!("http://{address}");
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".npmrc"),
        format!("@private:registry={registry}/\n//{address}/:_authToken=local-secret\n"),
    )
    .unwrap();
    let package = "@private/example";
    let identity = RegistryIdentity::for_package_in(project.path(), package);
    assert_eq!(identity.registry, registry);
    assert_eq!(identity.token.as_deref(), Some("local-secret"));
    let key = metadata_cache_key_for(package, &identity.registry, identity.token.as_deref());
    let cache_path = crate::origin::packument_path_with_identity(package, true, &identity);
    assert!(!key.contains("local-secret"));
    assert!(!cache_path.to_string_lossy().contains("local-secret"));

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
        }
        let body = br#"{"name":"@private/example","versions":{"1.0.0":{"name":"@private/example","version":"1.0.0"}}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        String::from_utf8(request).unwrap()
    });

    let body = fetch_packument_with_etag(package, true, &identity).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed["versions"].get("1.0.0").is_some());
    let request = server.join().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /@private%2fexample "), "{request}");
    assert!(
        request.contains("authorization: bearer local-secret"),
        "scoped registry auth header missing"
    );
}

#[cfg(test)]
mod compiled_spec_tests {
    use super::{compile_spec, version_satisfies_parsed, Version};

    /// `CompiledSpec` exists purely for speed, so the one thing it must never do
    /// is answer differently from the predicate it replaces. Checked across the
    /// range grammar's shapes and a spread of versions, including prereleases,
    /// pins, wildcards, OR-ranges and junk.
    #[test]
    fn compiled_spec_agrees_with_the_one_off_predicate() {
        let specs = [
            "*", "", "^1.2.3", "~1.2.3", "1.2.3", "=1.2.3", "v1.2.3", ">=1.0.0 <2",
            ">1.0.0", "<2.0.0", "1.x", "1.2.x", "^0.0.1", "^0.0.0", ">=1.0.0",
            "^3.0.0 || ^4.0.0", "||", "^1 || ", "1.2.3 - 2.0.0", "latest",
            "not a range", "1.0.0-nightly.1", "^4.0.0-nightly.1", "~2", "^2.3",
        ];
        let versions = [
            "0.0.0", "0.0.1", "1.0.0", "1.2.3", "1.2.4", "1.3.0", "1.9.9", "2.0.0",
            "2.3.0", "3.0.0", "3.5.1", "4.0.0", "4.2.11", "1.0.0-nightly.1",
            "2.0.0-rc.1", "10.1.0",
        ];
        for spec in specs {
            let compiled = compile_spec(spec);
            for v in versions {
                let parsed = Version::parse(v).expect("test version parses");
                assert_eq!(
                    compiled.matches(&parsed),
                    version_satisfies_parsed(spec, &parsed),
                    "spec {:?} version {}: compiled vs one-off disagree",
                    spec,
                    v
                );
            }
        }
    }
}

#[cfg(test)]
mod store_extract_guard_tests {
    use super::{store_extract_complete, STORE_COMPLETE_MARKER};

    /// A finished extract can still be unreadable. A zero-length manifest with
    /// the completion marker present used to count as complete, so every later
    /// install re-cloned the same corruption: `jest` warm runs reported
    /// `Added 1 packages` forever and the tree never matched the lockfile.
    #[test]
    fn zero_length_manifest_is_not_a_complete_extract() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join(STORE_COMPLETE_MARKER), b"ok").unwrap();
        std::fs::write(d.join("package.json"), b"").unwrap();
        std::fs::write(d.join("index.js"), b"module.exports = 1;").unwrap();
        assert!(
            !store_extract_complete(d),
            "an empty manifest must not be treated as an installed package"
        );

        // Repairing the manifest makes the entry usable again, with no other
        // change to the tree.
        std::fs::write(d.join("package.json"), br#"{"name":"x","version":"1.0.0"}"#).unwrap();
        assert!(store_extract_complete(d));
    }

    #[test]
    fn marker_plus_manifest_is_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join(STORE_COMPLETE_MARKER), b"ok").unwrap();
        std::fs::write(d.join("package.json"), br#"{"name":"x","version":"1.0.0"}"#).unwrap();
        assert!(store_extract_complete(d));
    }

    /// No manifest and no marker: still incomplete (the legacy branch needs a
    /// real body file and a manifest).
    #[test]
    fn missing_manifest_without_marker_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("index.js"), b"x").unwrap();
        assert!(!store_extract_complete(d));
    }
}

#[cfg(test)]
mod prune_keeping_tests {
    use super::{prune_packument_in_place, prune_packument_in_place_keeping};
    use serde_json::json;

    /// `time` is the entire input to `--as-of`. Pruning it away made the resolver
    /// declare "no publish dates" about metadata that had arrived complete — the
    /// document was pruned by the caller itself.
    #[test]
    fn keeping_time_preserves_publish_dates() {
        let mut doc = json!({
            "name": "p",
            "dist-tags": {"latest": "1.0.0"},
            "time": {"1.0.0": "2020-01-01T00:00:00.000Z"},
            "versions": {"1.0.0": {
                "name": "p", "version": "1.0.0", "license": "MIT",
                "dependencies": {"x": "^1"}, "dist": {"tarball": "u"}}},
            "readme": "large, useless here"
        });
        prune_packument_in_place_keeping(&mut doc, &["time"]);
        assert!(doc.get("time").is_some(), "time must survive");
        assert!(doc.get("readme").is_none(), "unrelated keys are still dropped");
        assert!(doc["versions"]["1.0.0"].get("dependencies").is_some());
    }

    #[test]
    fn default_prune_still_drops_time() {
        let mut doc = json!({
            "name": "p",
            "time": {"1.0.0": "2020-01-01T00:00:00.000Z"},
            "versions": {"1.0.0": {"version": "1.0.0"}}
        });
        prune_packument_in_place(&mut doc);
        assert!(doc.get("time").is_none());
    }
}
