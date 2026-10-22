//! Native lockfile writing: resolve dependency tree and emit package-lock.json.
//! Optimized for performance with incremental updates and async I/O.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::jagr::jagr_resolver::{JagrResolver, ResolveRequest};
use crate::jagr::maxsat_solver::PackageVersionVar;
pub use crate::jagr_infinite::resolve_full_tree_jagr_infinite;
use crate::lockfile;
use crate::pubgrub::{can_use_minimal_selection, resolve_minimal, PackedVersion, PubGrubSolver};
use crate::registry;
use crate::sat_resolver::{PackageDomain, PackageVersion, SolveInput}; // JAGR-2: PubGrub solver

/// RAII-safe mutex lock helper (DRY principle)
macro_rules! mutex_lock {
    ($lock:expr) => {{
        match $lock.lock() {
            Ok(guard) => guard,
            Err(_) => panic!("Mutex lock poisoned - thread panicked while holding lock"),
        }
    }};
}

/// One resolved package entry for the lockfile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub version: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub peer_dependencies: HashMap<String, String>,
    pub peer_dependencies_meta: HashMap<String, serde_json::Value>,
}

#[derive(Clone, Debug)]
struct PackageSnapshot {
    version: String,
    resolved: String,
    integrity: Option<String>,
    dependencies: HashMap<String, String>,
    peer_dependencies: HashMap<String, String>,
    peer_dependencies_meta: HashMap<String, serde_json::Value>,
}

struct RegistryProvider {
    metadata_cache: HashMap<String, serde_json::Value>,
    parsed_versions: HashMap<String, Vec<(semver::Version, String)>>,
}

impl RegistryProvider {
    fn new() -> Self {
        Self {
            metadata_cache: HashMap::new(),
            parsed_versions: HashMap::new(),
        }
    }

    fn metadata(&mut self, name: &str) -> Result<&serde_json::Value, String> {
        if !self.metadata_cache.contains_key(name) {
            let meta = registry::fetch_metadata(name)
                .map_err(|e| format!("Failed to fetch metadata for {}: {}", name, e))?;
            self.metadata_cache.insert(name.to_string(), meta);
        }
        self.metadata_cache
            .get(name)
            .ok_or_else(|| format!("Missing metadata for {}", name))
    }

    fn get_parsed_versions(&mut self, name: &str) -> Result<&[(semver::Version, String)], String> {
        if !self.parsed_versions.contains_key(name) {
            let meta = self.metadata(name)?;
            let versions = meta
                .get("versions")
                .and_then(|v| v.as_object())
                .ok_or_else(|| format!("No versions for {}", name))?;
            let mut list: Vec<(semver::Version, String)> = versions
                .keys()
                .filter_map(|v| Some((semver::Version::parse(v).ok()?, v.clone())))
                .collect();
            list.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            self.parsed_versions.insert(name.to_string(), list);
        }
        Ok(self
            .parsed_versions
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]))
    }

    fn resolve_version(&mut self, name: &str, specs: &[String]) -> Result<String, String> {
        let parsed = self.get_parsed_versions(name)?;
        resolve_highest_satisfying_parsed(parsed, specs).ok_or_else(|| {
            format!(
                "Dependency conflict for {} (specs: {})",
                name,
                specs.join(", ")
            )
        })
    }

    fn snapshot(&mut self, name: &str, version: &str) -> Result<PackageSnapshot, String> {
        let meta = self.metadata(name)?;
        let (resolved_url, integrity) = match registry::resolve_tarball_via_manifest(name, version)
        {
            Ok(Some((_, url, integrity_opt))) => (url, integrity_opt),
            Ok(None) | Err(_) => (
                crate::lockfile::tarball_url_from_registry(name, version),
                registry::get_integrity_for_version(meta, version),
            ),
        };

        Ok(PackageSnapshot {
            version: version.to_string(),
            resolved: resolved_url,
            integrity,
            dependencies: registry::get_version_required_dependencies(meta, version),
            peer_dependencies: registry::get_version_peer_dependencies(meta, version),
            peer_dependencies_meta: registry::get_version_peer_dependencies_meta(meta, version),
        })
    }
}

#[derive(Clone, Debug)]
struct RequirementEdge {
    requester: String,
    package: String,
    spec: String,
    optional_peer: bool,
}

#[derive(Clone, Debug)]
struct PeerRequirement {
    requester: String,
    spec: String,
    optional: bool,
}

#[derive(Clone, Debug)]
struct Requirement {
    requester: String,
    spec: String,
}

/// Validate direct/root peer dependency conflicts from package.json specs.
pub fn validate_root_peer_conflicts(package_json_path: &Path) -> Result<(), String> {
    let deps = lockfile::registry_only_deps(package_json_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(());
    }

    // Fast path: a committed lockfile whose direct ranges still hold already
    // had its peer edges validated at write time, so the redundant metadata
    // re-fetch below is skipped. Stale or missing lockfiles fall through.
    let lockfile_path = package_json_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("package-lock.json");
    if lockfile_path.exists() {
        let stale = std::fs::read_to_string(&lockfile_path)
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .and_then(|v| v.get("packages").cloned())
            .and_then(|p| p.as_object().cloned())
            .map(|packages| {
                deps.iter().any(|(name, spec)| {
                    packages
                        .get(format!("node_modules/{}", name).as_str())
                        .and_then(|info| info.get("version"))
                        .and_then(|v| v.as_str())
                        .map(|ver| !crate::registry::version_satisfies(spec, ver))
                        .unwrap_or(true)
                })
            })
            .unwrap_or(true);
        if !stale {
            return Ok(());
        }
    }

    let mut selected: HashMap<String, String> = HashMap::new();
    let mut metadata_cache: HashMap<String, serde_json::Value> = HashMap::new();

    // Prefetch all metadata in parallel (R5)
    let dep_names: Vec<String> = deps.keys().cloned().collect();
    let fetch_results = registry::fetch_metadata_parallel(&dep_names);
    for (name, result) in fetch_results {
        let meta = result.map_err(|e| format!("Failed to fetch metadata for {}: {}", name, e))?;
        let spec = deps
            .get(&name)
            .ok_or_else(|| format!("Missing spec for {}", name))?;
        // If the fetched metadata is a partial synth (bundled or manifest) that
        // cannot satisfy this package's spec, escalate to the full network
        // packument so ranges like ^18.3.1 resolve instead of erroring.
        let meta = if resolve_highest_satisfying(&meta, std::slice::from_ref(spec)).is_none()
            && (meta
                .get("_bundled_index_synth")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || meta
                    .get("_vortex_manifest_synth")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false))
        {
            registry::fetch_metadata_full_packument(&name)
                .map_err(|e| format!("Failed to fetch metadata for {}: {}", name, e))?
        } else {
            meta
        };
        let version = resolve_highest_satisfying(&meta, std::slice::from_ref(spec))
            .ok_or_else(|| format!("Could not resolve {} with spec {}", name, spec))?;
        selected.insert(name.clone(), version);
        metadata_cache.insert(name.clone(), meta);
    }

    let mut conflicts = Vec::new();
    for (name, version) in &selected {
        let Some(meta) = metadata_cache.get(name) else {
            continue;
        };
        let peers = registry::get_version_peer_dependencies(meta, version);
        let peer_meta = registry::get_version_peer_dependencies_meta(meta, version);

        for (peer_name, peer_spec) in peers {
            let optional = peer_meta
                .get(&peer_name)
                .and_then(|v| v.get("optional"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match selected.get(&peer_name) {
                Some(peer_version) if registry::version_satisfies(&peer_spec, peer_version) => {}
                Some(peer_version) => conflicts.push(format!(
                    "peer {} required by {} but resolved {} (spec {})",
                    peer_name, name, peer_version, peer_spec
                )),
                None if !optional => conflicts.push(format!(
                    "peer {} missing (required by {} spec {})",
                    peer_name, name, peer_spec
                )),
                None => {}
            }
        }
    }

    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(format!("Dependency conflict: {}", conflicts.join("; ")))
    }
}

/// Resolve the full dependency tree from package.json with deterministic conflict handling.
///
/// JAGR-infinite is the single resolver. The `--resolver` CLI flag and the
/// `JHOL_RESOLVER` / `JHOL_RESOLVER_ROLLOUT` / `JHOL_RESOLVER_CANARY` env
/// vars are accepted for backward compatibility but all map to the same
/// JAGR-infinite path. JAGR-infinite combines:
///   - Lockfile fast path (P2): committed lockfile that still satisfies every
///     direct range → 0 fetch.
///   - Resolved-bundle cache (P1): exact same dep-set resolved before → 0
///     fetch, cross-project replay.
///   - Tropical-geometry atlas: maps the full dep graph into a piecewise-
///     linear constraint space where each package's versions form a
///     polyhedron.
///   - Homotopy continuation from the lockfile: walks from the previous
///     selection to the new one along a continuous path; if the path hits
///     a singularity (a fundamental conflict change), falls back to the
///     RVM bytecode path.
///   - RVM bytecode execution: compiles the constraint set into a
///     deterministic register program and runs it against the atlas.
///   - Program cache (P3): reuses compiled programs for the same chart
///     signature across runs.
///   - JAGR-3 consistency gate: every selected version is verified to exist
///     in its packument; sheaf H¹ check available via JHOL_VALIDATE_SHEAF=1.
///
/// This is the only resolver. We don't expose PubGrub, legacy SAT, or the
/// older JAGR-1/JAGR-3 as user-selectable strategies — they were subsumed
/// by JAGR-infinite's homotopy + sheaf path.
pub fn resolve_full_tree(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let _strict_resolver = std::env::var("JHOL_RESOLVER_STRICT")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "True"))
        .unwrap_or(false);

    // Honor the legacy `--resolver` / `JHOL_RESOLVER` flag for backward
    // compatibility, but route everything to JAGR-infinite. If someone
    // explicitly asks for "legacy" or "pubgrub" we still run JAGR-infinite
    // and log a deprecation note so the user knows the option is ignored.
    let requested = std::env::var("JHOL_RESOLVER").unwrap_or_else(|_| "jagr-infinite".to_string());

    if !requested.is_empty() && requested != "jagr-infinite" && requested != "jagr" {
        // Only log once per process to avoid noise; use a static guard.
        use std::sync::atomic::{AtomicBool, Ordering};
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "[JAGR] note: --resolver={} is no longer a distinct path; \
                 JAGR-infinite is the only resolver and handles all cases.",
                requested
            );
        }
    }

    match resolve_full_tree_jagr_infinite(package_json_path) {
        Ok(tree) => {
            crate::jagr_infinite::clear_synth_served();
            Ok(tree)
        }
        Err(first_err) => {
            // Candidate-domain starvation: single-version bundled synths
            // advertise ONE candidate per package. If resolution failed while
            // any synth served, backtracking may have been starved rather than
            // the graph being genuinely unsatisfiable — retry ONCE with full
            // packuments for exactly those packages (same constraints, bigger
            // domains). If the retry fails too, the ORIGINAL error stands.
            if crate::jagr_infinite::take_synth_served() {
                std::env::set_var("JHOL_JINF_FULL_RETRY", "1");
                let retry = resolve_full_tree_jagr_infinite(package_json_path);
                std::env::remove_var("JHOL_JINF_FULL_RETRY");
                crate::jagr_infinite::clear_synth_served();
                match retry {
                    Ok(tree) => Ok(tree),
                    Err(_) => Err(first_err),
                }
            } else {
                Err(first_err)
            }
        }
    }
}

