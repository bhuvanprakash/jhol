//! Native lockfile writing: resolve dependency tree and emit package-lock.json.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::lockfile;
use crate::registry;

/// One resolved package entry for the lockfile.
#[derive(Clone, Debug)]
pub struct ResolvedPackage {
    pub version: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub dependencies: HashMap<String, String>,
}

#[derive(Clone, Debug)]
struct PeerRequirement {
    requester: String,
    spec: String,
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

        let version_deps = registry::get_version_dependencies(&meta, &version);
        let peer_deps = registry::get_version_peer_dependencies(&meta, &version);
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
            peer_requirements
                .entry(peer_name.clone())
                .or_default()
                .push(PeerRequirement {
                    requester: name.clone(),
                    spec: peer_spec.clone(),
                });
        }

        tree.insert(
            key,
            ResolvedPackage {
                version,
                resolved: resolved_url,
                integrity,
                dependencies: resolved_deps,
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
            let req_list = reqs
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
        entry.insert("dependencies".to_string(), serde_json::Value::Object(deps));
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
