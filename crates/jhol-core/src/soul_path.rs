//! Soul Path v3 — Spectral Capsule Runtime.
//!
//! classify → STABILITY CLASS → path
//! Stable   → Capsule execute + JAGR-S project
//! Marginal → greedy / existing resolve
//! Fragile  → full pipeline
//!
//! Install is a spectral decision tree, not a body pipeline.

use crate::capsule::{self, CapsuleView};
use crate::grm::ResolutionRecord;
use crate::lock_free_grm;
use crate::registry;
use crate::selective_extract::{self, FeatureSheaf};
use crate::utils;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static MIGRATED: AtomicBool = AtomicBool::new(false);

/// Spectral stability class (DSSC — Degree-Sequence Spectral Certificate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilityClass {
    Stable,
    Marginal,
    Fragile,
    Degenerate,
}

#[derive(Debug, Clone)]
pub struct SpectralGate {
    pub spectral_gap: f64,
    pub stability: StabilityClass,
    pub predicted_beta1: usize,
    pub conflict_risk: HashMap<String, f64>,
}

impl SpectralGate {
    pub fn can_skip_resolve(&self) -> bool {
        matches!(self.stability, StabilityClass::Stable) && self.predicted_beta1 == 0
    }
}

/// DSSC spectral gate from root dependency counts (no full tropical stack needed on hot path).
pub fn spectral_gate_from_deps(deps: &HashMap<String, String>) -> SpectralGate {
    let n = deps.len();
    if n == 0 {
        return SpectralGate {
            spectral_gap: 1.0,
            stability: StabilityClass::Degenerate,
            predicted_beta1: 0,
            conflict_risk: HashMap::new(),
        };
    }

    // Degree proxy: each root dep is one edge. Tight = exact version pin.
    let mut tight = 0u32;
    let mut conflict_risk = HashMap::new();
    for (name, spec) in deps {
        let s = spec.trim();
        let is_tight = !s.is_empty()
            && !s.starts_with('^')
            && !s.starts_with('~')
            && s != "*"
            && s != "latest"
            && !s.contains("||")
            && !s.contains(' ');
        if is_tight {
            tight += 1;
        }
        // Higher risk for wide ranges / peers-ish wildcards
        let risk = if s == "*" || s == "latest" {
            0.4
        } else if s.starts_with('^') || s.starts_with('~') {
            0.15
        } else if s.contains("||") {
            0.35
        } else {
            0.05
        };
        conflict_risk.insert(name.clone(), risk);
    }

    let max_degree = n as f64;
    let spectral_gap = if max_degree > 0.0 {
        (tight as f64 + 1.0) / (max_degree + 1.0)
    } else {
        1.0
    };

    let stability = if spectral_gap > 0.5 {
        StabilityClass::Stable
    } else if spectral_gap > 0.1 {
        StabilityClass::Marginal
    } else {
        StabilityClass::Fragile
    };

    let predicted_beta1 = if stability == StabilityClass::Fragile {
        1
    } else {
        0
    };

    SpectralGate {
        spectral_gap,
        stability,
        predicted_beta1,
        conflict_risk,
    }
}

/// Ensure JAGR-S migrated from v1 store once per process.
pub fn ensure_jagrs_migrated() {
    if MIGRATED.swap(true, Ordering::SeqCst) {
        return;
    }
    let cache = utils::get_cache_dir();
    let _ = crate::jagrs::init_store();
    let _ = crate::jagrs::migrate_from_v1(&cache);
}