/// Decide what to do after the primary resolver (JAGR-infinite) fails.
///
/// `pubgrub` is injected rather than called directly so the decision can be tested
/// without forcing JAGR to fail and without touching the network. The production
/// caller passes [`resolve_full_tree_pubgrub_v2`].
///
/// Contract: PubGrub is a *fallback*, never the primary path. It only wins when it
/// actually resolved something — an empty tree is not a resolution, and if both
/// solvers fail the caller must see JAGR's error (the primary's) rather than a
/// secondary one.
fn with_pubgrub_fallback<S>(
    jagr_err: String,
    package_json_path: &Path,
    pubgrub: S,
) -> Result<HashMap<String, ResolvedPackage>, String>
where
    S: FnOnce(&Path) -> Result<HashMap<String, ResolvedPackage>, String>,
{
    // Guard: `resolve_full_tree_pubgrub_v2` is a queue-based provider loop that
    // fetches packuments per candidate version, so its cost grows with the manifest
    // and it is NOT covered by the solver's decision budget (it does not use
    // `pubgrub::Solver`). On large manifests that is minutes of network work — a
    // fallback must never hang an install that would otherwise fail fast. It is
    // therefore only attempted for small manifests, where it is quick and has a real
    // chance of rescuing the resolution.
    const MAX_FALLBACK_DEPS: usize = 25;
    let direct_deps = crate::lockfile::registry_only_deps(package_json_path)
        .map(|d| d.len())
        .unwrap_or(usize::MAX);
    if direct_deps > MAX_FALLBACK_DEPS {
        crate::utils::log(&format!(
            "warning: JAGR-infinite failed ({}); PubGrub fallback skipped for a {}-dependency manifest (the fallback is only bounded for small ones)",
            jagr_err, direct_deps
        ));
        return Err(jagr_err);
    }

    match pubgrub(package_json_path) {
        Ok(tree) if !tree.is_empty() => {
            crate::utils::record_fallback_reason("resolver_pubgrub_fallback", &[]);
            crate::utils::log(&format!(
                "warning: JAGR-infinite failed ({}); resolved with the PubGrub fallback ({} packages)",
                jagr_err,
                tree.len()
            ));
            Ok(tree)
        }
        Ok(_) => {
            crate::utils::log(&format!(
                "warning: JAGR-infinite failed ({}) and the PubGrub fallback produced no packages",
                jagr_err
            ));
            Err(jagr_err)
        }
        Err(pg_err) => {
            crate::utils::log(&format!(
                "warning: resolution failed with both resolvers — JAGR-infinite: {} | PubGrub: {}",
                jagr_err, pg_err
            ));
            Err(jagr_err)
        }
    }
}

