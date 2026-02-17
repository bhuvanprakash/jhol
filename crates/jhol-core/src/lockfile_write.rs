//! Native lockfile writing: resolve dependency tree and emit package-lock.json.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::lockfile;
use crate::registry;
use crate::sat_resolver::{PackageDomain, PackageVersion, SolveInput};

/// One resolved package entry for the lockfile.
#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub version: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub peer_dependencies: HashMap<String, String>,
    pub peer_dependencies_meta: HashMap<String, serde_json::Value>,
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

/// Resolve the full dependency tree from package.json with deterministic conflict handling.
/// Prefetches direct deps in parallel, then uses cached packuments for the rest.
/// For conflicts, prefers the highest version that satisfies the combined specs; errors if none match.
pub fn resolve_full_tree(package_json_path: &Path) -> Result<HashMap<String, ResolvedPackage>, String> {
    // JAGR-1 is the default resolver strategy. Legacy greedy remains as fallback for safety.
    let strict_jagr = std::env::var("JHOL_RESOLVER_STRICT")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "True"))
        .unwrap_or(false);
    if std::env::var("JHOL_RESOLVER")
        .map(|v| v.eq_ignore_ascii_case("legacy"))
        .unwrap_or(false)
    {
        return resolve_full_tree_legacy(package_json_path);
    }
    match resolve_full_tree_jagr(package_json_path) {
        Ok(tree) => Ok(tree),
        Err(jagr_err) => {
            if strict_jagr {
                return Err(jagr_err);
            }
            eprintln!("warning: JAGR resolver failed, falling back to legacy: {}", jagr_err);
            resolve_full_tree_legacy(package_json_path)
                .map_err(|legacy_err| format!("JAGR failed: {}; legacy failed: {}", jagr_err, legacy_err))
        }
    }
}