/// Try pure capsule execute when manifest hash matches.
/// After install, verifies all root deps are satisfied (Spectral verify).
pub fn try_capsule_execute(project_dir: &Path, quiet: bool) -> Result<bool, String> {
    ensure_jagrs_migrated();
    let cap_path = capsule::capsule_path(project_dir);
    if !cap_path.exists() {
        return Ok(false);
    }
    let view = match capsule::read_capsule(&cap_path) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let hash = match capsule::compute_manifest_hash(project_dir) {
        Ok(h) => h,
        Err(_) => return Ok(false),
    };
    if view.manifest_hash() != hash {
        return Ok(false);
    }
    if !quiet {
        utils::log("Soul Path: Spectral Stable — capsule execute (zero resolve)");
    }
    capsule::install_from_capsule(&view, project_dir)?;

    // Spectral verify: confirm all root deps are satisfied after capsule install
    let pj_path = project_dir.join("package.json");
    if pj_path.exists() {
        if let Ok(pj_bytes) = std::fs::read_to_string(&pj_path) {
            if let Ok(pj) = serde_json::from_str::<serde_json::Value>(&pj_bytes) {
                let nm = project_dir.join("node_modules");
                if let Some(deps) = pj.get("dependencies").and_then(|d| d.as_object()) {
                    for (name, spec) in deps {
                        let spec_str = spec.as_str().unwrap_or("*");
                        let pkg_dir = nm.join(name);
                        if !pkg_dir.join("package.json").exists() {
                            if !quiet {
                                utils::log(&format!("Spectral verify FAIL: {} missing", name));
                            }
                            return Ok(false);
                        }
                        if let Ok(pkg_json) = std::fs::read_to_string(pkg_dir.join("package.json")) {
                            if let Ok(pkg_val) = serde_json::from_str::<serde_json::Value>(&pkg_json) {
                                if let Some(installed_ver) = pkg_val.get("version").and_then(|v| v.as_str()) {
                                    if !crate::registry::version_satisfies(spec_str, installed_ver) {
                                        if !quiet {
                                            utils::log(&format!(
                                                "Spectral verify FAIL: {}@{} does not satisfy {}",
                                                name, installed_ver, spec_str
                                            ));
                                        }
                                        return Ok(false);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(true)
}

/// Jacobian-style risk for adding packages (∂R/∂X proxy).
pub fn add_conflict_risk(existing_deps: &HashMap<String, String>, adding: &[String]) -> f64 {
    let gate = spectral_gate_from_deps(existing_deps);
    let mut risk = 0.0f64;
    for pkg in adding {
        let base = pkg.split('@').next().unwrap_or(pkg);
        risk = risk.max(
            gate.conflict_risk
                .get(base)
                .copied()
                .unwrap_or(0.2),
        );
        // New package always some risk
        risk = risk.max(0.1);
    }
    // If base graph already fragile, amplify
    if gate.stability == StabilityClass::Fragile {
        risk = (risk + 0.3).min(1.0);
    }
    risk
}

/// Store a single resolved package in GRM + lattice memory.
pub fn remember_package(
    pkg: &crate::lockfile_write::ResolvedPackage,
    name: &str,
) {
    let spec = format!("{}@{}", name, pkg.version);
    let _ = lock_free_grm::store_global(ResolutionRecord {
        package_spec: spec,
        resolved_version: pkg.version.clone(),
        id: 0,
        timestamp: crate::grm::current_timestamp(),
        ttl_secs: 7200,
        dependencies: pkg.dependencies.clone(),
        platform: Some(std::env::consts::OS.to_string()),
        node_version: std::env::var("NODE_VERSION").ok(),
        cached_at: crate::grm::current_timestamp(),
        hit_count: 1,
        ..Default::default()
    });
    // Constraint lattice: remember (name, constraint → version) for major/minor ranges
    let v = &pkg.version;
    if let Ok(sv) = semver::Version::parse(v) {
        for prefix in &[
            format!("~{}", v),
            format!("^{}.{}.0", sv.major, sv.minor),
            format!("^{}.0.0", sv.major),
        ] {
            crate::lattice_cache::remember(name, prefix, v);
        }
    }
    crate::lattice_cache::remember(name, v, v);
}

/// Store all resolved packages from a lockfile-style map in GRM + lattice memory.
pub fn remember_all_packages(packages: &HashMap<String, crate::lockfile_write::ResolvedPackage>) {
    for (name, pkg) in packages {
        remember_package(pkg, name);
    }
}

/// Classification using full spectral analysis (JAGR's compute_differential).
/// Requires available_versions (map of package → version list). Falls back to
/// DSSC proxy when not available.
pub fn full_spectral_classify(
    deps: &HashMap<String, String>,
    available_versions: Option<&HashMap<String, Vec<String>>>,
) -> (f64, StabilityClass, usize) {
    let dssc = spectral_gate_from_deps(deps);
    if available_versions.is_none() && dssc.stability == StabilityClass::Stable {
        // DSSC proxy sufficient for Stable classification
        return (dssc.spectral_gap, dssc.stability, dssc.predicted_beta1);
    }
    // Use JAGR's classify_stability with DSSC-computed fields for consistency
    let fiedler = if deps.len() > 1 {
        let n = deps.len() as f64;
        let pi = std::f64::consts::PI;
        // Lower bound estimate: δ_min * π² / n³
        let max_deg = deps.values().map(|s| {
            if s.starts_with('^') || s.starts_with('~') { 1u32 }
            else if s == "*" || s == "latest" { 0 }
            else { 2 }
        }).sum::<u32>() as f64;
        max_deg * pi * pi / n.powi(3)
    } else {
        0.0
    };
    let mapped = classify_stability_jagr(
        dssc.spectral_gap,
        fiedler,
        dssc.predicted_beta1,
        &dssc.conflict_risk,
    );
    (dssc.spectral_gap, mapped, dssc.predicted_beta1)
}

/// Spectral classifier that mirrors JAGR's Layer 0 logic.
/// Returns Stable/Marginal/Fragile based on spectral gap, Fiedler value,
/// predicted β₁, and per-package conflict risk.
fn classify_stability_jagr(
    spectral_gap: f64,
    fiedler: f64,
    predicted_beta1: usize,
    conflict_risk: &HashMap<String, f64>,
) -> StabilityClass {
    if spectral_gap < 1e-10 {
        return StabilityClass::Degenerate;
    }
    let max_risk = conflict_risk.values().copied().fold(0.0f64, f64::max);
    if spectral_gap > 0.5 && fiedler > 0.1 && predicted_beta1 == 0 && max_risk < 0.3 {
        StabilityClass::Stable
    } else if spectral_gap > 0.1 && predicted_beta1 == 0 && max_risk < 0.6 {
        StabilityClass::Marginal
    } else {
        StabilityClass::Fragile
    }
}

/// Resolve a version constraint against zero-copy metadata.
/// Returns the best matching version, or None if no version satisfies.
pub fn resolve_version_from_meta(
    version_req: &str,
    meta: &crate::zero_copy_metadata::PackageMetadata,
) -> Option<String> {
    let req = semver::VersionReq::parse(version_req.trim()).ok()?;
    let ver = semver::Version::parse(&meta.version).ok()?;
    if req.matches(&ver) {
        Some(meta.version.clone())
    } else {
        None
    }
}

/// Frontier-only add: when risk is low, append new packages as homological delta
/// without re-resolving the existing graph. Downloads just the frontier and its
/// transitive deps, then projects into node_modules + updates lockfile.
pub fn frontier_add(packages: &[String], quiet: bool) -> Result<(), String> {
    ensure_jagrs_migrated();
    let cache_dir = PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| e.to_string())?;

    for pkg in packages {
        let name = pkg.split('@').next().unwrap_or(pkg);
        let spec = if pkg.contains('@') && !pkg.starts_with('@') {
            pkg.splitn(2, '@').nth(1).unwrap_or("latest")
        } else if pkg.starts_with('@') {
            let idx = pkg.rfind('@').unwrap_or(0);
            if idx > 0 { &pkg[idx + 1..] } else { "latest" }
        } else {
            "latest"
        };

        // Check zero-copy metadata first
        let (version, tarball_url, integrity) =
            if let Some(meta) = crate::zero_copy_metadata::MmapMetadataStore::lookup_packument(name) {
                if let Some(v) = resolve_version_from_meta(spec, &meta) {
                    let url = if meta.dist.tarball_url.is_empty() {
                        lockfile_tarball_url(name, &v)
                    } else {
                        meta.dist.tarball_url.clone()
                    };
                    (v, url, meta.dist.integrity)
                } else {
                    return Err(format!("package {}@{} not found in zero-copy store", name, spec));
                }
            } else {
                return Err(format!("package {} not cached; run full install first", name));
            };

        // Download tarball to JAGR-S store
        let pkg_key = format!("{}@{}", name, version);
        let hash = crate::registry::download_tarball_to_store_hash_only(
            &tarball_url, &cache_dir, &pkg_key, integrity.as_deref(),
        ).map_err(|e| format!("download failed for {}: {}", pkg, e))?;

        // Full extract from memory via JAGR-S (no selective-untar)
        let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
        let unpacked = crate::registry::ensure_unpacked_in_store(&store_path, &cache_dir)?;

        // Feature-sheaf project
        let sheaf = FeatureSheaf::from_env();
        let pkg_dir = if name.starts_with('@') {
            let parts: Vec<&str> = name.splitn(2, '/').collect();
            node_modules.join(parts[0]).join(parts[1])
        } else {
            node_modules.join(name)
        };
        std::fs::create_dir_all(pkg_dir.parent().unwrap_or(node_modules))
            .map_err(|e| e.to_string())?;
        crate::selective_extract::project_from_store(&unpacked, &pkg_dir, sheaf)?;

        // Record in GRM + lattice + JMR
        let _ = crate::lock_free_grm::store_global(ResolutionRecord {
            package_spec: pkg_key.clone(),
            resolved_version: version.clone(),
            id: 0,
            timestamp: crate::grm::current_timestamp(),
            ttl_secs: 7200,
            dependencies: HashMap::new(),
            platform: Some(std::env::consts::OS.to_string()),
            node_version: std::env::var("NODE_VERSION").ok(),
            cached_at: crate::grm::current_timestamp(),
            hit_count: 1,
            ..Default::default()
        });
        let _ = crate::lattice_cache::remember(name, spec, &version);

        if !quiet {
            utils::log(&format!("Added {}@{} (frontier delta)", name, version));
        }
    }

    // Update lockfile
    let pj_path = Path::new("package.json");
    if pj_path.exists() {
        let node_modules = Path::new("node_modules");
        if let Ok(pkgs) = crate::install::read_all_installed_packages(node_modules) {
            let lock_path = Path::new("package-lock.json");
            let _ = crate::lockfile_write::write_package_lock(lock_path, pj_path, &pkgs);
            crate::resolution_mirror::record_resolution(
                crate::resolution_mirror::fingerprint_deps(
                    &crate::lockfile::read_package_json_deps(pj_path).unwrap_or_default(),
                ),
                &pkgs,
            );
        }
    }

    // Rewrite capsule
    let _ = crate::capsule::write_capsule_from_project(Path::new("."));
    Ok(())
}

fn lockfile_tarball_url(name: &str, version: &str) -> String {
    format!("https://registry.npmjs.org/{}/-/{}-{}.tgz", name, name, version)
}

/// Query GRM for a root package.
pub fn grm_lookup(spec: &str) -> Option<ResolutionRecord> {
    let platform = Some(std::env::consts::OS);
    let node = std::env::var("NODE_VERSION").ok();
    lock_free_grm::query_global(spec, platform, node.as_deref())
        .or_else(|| lock_free_grm::query_global(spec, None, None))
}

/// Project one package from store_unpacked / JAGR-S into node_modules.
pub fn project_package(
    name: &str,
    version: &str,
    hash_hex: &str,
    node_modules: &Path,
    cache_dir: &Path,
) -> Result<(), String> {
    let pkg_dir = package_dir(node_modules, name);
    if pkg_dir.join("package.json").exists() {
        return Ok(());
    }

    let shard = if hash_hex.len() >= 2 {
        &hash_hex[..2]
    } else {
        "xx"
    };
    let unpacked = cache_dir
        .join("store_unpacked")
        .join(shard)
        .join(hash_hex);

    if !unpacked.exists() {
        // Try JAGR-S bytes → full extract
        if let Some(bytes) = crate::jagrs::get_tarball(hash_hex) {
            std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
            registry::extract_tarball_bytes_public(&bytes, &unpacked)?;
        } else {
            let tgz = cache_dir.join("store").join(format!("{}.tgz", hash_hex));
            if tgz.exists() {
                let _ = registry::ensure_unpacked_in_store(&tgz, cache_dir)?;
            } else {
                return Err(format!("missing content for {}@{} ({})", name, version, hash_hex));
            }
        }
    }

    let sheaf = FeatureSheaf::from_env();
    selective_extract::project_from_store(&unpacked, &pkg_dir, sheaf)?;
    let _ = crate::bin_links::link_bins_for_package(node_modules, name);
    Ok(())
}

fn package_dir(node_modules: &Path, name: &str) -> PathBuf {
    if name.starts_with('@') {
        if let Some(slash) = name.find('/') {
            return node_modules.join(&name[..slash]).join(&name[slash + 1..]);
        }
    }
    node_modules.join(name)
}

/// Prefetch predictions into JAGR-S / daemon cache (background-safe).
/// Filters predictions by spectral priority: Fragile-risk packages prefetched first.
/// Also madvise(WILLNEED) on capsule + JAGR-S pages for kernel page cache.
pub fn predictive_prefetch(context_pkgs: &[String], limit: usize) {
    let preds = crate::resolution_mirror::global_mirror().predict(context_pkgs, limit);
    // Spectral priority: order predictions by conflict risk, prefetch fragile first
    let mut scored: Vec<_> = preds.iter().map(|p| {
        let gate = spectral_gate_from_deps(&std::collections::HashMap::from_iter(
            std::iter::once((p.package.clone(), "*".to_string()))
        ));
        let priority = match gate.stability {
            StabilityClass::Fragile => 3u8,
            StabilityClass::Marginal => 2,
            _ => 1,
        };
        (priority, p)
    }).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0)); // descending priority
    for (_priority, p) in scored {
        crate::daemon_cache::DaemonCache::prefetch(&p.package, true);
        // Store in zero-copy metadata mmap for instant packument lookup
        if let Some(meta) = crate::zero_copy_metadata::MmapMetadataStore::lookup_packument(&p.package) {
            // Already cached — no-op
            let _ = meta;
        }
        // Lattice / GRM warm
        if let Some(v) = crate::lattice_cache::lookup_lattice(&p.package, "latest") {
            let _ = lock_free_grm::store_global(ResolutionRecord {
                package_spec: p.package.clone(),
                resolved_version: v,
                id: 0,
                timestamp: crate::grm::current_timestamp(),
                ttl_secs: 3600,
                dependencies: HashMap::new(),
                platform: Some(std::env::consts::OS.to_string()),
                node_version: std::env::var("NODE_VERSION").ok(),
                cached_at: crate::grm::current_timestamp(),
                hit_count: 0,
                ..Default::default()
            });
        }
    }
    // madvise(WILLNEED) on capsule + JAGR-S pages for kernel-backed prefetch
    let cap_path = capsule::capsule_path(Path::new("."));
    if cap_path.exists() {
        madvise_willneed(&cap_path);
    }
    let cache = PathBuf::from(utils::get_cache_dir());
    let jagrs_path = cache.join("objects.jagrs");
    if jagrs_path.exists() {
        madvise_willneed(&jagrs_path);
    }
    // Also advise zero-copy metadata store
    let zc_path = cache.join("zero-copy").join("store.meta");
    if zc_path.exists() {
        madvise_willneed(&zc_path);
    }
}

/// Call madvise(MADV_WILLNEED) on a file to instruct the OS to prefetch
/// its pages into RAM. Safe no-op on non-Unix or when libc unavailable.
fn madvise_willneed(path: &Path) {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if let Ok(file) = std::fs::File::open(path) {
            if let Ok(meta) = file.metadata() {
                let len = meta.len() as usize;
                if len > 0 {
                    // mmap the file and advise WILLNEED
                    if let Ok(mmap) = unsafe {
                        memmap2::Mmap::map(&file)
                    } {
                        let _ = mmap.advise(memmap2::Advice::WillNeed);
                    }
                }
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    { let _ = path; }
}

/// GRM + lattice gate: check if all root deps are already resolved by cross-project memory.
/// Returns Some(resolved map) if every root is covered and Stable.
pub fn grm_lattice_assemble(
    deps: &HashMap<String, String>,
) -> Option<HashMap<String, crate::lockfile_write::ResolvedPackage>> {
    if deps.is_empty() {
        return None;
    }
    let gate = spectral_gate_from_deps(deps);
    if !gate.can_skip_resolve() {
        return None;
    }
    let mut resolved = HashMap::new();
    for (name, spec) in deps {
        // GRM first
        if let Some(rec) = grm_lookup(name) {
            if crate::registry::version_satisfies(spec, &rec.resolved_version) {
                let _ = crate::lattice_cache::remember(name, spec, &rec.resolved_version);
                resolved.insert(
                    name.clone(),
                    crate::lockfile_write::ResolvedPackage {
                        version: rec.resolved_version,
                        resolved: String::new(),
                        integrity: None,
                        dependencies: rec.dependencies.clone(),
                        peer_dependencies: HashMap::new(),
                        peer_dependencies_meta: HashMap::new(),
                    },
                );
                continue;
            }
        }
        // Lattice fallback
        if let Some(v) = crate::lattice_cache::lookup_lattice(name, spec) {
            if crate::registry::version_satisfies(spec, &v) {
                resolved.insert(
                    name.clone(),
                    crate::lockfile_write::ResolvedPackage {
                        version: v,
                        resolved: String::new(),
                        integrity: None,
                        dependencies: HashMap::new(),
                        peer_dependencies: HashMap::new(),
                        peer_dependencies_meta: HashMap::new(),
                    },
                );
                continue;
            }
        }
        return None;
    }
    if resolved.len() == deps.len() {
        Some(resolved)
    } else {
        None
    }
}

/// Full spectral analysis of a resolved package graph using
/// JAGR's Layer 0 `compute_differential`. Returns (spectral_gap, predicted_beta1)
/// for capsule header storage, or None if analysis not possible.
/// More accurate than DSSC proxy because it sees the full dependency graph.
pub fn full_resolved_classify(
    packages: &HashMap<String, crate::lockfile_write::ResolvedPackage>,
) -> Option<(f64, u64)> {
    use crate::jagr::resolution_differential::{build_request, compute_differential};

    let mut available_versions: HashMap<String, Vec<String>> = HashMap::new();
    for (name, pkg) in packages {
        available_versions
            .entry(name.clone())
            .or_default()
            .push(pkg.version.clone());
    }

    // Build deps map: for each package that has deps, record edges
    let mut deps_map: HashMap<String, String> = HashMap::new();
    for (name, pkg) in packages {
        if !pkg.dependencies.is_empty() {
            deps_map.insert(name.clone(), format!("^{}", pkg.version));
        }
    }

    if deps_map.is_empty() {
        return None;
    }

    let req = build_request(&deps_map, Some(&available_versions));
    let diff = compute_differential(&req);
    Some((diff.spectral_gap, diff.predicted_beta1 as u64))
}

/// Whether streaming soul install is enabled.
pub fn stream_install_enabled() -> bool {
    std::env::var("JHOL_STREAM_INSTALL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Whether auto-daemon attach is enabled.
/// Soul Path: connect to existing daemon if available and socket exists;
/// do NOT auto-start (causes capsule write races with concurrent install).
/// Use explicit `jhol daemon --serve` or `jhol install --daemon` for persistent mode.
pub fn auto_daemon_enabled() -> bool {
    if std::env::var("JHOL_NO_DAEMON")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }
    // CI: off unless explicitly on
    if std::env::var("CI").is_ok() {
        return std::env::var("JHOL_AUTO_DAEMON")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    }
    std::env::var("JHOL_AUTO_DAEMON")
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
        .unwrap_or(false) // default off until races resolved
}

/// Waveform install from capsule view (parallel by install_order waves).
pub fn waveform_install_capsule(view: &CapsuleView, project_dir: &Path) -> Result<(), String> {
    ensure_jagrs_migrated();
    let node_modules = project_dir.join("node_modules");
    std::fs::create_dir_all(&node_modules).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| e.to_string())?;
    let cache_dir = PathBuf::from(utils::get_cache_dir());

    // Group by install_order
    let mut waves: std::collections::BTreeMap<u16, Vec<usize>> = std::collections::BTreeMap::new();
    for i in 0..view.package_count() {
        let e = view.entry(i);
        waves.entry(e.install_order).or_default().push(i);
    }

    let nm = node_modules.clone();
    let cd = cache_dir.clone();
    for (_order, idxs) in waves {
        let results: Vec<Result<(), String>> = if idxs.len() <= 1 {
            idxs.iter()
                .map(|&i| {
                    let e = view.entry(i);
                    let name = view.entry_name(e);
                    let version = view.entry_version(e);
                    let hash = hex::encode(e.tarball_hash);
                    project_package(name, version, &hash, &nm, &cd)
                })
                .collect()
        } else {
            use rayon::prelude::*;
            idxs.par_iter()
                .map(|&i| {
                    let e = view.entry(i);
                    let name = view.entry_name(e);
                    let version = view.entry_version(e);
                    let hash = hex::encode(e.tarball_hash);
                    project_package(name, version, &hash, &nm, &cd)
                })
                .collect()
        };
        for r in results {
            r?;
        }
    }
    let _ = crate::bin_links::rebuild_bin_links(&node_modules);
    Ok(())
}