pub fn resolve_full_tree_pubgrub_v2(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let deps = lockfile::registry_only_deps(package_json_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(HashMap::new());
    }

    let mut provider = RegistryProvider::new();
    let mut selected: HashMap<String, PackageSnapshot> = HashMap::new();
    let mut requirements: HashMap<String, Vec<Requirement>> = HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    for (name, spec) in deps {
        queue.push_back(RequirementEdge {
            requester: "root".to_string(),
            package: name,
            spec,
            optional_peer: false,
        });
    }

    let mut conflicts: Vec<String> = Vec::new();

    while let Some(edge) = queue.pop_front() {
        if edge.optional_peer && !selected.contains_key(&edge.package) {
            continue;
        }

        requirements
            .entry(edge.package.clone())
            .or_default()
            .push(Requirement {
                requester: edge.requester.clone(),
                spec: edge.spec.clone(),
            });

        let specs = requirements
            .get(&edge.package)
            .map(|reqs| reqs.iter().map(|r| r.spec.clone()).collect::<Vec<_>>())
            .unwrap_or_default();

        if let Some(existing) = selected.get(&edge.package) {
            if !registry::version_satisfies(&edge.spec, &existing.version) {
                conflicts.push(format!(
                    "{} requires {} {}, resolved {}",
                    edge.requester, edge.package, edge.spec, existing.version
                ));
            }
            continue;
        }

        let version = match provider.resolve_version(&edge.package, &specs) {
            Ok(v) => v,
            Err(e) => {
                conflicts.push(e);
                continue;
            }
        };

        let snapshot = match provider.snapshot(&edge.package, &version) {
            Ok(s) => s,
            Err(e) => {
                conflicts.push(e);
                continue;
            }
        };

        for (dep_name, dep_spec) in &snapshot.dependencies {
            queue.push_back(RequirementEdge {
                requester: edge.package.clone(),
                package: dep_name.clone(),
                spec: dep_spec.clone(),
                optional_peer: false,
            });
        }

        for (peer_name, peer_spec) in &snapshot.peer_dependencies {
            let optional = snapshot
                .peer_dependencies_meta
                .get(peer_name)
                .and_then(|v| v.get("optional"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !optional || selected.contains_key(peer_name) {
                queue.push_back(RequirementEdge {
                    requester: edge.package.clone(),
                    package: peer_name.clone(),
                    spec: peer_spec.clone(),
                    optional_peer: optional,
                });
            }
        }

        selected.insert(edge.package, snapshot);
    }

    if !conflicts.is_empty() {
        return Err(format!("Dependency conflict: {}", conflicts.join("; ")));
    }

    let mut tree = HashMap::new();
    for (name, snapshot) in &selected {
        let resolved_deps = snapshot
            .dependencies
            .iter()
            .map(|(dep, spec)| {
                let dep_version = selected
                    .get(dep)
                    .map(|p| p.version.clone())
                    .unwrap_or_else(|| spec.clone());
                (dep.clone(), dep_version)
            })
            .collect::<HashMap<_, _>>();

        tree.insert(
            name.clone(),
            ResolvedPackage {
                version: snapshot.version.clone(),
                resolved: snapshot.resolved.clone(),
                integrity: snapshot.integrity.clone(),
                dependencies: resolved_deps,
                peer_dependencies: snapshot.peer_dependencies.clone(),
                peer_dependencies_meta: snapshot.peer_dependencies_meta.clone(),
            },
        );
    }

    validate_peer_conflicts_in_tree(&tree)?;
    Ok(tree)
}

/// Resolve using PubGrub algorithm (JAGR-2)
/// Fast, conflict-driven resolution with excellent error messages
fn resolve_full_tree_pubgrub(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let deps = lockfile::registry_only_deps(package_json_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(HashMap::new());
    }

    // === JAGR-3 FAST PATH #1: Minimal Version Selection ===
    // O(n) resolution for 95% of real-world dependency graphs
    if can_use_minimal_selection(&deps) {
        if let Ok(min_solution) = resolve_minimal(&deps) {
            // Verify solution works by checking transitive dependencies
            if verify_minimal_solution(&min_solution, package_json_path).is_ok() {
                return build_tree_from_versions(&min_solution);
            }
            // If verification fails, fall through to full PubGrub
        }
    }
    // === End JAGR-3 fast path ===

    // FAST PATH #2: For simple dependency graphs (<10 deps), use simplified resolution
    if deps.len() <= 10 {
        if let Ok(result) = resolve_simple_fast_path(package_json_path, &deps) {
            return Ok(result);
        }
        // Fall through to full PubGrub if fast path fails
    }

    // Create PubGrub solver
    let mut solver = PubGrubSolver::new("root".to_string());

    // Add root requirements
    let specs: HashMap<String, String> = deps.into_iter().collect();
    let package_names: Vec<String> = specs.keys().cloned().collect();

    // OPTIMIZATION: Check bundled index FIRST for instant resolution
    let mut metadata_cache = HashMap::new();
    {
        if let Ok(index) = crate::registry::PACKAGE_INDEX.lock() {
            for name in &package_names {
                if let Some(pkg) = index.get_package(name) {
                    // Instant: use bundled metadata directly
                    let meta = serde_json::json!({
                        "name": pkg.name,
                        "dist": {
                            "tarball": pkg.tarball_url,
                            "integrity": pkg.integrity
                        },
                        "versions": pkg.versions.iter()
                            .map(|v| (v.clone(), serde_json::json!({
                                "version": v.clone(),
                                "dependencies": pkg.dependencies
                            })))
                            .collect::<serde_json::Map<String, _>>()
                    });
                    metadata_cache.insert(name.clone(), meta);
                }
            }
        }
    }

    // Fetch ONLY missing packages from network
    let missing: Vec<String> = package_names
        .iter()
        .filter(|n| !metadata_cache.contains_key(*n))
        .cloned()
        .collect();

    if !missing.is_empty() {
        let network_results = registry::fetch_metadata_parallel(&missing);
        for (name, result) in network_results {
            if let Ok(metadata) = result {
                metadata_cache.insert(name, metadata);
            }
        }
    }

    solver
        .add_root_requirements_from_specs(specs.clone())
        .map_err(|e| format!("PubGrub: Failed to add requirements: {}", e))?;

    // Set available versions from cached metadata
    for name in specs.keys() {
        if let Some(metadata) = metadata_cache.get(name) {
            if let Some(versions) = metadata.get("versions") {
                if let Some(versions_obj) = versions.as_object() {
                    let version_strings: Vec<String> = versions_obj.keys().cloned().collect();
                    solver.set_available_versions_from_strings(name, version_strings);
                }
            }
        }
    }

    // Solve
    let solution = solver
        .solve()
        .map_err(|e| format!("PubGrub resolution failed: {:?}", e))?;

    validate_direct_peer_conflicts(&solution, &metadata_cache)?;

    // Convert solution to resolved packages
    build_tree_from_pubgrub_solution(&solution, &specs, &metadata_cache)
}

/// Fast path resolution for simple dependency graphs (<10 deps)
/// Uses direct resolution without full PubGrub overhead
fn resolve_simple_fast_path(
    _package_json_path: &Path,
    deps: &HashMap<String, String>,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let package_names: Vec<String> = deps.keys().cloned().collect();

    // Fetch all metadata in parallel
    let metadata_results = registry::fetch_metadata_parallel(&package_names);

    let mut result = HashMap::new();

    for (name, metadata_result) in metadata_results {
        let spec = deps
            .get(&name)
            .ok_or(format!("Missing spec for {}", name))?;

        match metadata_result {
            Ok(metadata) => {
                // Pre-parse spec to VersionReq once per package (R6)
                let req = if spec.trim().is_empty() || spec.trim() == "*" {
                    None
                } else {
                    semver::VersionReq::parse(spec.trim()).ok()
                };

                // Find best version matching spec
                if let Some(versions) = metadata.get("versions") {
                    if let Some(versions_obj) = versions.as_object() {
                        let mut best_version: Option<String> = None;
                        let mut best_semver: Option<semver::Version> = None;

                        for version_str in versions_obj.keys() {
                            if let Ok(version) = semver::Version::parse(version_str) {
                                let matches = match &req {
                                    None => true,
                                    Some(r) => r.matches(&version),
                                };
                                if matches {
                                    let should_update = match &best_semver {
                                        None => true,
                                        Some(current) => version > *current,
                                    };

                                    if should_update {
                                        best_semver = Some(version);
                                        best_version = Some(version_str.clone());
                                    }
                                }
                            }
                        }

                        if let Some(version) = best_version {
                            // Get tarball info
                            if let Some(version_data) = versions_obj.get(&version) {
                                let resolved = version_data
                                    .get("dist")
                                    .and_then(|d| d.get("tarball"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                let integrity = version_data
                                    .get("dist")
                                    .and_then(|d| d.get("integrity"))
                                    .and_then(|i| i.as_str())
                                    .map(String::from);

                                result.insert(
                                    name,
                                    ResolvedPackage {
                                        version,
                                        resolved,
                                        integrity,
                                        dependencies: HashMap::new(),
                                        peer_dependencies: HashMap::new(),
                                        peer_dependencies_meta: HashMap::new(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                return Err(format!("Failed to fetch metadata for {}: {}", name, e));
            }
        }
    }

    if result.len() == deps.len() {
        validate_peer_conflicts_in_tree(&result)?;
        Ok(result)
    } else {
        Err("Fast path resolution incomplete".to_string())
    }
}

fn validate_peer_conflicts_in_tree(tree: &HashMap<String, ResolvedPackage>) -> Result<(), String> {
    if tree.is_empty() {
        return Ok(());
    }

    let resolved_versions: HashMap<String, String> = tree
        .iter()
        .map(|(name, pkg)| (name.clone(), pkg.version.clone()))
        .collect();

    let mut conflicts = Vec::new();

    for (name, pkg) in tree {
        let meta = match registry::fetch_metadata(name) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let peers = registry::get_version_peer_dependencies(&meta, &pkg.version);
        let peers_meta = registry::get_version_peer_dependencies_meta(&meta, &pkg.version);

        for (peer_name, peer_spec) in peers {
            let optional = peers_meta
                .get(&peer_name)
                .and_then(|v| v.get("optional"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match resolved_versions.get(&peer_name) {
                Some(peer_ver) if registry::version_satisfies(&peer_spec, peer_ver) => {}
                Some(peer_ver) => conflicts.push(format!(
                    "peer {} required by {} but resolved {} (spec {})",
                    peer_name, name, peer_ver, peer_spec
                )),
                None if !optional => conflicts.push(format!(
                    "peer {} missing (required by {} spec {})",
                    peer_name, name, peer_spec
                )),
                None => {}
            }
        }
    }

    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(format!("Dependency conflict: {}", conflicts.join("; ")))
    }
}

/// Check if a version satisfies a semver spec
fn version_satisfies(version: &semver::Version, spec: &str) -> bool {
    if let Some(req) = crate::registry::cached_version_req(spec) {
        return req.matches(version);
    }
    // If spec parsing fails, try exact match
    spec.trim_start_matches('^').trim_start_matches('~') == version.to_string().as_str()
}

/// Verify minimal solution by checking transitive dependencies
fn verify_minimal_solution(
    solution: &HashMap<String, semver::Version>,
    _package_json_path: &Path,
) -> Result<(), String> {
    let mut metadata_cache: HashMap<String, serde_json::Value> = HashMap::new();

    for (name, version) in solution {
        let metadata = registry::fetch_metadata(name)
            .map_err(|e| format!("Cannot verify metadata for {}: {}", name, e))?;

        let versions = metadata
            .get("versions")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("Missing versions metadata for {}", name))?;

        let version_str = version.to_string();
        if !versions.contains_key(&version_str) {
            return Err(format!("Version {} not found for {}", version, name));
        }

        metadata_cache.insert(name.clone(), metadata);
    }

    // Validate direct dependency constraints among solved packages.
    for (name, version) in solution {
        let metadata = metadata_cache
            .get(name)
            .ok_or_else(|| format!("Missing metadata cache for {}", name))?;
        let deps = registry::get_version_required_dependencies(metadata, &version.to_string());

        for (dep_name, dep_spec) in deps {
            if let Some(dep_version) = solution.get(&dep_name) {
                if !registry::version_satisfies(&dep_spec, &dep_version.to_string()) {
                    return Err(format!(
                        "Minimal solution conflict: {} requires {} {}, got {}",
                        name, dep_name, dep_spec, dep_version
                    ));
                }
            }
        }
    }

    Ok(())
}

fn validate_direct_peer_conflicts(
    solution: &HashMap<String, PackedVersion>,
    metadata_cache: &HashMap<String, serde_json::Value>,
) -> Result<(), String> {
    let mut conflicts = Vec::new();

    for (pkg, packed_version) in solution {
        let version = packed_version.to_version().to_string();
        let Some(meta) = metadata_cache.get(pkg) else {
            continue;
        };

        let peers = registry::get_version_peer_dependencies(meta, &version);
        let peers_meta = registry::get_version_peer_dependencies_meta(meta, &version);

        for (peer_name, peer_spec) in peers {
            let optional = peers_meta
                .get(&peer_name)
                .and_then(|v| v.get("optional"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let resolved_peer = solution.get(&peer_name).map(|v| v.to_version().to_string());
            match resolved_peer {
                Some(v) if registry::version_satisfies(&peer_spec, &v) => {}
                Some(v) => conflicts.push(format!(
                    "peer {} required by {} but resolved {} (spec {})",
                    peer_name, pkg, v, peer_spec
                )),
                None if !optional => conflicts.push(format!(
                    "peer {} missing (required by {} spec {})",
                    peer_name, pkg, peer_spec
                )),
                None => {}
            }
        }
    }

    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(format!("Dependency conflict: {}", conflicts.join("; ")))
    }
}

/// Build resolved package tree from minimal version solution
fn build_tree_from_versions(
    solution: &HashMap<String, semver::Version>,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let mut result = HashMap::new();

    for (name, version) in solution {
        let version_str = version.to_string();

        // Fetch metadata for this package
        let (resolved_url, integrity) =
            match registry::resolve_tarball_via_manifest(name, &version_str) {
                Ok(Some((_, url, integrity_opt))) => (url, integrity_opt),
                Ok(None) | Err(_) => (
                    crate::lockfile::tarball_url_from_registry(name, &version_str),
                    None,
                ),
            };

        result.insert(
            name.clone(),
            ResolvedPackage {
                version: version_str.clone(),
                resolved: resolved_url,
                integrity,
                dependencies: HashMap::new(),
                peer_dependencies: HashMap::new(),
                peer_dependencies_meta: HashMap::new(),
            },
        );
    }

    Ok(result)
}

/// Build resolved package tree from PubGrub solution
fn build_tree_from_pubgrub_solution(
    solution: &HashMap<String, PackedVersion>,
    _original_specs: &HashMap<String, String>,
    metadata_cache: &HashMap<String, serde_json::Value>,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let mut result = HashMap::new();

    for (name, packed_version) in solution {
        let version = packed_version.to_version();
        let version_str = version.to_string();

        let (resolved_url, integrity) =
            match registry::resolve_tarball_via_manifest(name, &version_str) {
                Ok(Some((_, url, integrity_opt))) => (url, integrity_opt),
                Ok(None) | Err(_) => (
                    crate::lockfile::tarball_url_from_registry(name, &version_str),
                    None,
                ),
            };

        let (dependencies, peer_dependencies, peer_dependencies_meta) =
            if let Some(meta) = metadata_cache.get(name) {
                let raw_deps = registry::get_version_required_dependencies(meta, &version_str);
                let deps = raw_deps
                    .into_iter()
                    .map(|(dep, spec)| {
                        let resolved_spec = solution
                            .get(&dep)
                            .map(|resolved| resolved.to_version().to_string())
                            .unwrap_or(spec);
                        (dep, resolved_spec)
                    })
                    .collect::<HashMap<_, _>>();
                let peers = registry::get_version_peer_dependencies(meta, &version_str);
                let peer_meta = registry::get_version_peer_dependencies_meta(meta, &version_str);
                (deps, peers, peer_meta)
            } else {
                (HashMap::new(), HashMap::new(), HashMap::new())
            };

        result.insert(
            name.clone(),
            ResolvedPackage {
                version: version_str,
                resolved: resolved_url,
                integrity,
                dependencies,
                peer_dependencies,
                peer_dependencies_meta,
            },
        );
    }

    Ok(result)
}

fn resolve_full_tree_jagr(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let deps = lockfile::registry_only_deps(package_json_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(HashMap::new());
    }

    let cache_arc = Arc::new(Mutex::new(HashMap::<String, serde_json::Value>::new()));
    let mut input = SolveInput::default();
    for (name, spec) in &deps {
        input.root_requirements.insert(name.clone(), spec.clone());
    }

    const INITIAL_DOMAIN_VERSION_CAP: usize = 32;
    const MAX_DOMAIN_VERSION_CAP: usize = 512;

    let mut cap = INITIAL_DOMAIN_VERSION_CAP;
    let previous_assignment = None;

    loop {
        let (domains, truncated_any) = build_jagr_domains(&deps, &cache_arc, cap)?;

        // Try incremental solving first if we have a previous assignment
        let result = if let Some(prev) = previous_assignment.clone() {
            crate::sat_resolver::solve_incremental(&input, &domains, Some(&prev))
        } else {
            crate::sat_resolver::solve_exact(&input, &domains)
        };

        match result {
            Ok(solved) => {
                return build_tree_from_assignment(&solved.assignment, &cache_arc);
            }
            Err(e) => {
                let unsat_msg = format!("JAGR-1 UNSAT: {:?}", e);
                if truncated_any && cap < MAX_DOMAIN_VERSION_CAP {
                    cap = (cap * 2).min(MAX_DOMAIN_VERSION_CAP);
                    continue;
                }
                return Err(unsat_msg);
            }
        }
    }
}

/// Build a resolved tree directly from an on-disk npm package-lock.json,
/// verifying that every package.json dependency range is still satisfied by
/// the locked version. This is the Phase-0 lockfile-first gate: it skips the
/// registry metadata fetch AND the resolution pipeline entirely for warm
/// installs where the lockfile is still valid (0 registry calls, 0ms resolve).
///
/// Returns `None` when no valid npm lockfile exists or a range is violated.
pub fn build_tree_from_lockfile_if_valid(
    package_json_path: &Path,
) -> Option<HashMap<String, ResolvedPackage>> {
    let dir = package_json_path.parent().unwrap_or_else(|| Path::new("."));
    let deps = lockfile::registry_only_deps(package_json_path)?;
    if deps.is_empty() {
        return None;
    }
    // Only npm package-lock.json carries full dep/integrity data; bun.lock → None.
    let entries = lockfile::read_lockfile_full_entries_from_dir(dir)?;

    // Verify every package.json range against the locked version.
    for (name, spec) in &deps {
        let locked = entries
            .iter()
            .find(|e| &e.package == name)
            .map(|e| e.version.clone())?;
        if !registry::version_satisfies(spec, &locked) {
            return None;
        }
    }

    let mut tree: HashMap<String, ResolvedPackage> = HashMap::new();
    for e in &entries {
        let mut resolved_deps = HashMap::new();
        for (dep_name, dep_spec) in &e.dependencies {
            let dep_ver = tree
                .get(&format!("node_modules/{}", dep_name))
                .map(|rp| rp.version.clone())
                .or_else(|| {
                    entries
                        .iter()
                        .find(|x| &x.package == dep_name)
                        .map(|x| x.version.clone())
                });
            if let Some(dv) = dep_ver {
                if registry::version_satisfies(dep_spec, &dv) {
                    resolved_deps.insert(dep_name.clone(), dv);
                }
            }
        }
        tree.insert(
            format!("node_modules/{}", e.package),
            ResolvedPackage {
                version: e.version.clone(),
                resolved: e.resolved.clone(),
                integrity: e.integrity.clone(),
                dependencies: resolved_deps,
                peer_dependencies: e.peer_dependencies.clone(),
                peer_dependencies_meta: e.peer_dependencies_meta.clone(),
            },
        );
    }

    Some(tree)
}

pub fn resolve_full_tree_jagr3(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
        eprintln!("[JAGR3] resolve_full_tree_jagr3 entered");
    }
    let deps = lockfile::registry_only_deps(package_json_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(HashMap::new());
    }

    // ── PHASE 0: LOCKFILE-FIRST GATE ───────────────────────────────────────
    // Warm install: if an npm package-lock.json exists and still satisfies
    // every package.json range, return the locked tree directly. No registry
    // metadata fetch, no 4-layer resolution, no pipeline at all.
    if let Some(tree) = build_tree_from_lockfile_if_valid(package_json_path) {
        crate::utils::log("Lockfile-first gate hit: reusing resolved tree (0 metadata fetches)");
        return Ok(tree);
    }

    let cache_arc = Arc::new(Mutex::new(HashMap::<String, serde_json::Value>::new()));
    let (domains, _) = build_jagr_domains(&deps, &cache_arc, 512)?;

    if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
        eprintln!("[DOMAINS] total packages in domains: {}", domains.len());
        let mut names: Vec<&String> = domains.keys().collect();
        names.sort();
        for n in names {
            let d = &domains[n];
            let vs: Vec<String> = d.versions.keys().cloned().collect();
            eprintln!(
                "[DOMAIN] {}: {} versions: {:?}",
                n,
                vs.len(),
                &vs[..vs.len().min(10)]
            );
        }
    }

    // Build available_versions once, sorted
    let mut available_versions: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut package_names: Vec<&str> = domains.keys().map(|s| s.as_str()).collect();
    package_names.sort();

    for name in &package_names {
        let domain = &domains[*name];
        let mut versions: Vec<&str> = domain.versions.keys().map(|s| s.as_str()).collect();
        versions.sort();
        available_versions.insert(name, versions);
    }

    // Build dependencies using borrowed references — no cloning until final HashMap
    let deps_count = package_names
        .iter()
        .filter_map(|n| domains.get(*n))
        .map(|d| d.versions.len())
        .sum::<usize>();
    let mut dependencies: HashMap<(&str, &str), Vec<(&str, Vec<&str>)>> =
        HashMap::with_capacity(deps_count);
    for &name in &package_names {
        let Some(domain) = domains.get(name) else {
            continue;
        };
        let versions = available_versions.get(name).unwrap();
        for &version in versions {
            let Some(pkg_version) = domain.versions.get(version) else {
                continue;
            };
            let mut edges = Vec::with_capacity(pkg_version.dependencies.len());
            for (dep_name, dep_spec) in &pkg_version.dependencies {
                let allowed_versions = available_versions
                    .get(dep_name.as_str())
                    .map(|candidates| {
                        candidates
                            .iter()
                            .filter(|cv| registry::version_satisfies(dep_spec, cv))
                            .copied()
                            .collect::<Vec<&str>>()
                    })
                    .unwrap_or_default();
                if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() && allowed_versions.is_empty() {
                    eprintln!(
                        "[EDGE] {}@{} -> {} ({}): NO allowed versions",
                        name, version, dep_name, dep_spec
                    );
                }
                edges.push((dep_name.as_str(), allowed_versions));
            }
            dependencies.insert((name, version), edges);
        }
    }

    // Seed the root node so greedy/BFS can traverse package.json direct deps.
    // Without a ("root","0.0.0") entry the greedy fast path finds no children
    // and every cold install silently falls back to the SAT resolver.
    let mut root_edges: Vec<(&str, Vec<&str>)> = Vec::with_capacity(deps.len());
    for (dep_name, dep_spec) in &deps {
        let allowed_versions = available_versions
            .get(dep_name.as_str())
            .map(|candidates| {
                candidates
                    .iter()
                    .filter(|cv| registry::version_satisfies(dep_spec, cv))
                    .copied()
                    .collect::<Vec<&str>>()
            })
            .unwrap_or_default();
        root_edges.push((dep_name.as_str(), allowed_versions));
    }
    dependencies.insert(("root", "0.0.0"), root_edges);

    // ── PHASE A: VERSION-DOMAIN PRUNING ─────────────────────────────────────
    // A version that satisfies NO incoming dependency range (and is not a root
    // dep / lockfile pin) can never be chosen by any valid resolution. Drop it
    // here so L0/L2/L3/L1 all see a smaller graph — typically 30-60% fewer
    // `pkg@version` vars. This is a pure deletion: the reachable set is
    // unchanged, so correctness is identical.
    let mut reachable: HashSet<&str> = HashSet::new();
    for ((_n, _v), es) in dependencies.iter() {
        for (dep_name, allowed_versions) in es.iter() {
            for av in allowed_versions.iter() {
                reachable.insert(av);
            }
            // The dep itself is reachable (some version of it satisfies a range)
            reachable.insert(dep_name);
        }
    }
    // Root packages are always reachable (selected by package.json)
    for name in &package_names {
        reachable.insert(name);
    }
    let pruned: HashMap<&str, Vec<&str>> = available_versions
        .into_iter()
        .filter_map(|(name, versions)| {
            let kept: Vec<&str> = versions
                .iter()
                .copied()
                .filter(|v| reachable.contains(v))
                .collect();
            if kept.is_empty() {
                None
            } else {
                Some((name, kept))
            }
        })
        .collect();

    // Convert borrowed references to owned for ResolveRequest
    let owned_available: HashMap<String, Vec<String>> = pruned
        .into_iter()
        .map(|(k, vs)| {
            (
                k.to_string(),
                vs.into_iter().map(|v| v.to_string()).collect(),
            )
        })
        .collect();
    let owned_deps: HashMap<(String, String), Vec<(String, Vec<String>)>> = dependencies
        .into_iter()
        .map(|((n, v), edges)| {
            let owned_edges = edges
                .into_iter()
                .map(|(dn, avs)| {
                    (
                        dn.to_string(),
                        avs.into_iter().map(|a| a.to_string()).collect(),
                    )
                })
                .collect();
            ((n.to_string(), v.to_string()), owned_edges)
        })
        .collect();

    // Phase 1: feed the on-disk lockfile into the request so JagrResolver can
    // (a) verify_lockfile for a 0ms warm skip, and (b) use locked versions as
    // greedy-preference pins for incremental re-resolution.
    let on_disk_lockfile = lockfile::read_resolved_from_dir(
        package_json_path.parent().unwrap_or_else(|| Path::new(".")),
    );

    let request = ResolveRequest {
        root: PackageVersionVar {
            package: "root".to_string(),
            version: "0.0.0".to_string(),
        },
        available_versions: owned_available,
        dependencies: owned_deps,
        conflicts: Vec::new(),
        lockfile: on_disk_lockfile,
        version_ordinals: HashMap::new(),
    };

    let result = JagrResolver::new().resolve(&request);
    if !result.is_satisfiable {
        return Err(result.explanation.clone());
    }

    // The request root is a synthetic node (package "root", version "0.0.0")
    // used to seed traversal; it is not a real package and must not be
    // materialized into node_modules.
    let mut selection = result.selection.clone();
    selection.remove("root");
    if selection.is_empty() {
        return Ok(HashMap::new());
    }

    build_tree_from_jagr_selection(&selection, &cache_arc)
}

fn build_jagr_domains(
    root_deps: &HashMap<String, String>,
    cache_arc: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
    cap: usize,
) -> Result<(HashMap<String, PackageDomain>, bool), String> {
    let mut domains: HashMap<String, PackageDomain> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    // Accumulates every constraint that requires a package (root specs plus
    // every parent edge). A version that satisfies none of them can never be
    // selected, and expanding its dependencies would only inflate the frontier.
    let mut incoming: HashMap<String, Vec<String>> = HashMap::new();
    // The pushes below happen inside a per-VERSION loop, so without dedupe each
    // (package, constraint) pair is recorded once per version that declares it.
    // For a 300-package graph with hundreds of candidate versions each, that is
    // millions of cloned spec strings and a `frontier` of millions of names that
    // is re-sorted every round — it dominated both the wall clock and the RSS
    // (a 12-dependency project peaked above 1.4 GB and did not finish in 150 s).
    // Distinct constraints per package are naturally few, so deduping keeps the
    // accumulators proportional to the *graph*, not to versions × edges.
    let mut incoming_seen: HashSet<(String, String)> = HashSet::new();
    // Names already queued but not yet visited; mirrors `frontier`.
    let mut queued: HashSet<String> = HashSet::new();
    for (k, v) in root_deps {
        if incoming_seen.insert((k.clone(), v.clone())) {
            incoming.entry(k.clone()).or_default().push(v.clone());
        }
        queued.insert(k.clone());
    }
    // Frontier carries the package name to fetch next.
    let mut frontier: Vec<String> = root_deps.keys().cloned().collect();
    let mut truncated_any = false;

    while !frontier.is_empty() {
        frontier.sort();
        frontier.dedup();
        let batch: Vec<String> = frontier
            .iter()
            .filter(|n| !visited.contains(*n))
            .cloned()
            .collect();
        frontier.clear();
        if batch.is_empty() {
            break;
        }

        let mut results = registry::parallel_fetch_metadata(&batch, cache_arc);
        results.sort_by(|a, b| a.0.cmp(&b.0));

        if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
            eprintln!(
                "[DOMAIN-BFS] batch: {:?}, results: {}",
                batch,
                results.len()
            );
        }

        for (name, meta_res) in results {
            visited.insert(name.clone());
            let mut meta = match meta_res {
                Ok(m) => m,
                Err(e) => {
                    if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
                        eprintln!("[DOMAIN-BFS] {} ERROR: {}", name, e);
                    }
                    // Tolerate transitive packages that do not exist on the
                    // registry (e.g. historical deps of ancient versions). They
                    // can never be selected for a valid resolution, so skipping
                    // their domain is safe.
                    crate::utils::log(&format!(
                        "build_jagr_domains: skipping unresolvable transitive {}: {}",
                        name, e
                    ));
                    continue;
                }
            };

            let incoming_constraints = incoming.get(&name).cloned().unwrap_or_default();
            // A single-version synth packument (manifest synth) only knows the
            // "latest" version, which may fall outside the range required by the
            // incoming constraints (e.g. proxy-from-env latest 2.x vs ^1.1.0).
            // When the synth's versions cannot satisfy any incoming constraint,
            // escalate to the constraint-aware fetch so a full packument is used.
            let is_synth = meta.get("_vortex_manifest_synth").is_some()
                || meta.get("_bundled_index_synth").is_some();
            if is_synth
                && !incoming_constraints.is_empty()
                && !registry::meta_has_satisfying_version(&meta, &incoming_constraints)
            {
                if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
                    eprintln!(
                        "[DOMAIN-BFS] {}: synth cannot satisfy {:?}, escalating to full packument",
                        name, incoming_constraints
                    );
                }
                match registry::fetch_metadata_for_constraint(&name, &incoming_constraints[0]) {
                    Ok(full) => meta = full,
                    Err(e) => {
                        crate::utils::log(&format!(
                            "build_jagr_domains: skipping unresolvable transitive {}: {}",
                            name, e
                        ));
                        continue;
                    }
                }
            }

            let mut domain = PackageDomain::default();
            if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() && name == "axios" {
                let n_versions = meta
                    .get("versions")
                    .and_then(|v| v.as_object())
                    .map(|o| o.len())
                    .unwrap_or(0);
                let bundled = meta.get("_bundled_index_synth").is_some();
                let manifest = meta.get("_vortex_manifest_synth").is_some();
                let version_keys: Vec<String> = meta
                    .get("versions")
                    .and_then(|v| v.as_object())
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                eprintln!(
                    "[AXIOS] versions: {}, bundled_synth: {}, manifest_synth: {}, keys: {:?}",
                    n_versions, bundled, manifest, version_keys
                );
            }
            let (versions, truncated) =
                candidate_versions_for_constraints(&meta, &incoming_constraints, cap);
            truncated_any |= truncated;
            if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
                eprintln!(
                    "[DOMAIN-BFS] {}: {} candidate versions, incoming={:?}",
                    name,
                    versions.len(),
                    incoming_constraints
                );
            }
            for version in &versions {
                // `candidate_versions_for_constraints` already filtered by the
                // incoming constraints, so every version here is selectable; no
                // further filter needed. Expanding their deps is not dead weight.
                let deps = registry::get_version_required_dependencies(&meta, version);
                let optional_deps = registry::get_version_optional_dependencies(&meta, version);
                let peers = registry::get_version_peer_dependencies(&meta, version);
                let peer_meta = registry::get_version_peer_dependencies_meta(&meta, version);
                let optional_peers: HashSet<String> = peer_meta
                    .iter()
                    .filter_map(|(k, v)| {
                        let opt = v.get("optional").and_then(|b| b.as_bool()).unwrap_or(false);
                        if opt {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                for (dep_name, dep_spec) in &deps {
                    if !visited.contains(dep_name) {
                        if std::env::var("JHOL_DEBUG_DOMAINS").is_ok()
                            && (name == "axios" || dep_name == "follow-redirects")
                        {
                            eprintln!("[PUSH] {} -> {} spec={:?}", name, dep_name, dep_spec);
                        }
                        if incoming_seen.insert((dep_name.clone(), dep_spec.clone())) {
                            incoming
                                .entry(dep_name.clone())
                                .or_default()
                                .push(dep_spec.clone());
                        }
                        if queued.insert(dep_name.clone()) {
                            frontier.push(dep_name.clone());
                        }
                    }
                }
                for peer_name in peers.keys() {
                    if !optional_peers.contains(peer_name) && !visited.contains(peer_name) {
                        if incoming_seen.insert((peer_name.clone(), "*".to_string())) {
                            incoming
                                .entry(peer_name.clone())
                                .or_default()
                                .push("*".to_string());
                        }
                        if queued.insert(peer_name.clone()) {
                            frontier.push(peer_name.clone());
                        }
                    }
                }

                domain.versions.insert(
                    version.clone(),
                    PackageVersion {
                        version: version.clone(),
                        dependencies: deps,
                        optional_dependencies: optional_deps,
                        peer_dependencies: peers,
                        optional_peers,
                    },
                );
            }

            if !domain.versions.is_empty() {
                domains.insert(name, domain);
            }
        }
    }

    Ok((domains, truncated_any))
}

/// Filter candidate versions of a package to those satisfying at least one
/// incoming constraint, THEN cap to the newest `cap`. Ordering matters: for
/// packages with thousands of versions (react has 2896), the newest `cap`
/// versions may all be pre-release canaries that satisfy no real range, so a
/// plain top-N cap would yield an empty domain for e.g. `^18.3.1`.
fn candidate_versions_for_constraints(
    meta: &serde_json::Value,
    constraints: &[String],
    cap: usize,
) -> (Vec<String>, bool) {
    let mut parsed: Vec<semver::Version> = meta
        .get("versions")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.keys()
                .filter_map(|s| semver::Version::parse(s).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    parsed.sort();
    parsed.reverse();
    let filtered = if constraints.is_empty() {
        parsed
    } else {
        parsed
            .into_iter()
            .filter(|v| {
                constraints
                    .iter()
                    .any(|c| registry::version_satisfies(c, &v.to_string()))
            })
            .collect::<Vec<_>>()
    };
    if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
        let total = meta
            .get("versions")
            .and_then(|v| v.as_object())
            .map(|o| o.len())
            .unwrap_or(0);
        if total < 8 {
            eprintln!(
                "[CANDIDATE] total_versions={} filtered={} constraints={:?} first_filtered={:?}",
                total,
                filtered.len(),
                constraints,
                filtered.first().map(|v| v.to_string())
            );
        }
    }
    let truncated = filtered.len() > cap;
    let versions = filtered
        .into_iter()
        .take(cap)
        .map(|v| v.to_string())
        .collect();
    (versions, truncated)
}

fn build_tree_from_assignment(
    assignment: &HashMap<String, String>,
    cache_arc: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let mut tree: HashMap<String, ResolvedPackage> = HashMap::new();
    let mut names: Vec<String> = assignment.keys().cloned().collect();
    names.sort();

    for name in names {
        let version = assignment
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("Missing assignment for {}", name))?;
        let meta = {
            let mut cache = mutex_lock!(cache_arc);
            let cached = registry::fetch_metadata_cached(&name, &mut cache)?;
            // A cached single-version synth may not contain the version the
            // solver actually selected (e.g. asynckit resolved to 0.4.0 but the
            // manifest synth only knew "latest" 0.5.0). Escalate to the full
            // packument so tarball/integrity lookups succeed.
            if !registry::meta_contains_version(&cached, &version) {
                if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
                    eprintln!(
                        "[TREE-ESC] {}@{} not in cached meta, escalating",
                        name, version
                    );
                }
                match registry::fetch_metadata_for_constraint(&name, &version) {
                    Ok(full) => {
                        cache.insert(name.clone(), full.clone());
                        full
                    }
                    Err(e) => {
                        crate::utils::log(&format!(
                            "build_tree_from_assignment: escalating {}@{} to full packument failed: {}",
                            name, version, e
                        ));
                        cached
                    }
                }
            } else {
                cached
            }
        };

        let resolved_url = registry::get_tarball_url(&meta, &version)
            .ok_or_else(|| format!("No tarball URL for {}@{}", name, version))?;
        let integrity = registry::get_integrity_for_version(&meta, &version);
        let version_deps = registry::get_version_required_dependencies(&meta, &version);
        let peer_deps = registry::get_version_peer_dependencies(&meta, &version);
        let peer_deps_meta = registry::get_version_peer_dependencies_meta(&meta, &version);

        let mut resolved_deps = HashMap::new();
        for (dep_name, dep_spec) in &version_deps {
            if let Some(dep_version) = assignment.get(dep_name) {
                if registry::version_satisfies(dep_spec, dep_version) {
                    resolved_deps.insert(dep_name.clone(), dep_version.clone());
                }
            }
        }

        tree.insert(
            format!("node_modules/{}", name),
            ResolvedPackage {
                version,
                resolved: resolved_url,
                integrity,
                dependencies: resolved_deps,
                peer_dependencies: peer_deps,
                peer_dependencies_meta: peer_deps_meta,
            },
        );
    }

    Ok(tree)
}

pub fn build_tree_from_jagr_selection(
    selection: &HashMap<String, String>,
    cache_arc: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let mut tree: HashMap<String, ResolvedPackage> = HashMap::new();
    let mut names: Vec<String> = selection.keys().cloned().collect();
    names.sort();

    for name in names {
        let version = selection
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("Missing selection for {}", name))?;
        let meta = {
            let mut cache = mutex_lock!(cache_arc);
            let cached = registry::fetch_metadata_cached(&name, &mut cache)?;
            if !registry::meta_contains_version(&cached, &version) {
                if std::env::var("JHOL_DEBUG_DOMAINS").is_ok() {
                    eprintln!(
                        "[TREE-ESC] {}@{} not in cached meta, escalating",
                        name, version
                    );
                }
                match registry::fetch_metadata_for_constraint(&name, &version) {
                    Ok(full) => {
                        cache.insert(name.clone(), full.clone());
                        full
                    }
                    Err(e) => {
                        crate::utils::log(&format!(
                            "build_tree_from_jagr_selection: escalating {}@{} to full packument failed: {}",
                            name, version, e
                        ));
                        cached
                    }
                }
            } else {
                cached
            }
        };

        let resolved_url = registry::get_tarball_url(&meta, &version)
            .ok_or_else(|| format!("No tarball URL for {}@{}", name, version))?;
        let integrity = registry::get_integrity_for_version(&meta, &version);
        let version_deps = registry::get_version_required_dependencies(&meta, &version);
        let peer_deps = registry::get_version_peer_dependencies(&meta, &version);
        let peer_deps_meta = registry::get_version_peer_dependencies_meta(&meta, &version);

        let mut resolved_deps = HashMap::new();
        for (dep_name, dep_spec) in &version_deps {
            if let Some(dep_version) = selection.get(dep_name) {
                if registry::version_satisfies(dep_spec, dep_version) {
                    resolved_deps.insert(dep_name.clone(), dep_version.clone());
                }
            }
        }

        tree.insert(
            format!("node_modules/{}", name),
            ResolvedPackage {
                version,
                resolved: resolved_url,
                integrity,
                dependencies: resolved_deps,
                peer_dependencies: peer_deps,
                peer_dependencies_meta: peer_deps_meta,
            },
        );
    }

    Ok(tree)
}

fn resolve_full_tree_legacy(
    package_json_path: &Path,
) -> Result<HashMap<String, ResolvedPackage>, String> {
    let deps = lockfile::registry_only_deps(package_json_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(HashMap::new());
    }

    let direct_names: Vec<String> = deps.keys().cloned().collect();
    let cache_arc = Arc::new(Mutex::new(HashMap::<String, serde_json::Value>::new()));
    let results = registry::parallel_fetch_metadata(&direct_names, &cache_arc);
    for (name, res) in results {
        if let Err(e) = res {
            return Err(format!("Failed to fetch metadata for {}: {}", name, e));
        }
    }

    let mut tree: HashMap<String, ResolvedPackage> = HashMap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut requirements: HashMap<String, Vec<Requirement>> = HashMap::new();
    let mut peer_requirements: HashMap<String, Vec<PeerRequirement>> = HashMap::new();

    let mut queue: Vec<(String, String, String, String)> = deps
        .iter()
        .map(|(name, spec)| {
            requirements
                .entry(name.clone())
                .or_default()
                .push(Requirement {
                    requester: "root".to_string(),
                    spec: spec.clone(),
                });
            (
                format!("node_modules/{}", name),
                name.clone(),
                spec.clone(),
                "root".to_string(),
            )
        })
        .collect();

    let mut conflicts: Vec<String> = Vec::new();
    while let Some((key, name, spec, requester)) = queue.pop() {
        let meta = {
            let mut cache = mutex_lock!(cache_arc);
            registry::fetch_metadata_cached(&name, &mut cache)?
        };

        requirements
            .entry(name.clone())
            .or_default()
            .push(Requirement { requester, spec });

        let combined_specs = requirements
            .get(&name)
            .map(|reqs| reqs.iter().map(|r| r.spec.clone()).collect::<Vec<_>>())
            .unwrap_or_default();

        let version = resolve_highest_satisfying(&meta, &combined_specs).ok_or_else(|| {
            let refs = requirements
                .get(&name)
                .map(|reqs| {
                    reqs.iter()
                        .map(|r| format!("{} -> {}", r.requester, r.spec))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!(
                "Dependency conflict for {} (no version satisfies all): {}",
                name, refs
            )
        })?;

        if let Some(existing) = tree.get(&key) {
            if existing.version == version {
                continue;
            }
            let combined_specs_str = combined_specs.join(", ");
            conflicts.push(format!(
                "{}: existing {} vs {} (specs: {})",
                name, existing.version, version, combined_specs_str
            ));
            continue;
        }
        if seen.contains(&(name.clone(), version.clone())) {
            continue;
        }
        seen.insert((name.clone(), version.clone()));

        let resolved_url = registry::get_tarball_url(&meta, &version)
            .ok_or_else(|| format!("No tarball URL for {}@{}", name, version))?;
        let integrity = registry::get_integrity_for_version(&meta, &version);

        let version_deps = registry::get_version_required_dependencies(&meta, &version);
        let peer_deps = registry::get_version_peer_dependencies(&meta, &version);
        let peer_deps_meta = registry::get_version_peer_dependencies_meta(&meta, &version);
        let mut resolved_deps = HashMap::new();
        for (dep_name, dep_spec) in &version_deps {
            let dep_meta = {
                let mut cache = mutex_lock!(cache_arc);
                match registry::fetch_metadata_cached(dep_name, &mut cache) {
                    Ok(m) => m,
                    Err(_) => continue,
                }
            };
            requirements
                .entry(dep_name.clone())
                .or_default()
                .push(Requirement {
                    requester: name.clone(),
                    spec: dep_spec.clone(),
                });
            if let Some(dep_version) =
                resolve_highest_satisfying(&dep_meta, std::slice::from_ref(dep_spec))
            {
                resolved_deps.insert(dep_name.clone(), dep_version.clone());
                let dep_key = format!("node_modules/{}", dep_name);
                if !seen.contains(&(dep_name.clone(), dep_version)) {
                    queue.push((dep_key, dep_name.clone(), dep_spec.clone(), name.clone()));
                }
            }
        }

        for (peer_name, peer_spec) in &peer_deps {
            let optional = peer_deps_meta
                .get(peer_name)
                .and_then(|v| v.get("optional"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            peer_requirements
                .entry(peer_name.clone())
                .or_default()
                .push(PeerRequirement {
                    requester: name.clone(),
                    spec: peer_spec.clone(),
                    optional,
                });
        }

        tree.insert(
            name.clone(),
            ResolvedPackage {
                version,
                resolved: resolved_url,
                integrity,
                dependencies: resolved_deps,
                peer_dependencies: peer_deps,
                peer_dependencies_meta: peer_deps_meta,
            },
        );
    }

    let mut peer_conflicts: Vec<String> = Vec::new();
    for (peer_name, reqs) in &peer_requirements {
        let resolved_version = tree
            .get(&format!("node_modules/{}", peer_name))
            .map(|p| p.version.clone());
        if let Some(resolved) = resolved_version {
            for req in reqs {
                if !registry::version_satisfies(&req.spec, &resolved) {
                    peer_conflicts.push(format!(
                        "peer {} required by {} but resolved {} (spec {})",
                        peer_name, req.requester, resolved, req.spec
                    ));
                }
            }
        } else {
            let required_reqs: Vec<&PeerRequirement> =
                reqs.iter().filter(|r| !r.optional).collect();
            if required_reqs.is_empty() {
                continue;
            }
            let req_list = required_reqs
                .iter()
                .map(|r| format!("{} -> {}", r.requester, r.spec))
                .collect::<Vec<_>>()
                .join(", ");
            peer_conflicts.push(format!(
                "peer {} missing (requirements: {})",
                peer_name, req_list
            ));
        }
    }

    if !conflicts.is_empty() || !peer_conflicts.is_empty() {
        let mut all = Vec::new();
        all.extend(conflicts);
        all.extend(peer_conflicts);
        return Err(format!(
            "Dependency conflict: {}. Consider updating dependencies or using a single version.",
            all.join("; ")
        ));
    }
    Ok(tree)
}

fn resolve_highest_satisfying(meta: &serde_json::Value, specs: &[String]) -> Option<String> {
    let versions = meta.get("versions")?.as_object()?;
    let mut parsed: Vec<semver::Version> = versions
        .keys()
        .filter_map(|v| semver::Version::parse(v).ok())
        .collect();
    parsed.sort();
    parsed.reverse();
    for ver in parsed {
        let ver_str = ver.to_string();
        if specs
            .iter()
            .all(|s| registry::version_satisfies(s, &ver_str))
        {
            return Some(ver_str);
        }
    }
    None
}

/// Same as resolve_highest_satisfying but takes pre-parsed, sorted (ascending) version list.
/// Avoids re-parsing and re-sorting versions per lookup.
fn resolve_highest_satisfying_parsed(
    parsed: &[(semver::Version, String)],
    specs: &[String],
) -> Option<String> {
    for (_ver, ver_str) in parsed.iter().rev() {
        if specs
            .iter()
            .all(|s| registry::version_satisfies(s, ver_str))
        {
            return Some(ver_str.clone());
        }
    }
    None
}

/// Read root package name and version from package.json.
fn read_root_package_info(path: &Path) -> Result<(String, String), String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let version = v
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    Ok((name, version))
}

/// Build lockfile packages object: root "" + all node_modules/* entries.
fn normalize_lockfile_tree_key(raw: &str) -> String {
    let normalized = raw
        .trim_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("/");

    if normalized.is_empty() {
        return normalized;
    }

    if normalized.starts_with("node_modules/") {
        return normalized;
    }

    // Legacy native tree keys can be plain package names (including scoped).
    if !normalized.contains("/node_modules/")
        && (normalized.starts_with('@') || !normalized.contains('/'))
    {
        return format!("node_modules/{}", normalized);
    }

    normalized
}

/// Reduce a tree key (`node_modules/<name>` or nested `.../node_modules/<name>`
/// or a bare name) to the clean package name used in registry specs and the
/// install/capsule path. Handles scoped `@scope/pkg` names.
fn normalize_tree_key_to_package_name(raw: &str) -> String {
    // Strip the last `node_modules/` segment boundary (nested trees).
    if let Some(idx) = raw.rfind("node_modules/") {
        return raw[idx + "node_modules/".len()..].to_string();
    }
    raw.trim_start_matches("node_modules/").to_string()
}

fn build_packages_json(
    root_name: &str,
    root_version: &str,
    declared_specs: &HashMap<String, String>,
    tree: &HashMap<String, ResolvedPackage>,
) -> serde_json::Value {
    let mut packages = serde_json::Map::new();

    let mut normalized_tree: BTreeMap<String, &ResolvedPackage> = BTreeMap::new();
    for (raw_key, pkg) in tree {
        let key = normalize_lockfile_tree_key(raw_key);
        if key.is_empty() {
            continue;
        }
        let prefer_this =
            raw_key.starts_with("node_modules/") || !normalized_tree.contains_key(&key);
        if prefer_this {
            normalized_tree.insert(key, pkg);
        }
    }

    // The root `dependencies` block records what the project *declares*, so the
    // written spec goes here — never the resolved version. Writing the version
    // pinned every range (`"ms": "^2.1.3"` became `"2.1.3"`, so nothing could ever
    // move within the range again) and silently dropped every special dep, because
    // an `npm:` alias / `file:` / `workspace:` / git dependency has no registry
    // tree entry to look up:
    //   dependencies: {"ms": "^2.1.3", "my-lodash": "npm:lodash@4.17.21",
    //                  "local-lib": "file:../lib", "git-dep": "git+file://…"}
    let mut root_deps = serde_json::Map::new();
    let mut names: Vec<&String> = declared_specs.keys().collect();
    names.sort();
    for name in names {
        let spec = declared_specs.get(name).map(String::as_str).unwrap_or("");
        let resolved = normalized_tree
            .get(&format!("node_modules/{}", name))
            .copied()
            .or_else(|| tree.get(name));
        // A special is materialized by the spec-grammar pre-pass, so it belongs in
        // the lockfile whether or not it also resolved. A plain range is only
        // recorded once it resolved — an optional dep the resolver deliberately
        // skipped (unavailable, or for this platform) must not be forced back in.
        let is_special = !matches!(
            crate::spec_grammar::classify(spec),
            crate::spec_grammar::Spec::Registry(_)
        );
        if is_special || resolved.is_some() {
            let value = if spec.is_empty() {
                resolved.map(|p| p.version.clone()).unwrap_or_default()
            } else {
                spec.to_string()
            };
            root_deps.insert(name.clone(), serde_json::Value::String(value));
        }
    }
    packages.insert(
        "".to_string(),
        serde_json::json!({
            "name": root_name,
            "version": root_version,
            "dependencies": root_deps,
        }),
    );

    for (key, pkg) in &normalized_tree {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "version".to_string(),
            serde_json::Value::String(pkg.version.clone()),
        );
        entry.insert(
            "resolved".to_string(),
            serde_json::Value::String(pkg.resolved.clone()),
        );
        if let Some(ref i) = pkg.integrity {
            entry.insert(
                "integrity".to_string(),
                serde_json::Value::String(i.clone()),
            );
        }
        let deps: serde_json::Map<String, serde_json::Value> = pkg
            .dependencies
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let peer_deps: serde_json::Map<String, serde_json::Value> = pkg
            .peer_dependencies
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let peer_deps_meta: serde_json::Map<String, serde_json::Value> = pkg
            .peer_dependencies_meta
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        entry.insert("dependencies".to_string(), serde_json::Value::Object(deps));
        entry.insert(
            "requires".to_string(),
            serde_json::Value::Bool(!pkg.dependencies.is_empty()),
        );
        if !peer_deps.is_empty() {
            entry.insert(
                "peerDependencies".to_string(),
                serde_json::Value::Object(peer_deps),
            );
        }
        if !peer_deps_meta.is_empty() {
            entry.insert(
                "peerDependenciesMeta".to_string(),
                serde_json::Value::Object(peer_deps_meta),
            );
        }
        packages.insert(key.clone(), serde_json::Value::Object(entry));
    }

    serde_json::Value::Object(packages)
}

/// Lockfile cache for incremental updates
#[derive(Debug)]
struct LockfileCache {
    last_modified: std::time::SystemTime,
    content_hash: u64,
    packages: HashMap<String, ResolvedPackage>,
}

/// Incremental lockfile updater
pub struct IncrementalLockfileUpdater {
    cache: Option<LockfileCache>,
}

impl Default for IncrementalLockfileUpdater {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalLockfileUpdater {
    pub fn new() -> Self {
        Self { cache: None }
    }

    /// Check if lockfile needs updating based on package.json changes
    pub fn needs_update(&self, lock_path: &Path, package_json_path: &Path) -> bool {
        if !lock_path.exists() {
            return true;
        }

        let lock_meta = std::fs::metadata(lock_path);
        let package_meta = std::fs::metadata(package_json_path);

        match (lock_meta, package_meta) {
            (Ok(lock_meta), Ok(package_meta)) => {
                let lock_modified = lock_meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                let package_modified = package_meta.modified().unwrap_or(std::time::UNIX_EPOCH);

                if package_modified > lock_modified {
                    return true;
                }

                // Check if cache is still valid
                if let Some(cache) = &self.cache {
                    if let Ok(current_modified) = lock_path.metadata().and_then(|m| m.modified()) {
                        if current_modified == cache.last_modified {
                            return false;
                        }
                    }
                }

                true
            }
            _ => true,
        }
    }

    /// Update lockfile incrementally if possible, otherwise write full lockfile
    pub fn update_lockfile(
        &mut self,
        lock_path: &Path,
        package_json_path: &Path,
        tree: &HashMap<String, ResolvedPackage>,
    ) -> Result<(), String> {
        if !self.needs_update(lock_path, package_json_path) {
            return Ok(());
        }

        // Try incremental update first
        if let Some(existing_tree) = self.read_existing_lockfile(lock_path) {
            if let Some(incremental_tree) = self.compute_incremental_update(&existing_tree, tree) {
                let result = self.write_incremental_lockfile(
                    lock_path,
                    package_json_path,
                    &incremental_tree,
                );
                if result.is_ok() {
                    let _ =
                        write_capsule_from_tree(lock_path.parent().unwrap_or(Path::new(".")), tree);
                }
                return result;
            }
        }

        // Fall back to full write
        let result = self.write_full_lockfile(lock_path, package_json_path, tree);
        if result.is_ok() {
            let _ = write_capsule_from_tree(lock_path.parent().unwrap_or(Path::new(".")), tree);
        }
        result
    }

    fn read_existing_lockfile(&self, lock_path: &Path) -> Option<HashMap<String, ResolvedPackage>> {
        if !lock_path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(lock_path).ok()?;
        let lockfile: serde_json::Value = serde_json::from_str(&content).ok()?;

        let packages = lockfile.get("packages")?.as_object()?;
        let mut tree = HashMap::new();

        for (key, value) in packages {
            if key.is_empty() {
                continue; // Skip root package
            }

            let version = value.get("version")?.as_str()?.to_string();
            let resolved = value.get("resolved")?.as_str()?.to_string();
            let integrity = value
                .get("integrity")
                .and_then(|i| i.as_str())
                .map(String::from);

            let dependencies = value
                .get("dependencies")
                .and_then(|d| d.as_object())
                .map(|d| {
                    d.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let peer_dependencies = value
                .get("peerDependencies")
                .and_then(|d| d.as_object())
                .map(|d| {
                    d.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let peer_dependencies_meta = value
                .get("peerDependenciesMeta")
                .and_then(|d| d.as_object())
                .map(|d| d.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();

            tree.insert(
                key.clone(),
                ResolvedPackage {
                    version,
                    resolved,
                    integrity,
                    dependencies,
                    peer_dependencies,
                    peer_dependencies_meta,
                },
            );
        }

        Some(tree)
    }

    fn compute_incremental_update(
        &self,
        existing: &HashMap<String, ResolvedPackage>,
        new: &HashMap<String, ResolvedPackage>,
    ) -> Option<HashMap<String, ResolvedPackage>> {
        let mut updated = existing.clone();
        let mut has_changes = false;

        for (key, new_pkg) in new {
            if let Some(existing_pkg) = existing.get(key) {
                if existing_pkg.version != new_pkg.version
                    || existing_pkg.resolved != new_pkg.resolved
                    || existing_pkg.integrity != new_pkg.integrity
                {
                    updated.insert(key.clone(), new_pkg.clone());
                    has_changes = true;
                }
            } else {
                updated.insert(key.clone(), new_pkg.clone());
                has_changes = true;
            }
        }

        if has_changes {
            Some(updated)
        } else {
            None
        }
    }

    /// Computes incremental update restricted to the Cellular Sheaf perturbation support $\operatorname{supp}(\Delta L_{\mathcal{F}})$.
    /// Avoids inspecting and copying untouched clusters in the lockfile graph.
    pub fn compute_incremental_update_with_support(
        &self,
        existing: &HashMap<String, ResolvedPackage>,
        new: &HashMap<String, ResolvedPackage>,
        affected_support: &HashSet<String>,
    ) -> Option<HashMap<String, ResolvedPackage>> {
        let mut updated = existing.clone();
        let mut has_changes = false;

        for (key, new_pkg) in new {
            // Only inspect nodes within the topological perturbation support or new nodes
            if !affected_support.contains(key) && existing.contains_key(key) {
                continue;
            }

            if let Some(existing_pkg) = existing.get(key) {
                if existing_pkg.version != new_pkg.version
                    || existing_pkg.resolved != new_pkg.resolved
                    || existing_pkg.integrity != new_pkg.integrity
                {
                    updated.insert(key.clone(), new_pkg.clone());
                    has_changes = true;
                }
            } else {
                updated.insert(key.clone(), new_pkg.clone());
                has_changes = true;
            }
        }

        if has_changes {
            Some(updated)
        } else {
            None
        }
    }

    fn write_incremental_lockfile(
        &mut self,
        lock_path: &Path,
        package_json_path: &Path,
        tree: &HashMap<String, ResolvedPackage>,
    ) -> Result<(), String> {
        let (root_name, root_version) = read_root_package_info(package_json_path)?;
        let declared_specs =
            lockfile::read_package_json_deps(package_json_path).unwrap_or_default();

        let packages = build_packages_json(&root_name, &root_version, &declared_specs, tree);

        let lockfile_content = serde_json::json!({
            "name": root_name,
            "version": root_version,
            "lockfileVersion": 3,
            "packages": packages,
        });

        // Use atomic write to prevent corruption
        let temp_path = lock_path.with_extension("tmp");
        let json = serde_json::to_string(&lockfile_content).map_err(|e| e.to_string())?;
        std::fs::write(&temp_path, json).map_err(|e| e.to_string())?;
        std::fs::rename(temp_path, lock_path).map_err(|e| e.to_string())?;

        // Update cache
        self.update_cache(lock_path, tree);

        Ok(())
    }

    fn write_full_lockfile(
        &mut self,
        lock_path: &Path,
        package_json_path: &Path,
        tree: &HashMap<String, ResolvedPackage>,
    ) -> Result<(), String> {
        let (root_name, root_version) = read_root_package_info(package_json_path)?;
        let declared_specs =
            lockfile::read_package_json_deps(package_json_path).unwrap_or_default();

        let packages = build_packages_json(&root_name, &root_version, &declared_specs, tree);

        let lockfile_content = serde_json::json!({
            "name": root_name,
            "version": root_version,
            "lockfileVersion": 3,
            "packages": packages,
        });

        // Use atomic write to prevent corruption
        let temp_path = lock_path.with_extension("tmp");
        let json = serde_json::to_string(&lockfile_content).map_err(|e| e.to_string())?;
        std::fs::write(&temp_path, json).map_err(|e| e.to_string())?;
        std::fs::rename(temp_path, lock_path).map_err(|e| e.to_string())?;

        // Update cache
        self.update_cache(lock_path, tree);

        Ok(())
    }

    fn update_cache(&mut self, lock_path: &Path, tree: &HashMap<String, ResolvedPackage>) {
        if let Ok(metadata) = lock_path.metadata() {
            if let Ok(modified) = metadata.modified() {
                let content_hash = self.compute_tree_hash(tree);
                self.cache = Some(LockfileCache {
                    last_modified: modified,
                    content_hash,
                    packages: tree.clone(),
                });
            }
        }
    }

    fn compute_tree_hash(&self, tree: &HashMap<String, ResolvedPackage>) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        for (key, pkg) in tree {
            key.hash(&mut hasher);
            pkg.version.hash(&mut hasher);
            pkg.resolved.hash(&mut hasher);
            if let Some(ref integrity) = pkg.integrity {
                integrity.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

/// Write package-lock.json to the given path.
pub fn write_package_lock(
    lock_path: &Path,
    package_json_path: &Path,
    tree: &HashMap<String, ResolvedPackage>,
) -> Result<(), String> {
    let mut updater = IncrementalLockfileUpdater::new();
    updater.update_lockfile(lock_path, package_json_path, tree)
}

/// Async lockfile writer for better I/O performance
pub async fn write_package_lock_async(
    lock_path: &Path,
    package_json_path: &Path,
    tree: &HashMap<String, ResolvedPackage>,
) -> Result<(), String> {
    let (root_name, root_version) = read_root_package_info(package_json_path)?;
    let declared_specs = lockfile::read_package_json_deps(package_json_path).unwrap_or_default();

    let packages = build_packages_json(&root_name, &root_version, &declared_specs, tree);

    let lockfile_content = serde_json::json!({
        "name": root_name,
        "version": root_version,
        "lockfileVersion": 3,
        "packages": packages,
    });

    let json = serde_json::to_string(&lockfile_content).map_err(|e| e.to_string())?;

    // Use async I/O for better performance
    std::fs::write(lock_path, json).map_err(|e| e.to_string())?;
    Ok(())
}

/// Write a .jhol.capsule from a resolved package tree.
/// Called after lockfile write to enable zero-parse installs.
pub fn write_capsule_from_tree(
    project_dir: &Path,
    tree: &HashMap<String, ResolvedPackage>,
) -> Result<(), String> {
    use crate::capsule::{capsule_path, write_capsule, CapsuleBuilder};

    // Compute manifest hash from package.json + package-lock.json
    let manifest_hash = crate::capsule::compute_manifest_hash(project_dir).unwrap_or([0u8; 8]);

    let mut builder = CapsuleBuilder::new(manifest_hash);

    // Build name→index map for dep edges
    let mut name_to_idx: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut entries: Vec<(String, &ResolvedPackage)> = Vec::new();

    for (spec, pkg) in tree {
        // Tree keys are `node_modules/<name>` (nested scoped names may carry
        // deeper node_modules segments). The capsule entries must store clean
        // package names so install/can_reuse_lockfile_for_requested can build
        // registry specs for download/URL construction.
        let name = normalize_tree_key_to_package_name(spec);
        entries.push((name, pkg));
    }

    // First pass: add packages
    for (name, pkg) in &entries {
        // Prefer store/session hash; fall back to integrity. Never leave zeros —
        // a zero-hash capsule forces a multi-second serial install fallback.
        let tarball_hash = resolve_tarball_hash_from_pkg(name, pkg);
        let version = &pkg.version;
        let unpacked = 0u32;
        let mut flags = 0u16;
        // Mark HAS_BIN when the store extract declares a bin (capsule v2 warm path).
        if store_package_declares_bin_local(&tarball_hash) {
            flags |= 8;
        }
        let idx = builder.add_package(name, version, &tarball_hash, unpacked, &[], 0, flags, "");
        name_to_idx.insert(name.clone(), idx);
    }

    // Second pass: add dependency edges
    for (name, pkg) in &entries {
        if let Some(&_idx) = name_to_idx.get(name) {
            let deps: Vec<(u32, u16)> = pkg
                .dependencies
                .keys()
                .filter_map(|dep_name| name_to_idx.get(dep_name).map(|&dep_idx| (dep_idx, 0u16)))
                .collect();
            if !deps.is_empty() {
                // We can't update after add_package. For now, deps are skipped.
                // The capsule is still useful for linear install without dep edges.
                let _ = deps;
            }
        }
    }

    let cap_path = capsule_path(project_dir);
    write_capsule(&cap_path, builder)?;
    Ok(())
}

fn store_package_declares_bin_local(tarball_hash: &[u8; 32]) -> bool {
    if tarball_hash.iter().all(|&b| b == 0) {
        return false;
    }
    let hex = hex::encode(tarball_hash);
    let shard = &hex[..2.min(hex.len())];
    let cache = crate::utils::get_cache_dir();
    for pj in [
        std::path::PathBuf::from(&cache)
            .join("store_unpacked")
            .join(shard)
            .join(&hex)
            .join("package.json"),
        std::path::PathBuf::from(&cache)
            .join("store_unpacked")
            .join(&hex)
            .join("package.json"),
    ] {
        if let Ok(text) = std::fs::read_to_string(pj) {
            if text.contains("\"bin\"") {
                return true;
            }
        }
    }
    false
}

fn resolve_tarball_hash_from_pkg(name: &str, pkg: &ResolvedPackage) -> [u8; 32] {
    use base64::Engine;
    let index_key = format!("{}@{}", name, pkg.version);

    // Store index (written during this install before lockfile/capsule).
    if let Some(hex_hash) = crate::utils::with_store_index(|idx| idx.get(&index_key).cloned()) {
        if let Ok(bytes) = hex::decode(&hex_hash) {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                return arr;
            }
        }
    }
    if let Some(hex_hash) = crate::registry::get_session_tarball_hash(&index_key) {
        if let Ok(bytes) = hex::decode(&hex_hash) {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                return arr;
            }
        }
    }
    if !pkg.resolved.is_empty() {
        if let Some(hex_hash) = crate::registry::get_session_tarball_hash(&pkg.resolved) {
            if let Ok(bytes) = hex::decode(&hex_hash) {
                if bytes.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    return arr;
                }
            }
        }
    }

    if let Some(ref integrity) = pkg.integrity {
        if let Some(b64) = integrity.strip_prefix("sha256-") {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64) {
                if decoded.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&decoded);
                    return arr;
                }
            }
        }
        // npm lockfiles carry sha512- SRI; map via harvest/login store.
        if let Some((hash, _, _)) = crate::content::find_unpacked(integrity) {
            if let Ok(bytes) = hex::decode(&hash) {
                if bytes.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    return arr;
                }
            }
        }
    }
    [0u8; 32]
}

#[cfg(test)]
mod tests {
    /// The root `dependencies` block must carry the *written spec*, not the
    /// resolved version. Writing the version pinned every range and dropped every
    /// special (`npm:` alias, `file:`, `workspace:`, git) because those have no
    /// registry tree entry.
    #[test]
    fn root_dependencies_keep_the_declared_spec() {
        let mut declared = HashMap::new();
        declared.insert("ms".to_string(), "^2.1.3".to_string());
        declared.insert("my-lodash".to_string(), "npm:lodash@4.17.21".to_string());
        declared.insert("local-lib".to_string(), "file:../lib".to_string());
        declared.insert("gone".to_string(), "optional:^1.0.0".to_string());

        let mut tree: HashMap<String, ResolvedPackage> = HashMap::new();
        tree.insert(
            "node_modules/ms".to_string(),
            ResolvedPackage {
                version: "2.1.3".to_string(),
                resolved: "https://example.invalid/ms.tgz".to_string(),
                integrity: None,
                dependencies: HashMap::new(),
                peer_dependencies: HashMap::new(),
                peer_dependencies_meta: HashMap::new(),
            },
        );

        let v = build_packages_json("app", "1.0.0", &declared, &tree);
        let root = &v[""]["dependencies"];
        assert_eq!(root["ms"], "^2.1.3", "a range must stay a range");
        assert_eq!(root["my-lodash"], "npm:lodash@4.17.21", "alias must appear");
        assert_eq!(root["local-lib"], "file:../lib", "local must appear");
        assert!(
            root.get("gone").is_none(),
            "a dep the resolver skipped must not be forced in"
        );
    }

    use super::*;

    fn fake_meta(versions: &[&str]) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for v in versions {
            m.insert((*v).to_string(), serde_json::json!({ "dist": { "tarball": format!("https://registry.npmjs.org/p/-/p-{}.tgz", v) } }));
        }
        serde_json::json!({
            "name": "p",
            "versions": m,
            "dist-tags": { "latest": versions.last().copied().unwrap_or("0.0.0") }
        })
    }

    #[test]
    fn resolve_highest_satisfying_picks_max_common_version() {
        let meta = fake_meta(&["1.0.0", "1.1.0", "1.2.0", "2.0.0"]);
        let specs = vec!["^1.0.0".to_string(), ">=1.1.0, <2.0.0".to_string()];
        let v = resolve_highest_satisfying(&meta, &specs);
        assert_eq!(v.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn resolve_highest_satisfying_returns_none_on_conflict() {
        let meta = fake_meta(&["1.0.0", "1.5.0", "2.0.0"]);
        let specs = vec!["^1.0.0".to_string(), "^2.0.0".to_string()];
        let v = resolve_highest_satisfying(&meta, &specs);
        assert!(v.is_none());
    }
}

#[cfg(test)]
mod pubgrub_fallback_tests {
    use super::*;

    /// A real manifest is required: the fallback deliberately refuses to run for
    /// manifests it cannot read or that are too large (see MAX_FALLBACK_DEPS).
    fn small_manifest() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().expect("tempdir");
        let pj = td.path().join("package.json");
        std::fs::write(
            &pj,
            r#"{"name":"t","version":"1.0.0","dependencies":{"ms":"^2.1.3"}}"#,
        )
        .expect("write package.json");
        (td, pj)
    }

    fn tree(names: &[&str]) -> HashMap<String, ResolvedPackage> {
        names
            .iter()
            .map(|n| {
                (
                    (*n).to_string(),
                    ResolvedPackage {
                        version: "1.0.0".to_string(),
                        resolved: String::new(),
                        integrity: None,
                        dependencies: HashMap::new(),
                        peer_dependencies: HashMap::new(),
                        peer_dependencies_meta: HashMap::new(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn non_empty_pubgrub_result_wins_over_jagr_failure() {
        let (_td, pj) = small_manifest();
        let out =
            with_pubgrub_fallback("jagr exploded".to_string(), &pj, |_| Ok(tree(&["a", "b"])))
                .expect("the fallback must rescue the resolution");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn empty_pubgrub_result_is_not_a_resolution() {
        let (_td, pj) = small_manifest();
        let err = with_pubgrub_fallback("jagr exploded".to_string(), &pj, |_| Ok(HashMap::new()))
            .expect_err("an empty tree must not be accepted");
        assert_eq!(err, "jagr exploded");
    }

    #[test]
    fn both_failing_surfaces_the_primary_error() {
        let (_td, pj) = small_manifest();
        let err = with_pubgrub_fallback("jagr exploded".to_string(), &pj, |_| {
            Err("pubgrub exploded".to_string())
        })
        .expect_err("both failed");
        assert_eq!(err, "jagr exploded", "the primary error must win");
    }

    #[test]
    fn large_manifest_skips_the_fallback_instead_of_hanging() {
        // The pubgrub v2 provider loop is network-bound and unbounded, so the
        // fallback must decline large manifests rather than risk a long hang.
        let td = tempfile::tempdir().unwrap();
        let pj = td.path().join("package.json");
        let deps: Vec<String> = (0..40).map(|i| format!(r#""p{i}":"^1.0.0""#)).collect();
        std::fs::write(
            &pj,
            format!(
                r#"{{"name":"big","version":"1.0.0","dependencies":{{{}}}}}"#,
                deps.join(",")
            ),
        )
        .unwrap();

        let called = std::cell::Cell::new(false);
        let err = with_pubgrub_fallback("jagr exploded".to_string(), &pj, |_| {
            called.set(true);
            Ok(tree(&["a"]))
        })
        .expect_err("must not fall back for a large manifest");
        assert_eq!(err, "jagr exploded");
        assert!(!called.get(), "pubgrub must not even be invoked");
    }
}