fn resolve_full_tree_jagr(package_json_path: &Path) -> Result<HashMap<String, ResolvedPackage>, String> {
    let deps = lockfile::read_package_json_deps(package_json_path)
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
    loop {
        let (domains, truncated_any) = build_jagr_domains(&deps, &cache_arc, cap)?;
        match crate::sat_resolver::solve_exact_with_stats(&input, &domains) {
            Ok((solved, _stats)) => return build_tree_from_assignment(&solved.assignment, &cache_arc),
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

fn build_jagr_domains(
    root_deps: &HashMap<String, String>,
    cache_arc: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
    cap: usize,
) -> Result<(HashMap<String, PackageDomain>, bool), String> {

    let mut domains: HashMap<String, PackageDomain> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
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

        for (name, meta_res) in results {
            visited.insert(name.clone());
            let meta = meta_res?;

            let mut domain = PackageDomain::default();
            let (versions, truncated) = candidate_versions_desc(&meta, cap);
            truncated_any |= truncated;
            for version in versions {
                let deps = registry::get_version_required_dependencies(&meta, &version);
                let optional_deps = registry::get_version_optional_dependencies(&meta, &version);
                let peers = registry::get_version_peer_dependencies(&meta, &version);
                let peer_meta = registry::get_version_peer_dependencies_meta(&meta, &version);
                let optional_peers: HashSet<String> = peer_meta
                    .iter()
                    .filter_map(|(k, v)| {
                        let opt = v
                            .get("optional")
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false);
                        if opt { Some(k.clone()) } else { None }
                    })
                    .collect();

                for dep_name in deps.keys() {
                    if !visited.contains(dep_name) {
                        frontier.push(dep_name.clone());
                    }
                }
                for peer_name in peers.keys() {
                    if !optional_peers.contains(peer_name) && !visited.contains(peer_name) {
                        frontier.push(peer_name.clone());
                    }
                }

                domain.versions.insert(
                    version.clone(),
                    PackageVersion {
                        version,
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

fn candidate_versions_desc(meta: &serde_json::Value, cap: usize) -> (Vec<String>, bool) {
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
    let truncated = parsed.len() > cap;
    let versions = parsed.into_iter().take(cap).map(|v| v.to_string()).collect();
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
            let mut cache = cache_arc.lock().unwrap();
            registry::fetch_metadata_cached(&name, &mut *cache)?
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

fn resolve_full_tree_legacy(package_json_path: &Path) -> Result<HashMap<String, ResolvedPackage>, String> {
    let deps = lockfile::read_package_json_deps(package_json_path)
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
            let mut cache = cache_arc.lock().unwrap();
            registry::fetch_metadata_cached(&name, &mut *cache)?
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
            format!("Dependency conflict for {} (no version satisfies all): {}", name, refs)
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
                let mut cache = cache_arc.lock().unwrap();
                match registry::fetch_metadata_cached(dep_name, &mut *cache) {
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
            if let Some(dep_version) = resolve_highest_satisfying(&dep_meta, &[dep_spec.clone()]) {
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
            key,
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
            let required_reqs: Vec<&PeerRequirement> = reqs.iter().filter(|r| !r.optional).collect();
            if required_reqs.is_empty() {
                continue;
            }
            let req_list = required_reqs
                .iter()
                .map(|r| format!("{} -> {}", r.requester, r.spec))
                .collect::<Vec<_>>()
                .join(", ");
            peer_conflicts.push(format!("peer {} missing (requirements: {})", peer_name, req_list));
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
        if specs.iter().all(|s| registry::version_satisfies(s, &ver_str)) {
            return Some(ver_str);
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
fn build_packages_json(
    root_name: &str,
    root_version: &str,
    direct_dep_names: &[String],
    tree: &HashMap<String, ResolvedPackage>,
) -> serde_json::Value {
    let mut packages = serde_json::Map::new();

    let mut root_deps = serde_json::Map::new();
    for name in direct_dep_names {
        let key = format!("node_modules/{}", name);
        if let Some(pkg) = tree.get(&key) {
            root_deps.insert(name.clone(), serde_json::Value::String(pkg.version.clone()));
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

    for (key, pkg) in tree {
        let mut entry = serde_json::Map::new();
        entry.insert("version".to_string(), serde_json::Value::String(pkg.version.clone()));
        entry.insert("resolved".to_string(), serde_json::Value::String(pkg.resolved.clone()));
        if let Some(ref i) = pkg.integrity {
            entry.insert("integrity".to_string(), serde_json::Value::String(i.clone()));
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
        entry.insert("requires".to_string(), serde_json::Value::Bool(!pkg.dependencies.is_empty()));
        if !peer_deps.is_empty() {
            entry.insert("peerDependencies".to_string(), serde_json::Value::Object(peer_deps));
        }
        if !peer_deps_meta.is_empty() {
            entry.insert("peerDependenciesMeta".to_string(), serde_json::Value::Object(peer_deps_meta));
        }
        packages.insert(key.clone(), serde_json::Value::Object(entry));
    }

    serde_json::Value::Object(packages)
}

/// Write package-lock.json to the given path.
pub fn write_package_lock(
    lock_path: &Path,
    package_json_path: &Path,
    tree: &HashMap<String, ResolvedPackage>,
) -> Result<(), String> {
    let (root_name, root_version) = read_root_package_info(package_json_path)?;
    let deps = lockfile::read_package_json_deps(package_json_path).unwrap_or_default();
    let direct_dep_names: Vec<String> = deps.keys().cloned().collect();

    let packages = build_packages_json(&root_name, &root_version, &direct_dep_names, tree);

    let lockfile_content = serde_json::json!({
        "name": root_name,
        "version": root_version,
        "lockfileVersion": 3,
        "packages": packages,
    });

    let pretty = serde_json::to_string_pretty(&lockfile_content).map_err(|e| e.to_string())?;
    std::fs::write(lock_path, pretty).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
        let specs = vec!["^1.0.0".to_string(), ">=1.1.0 <2.0.0".to_string()];
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
