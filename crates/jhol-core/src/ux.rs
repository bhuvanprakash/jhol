//! UX parity helpers: uninstall/update/why.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::lockfile;
use crate::lockfile_write;
use crate::registry;

fn read_package_json(path: &Path) -> Result<serde_json::Value, String> {
    let s = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&s).map_err(|e| e.to_string())
}

fn write_package_json(path: &Path, v: &serde_json::Value) -> Result<(), String> {
    let s = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    fs::write(path, s).map_err(|e| e.to_string())
}

fn remove_dep(map: &mut serde_json::Map<String, serde_json::Value>, name: &str) -> bool {
    map.remove(name).is_some()
}

/// Uninstall package: remove from node_modules and (optionally) package.json.
pub fn uninstall(package: &str, update_package_json: bool) -> Result<(), String> {
    let pkg = package.trim();
    if pkg.is_empty() {
        return Err("Package name required".to_string());
    }
    let node_modules = Path::new("node_modules");
    if node_modules.exists() {
        let path = if pkg.starts_with('@') {
            let parts: Vec<&str> = pkg.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid scoped package: {}", pkg));
            }
            node_modules.join(parts[0]).join(parts[1])
        } else {
            node_modules.join(pkg)
        };
        if path.exists() {
            fs::remove_dir_all(&path).or_else(|_| fs::remove_file(&path)).map_err(|e| e.to_string())?;
        }
    }

    if update_package_json {
        let pj = Path::new("package.json");
        if !pj.exists() {
            return Err("No package.json found in current directory".to_string());
        }
        let mut v = read_package_json(pj)?;
        let mut changed = false;
        if let Some(deps) = v.get_mut("dependencies").and_then(|d| d.as_object_mut()) {
            if remove_dep(deps, pkg) {
                changed = true;
            }
        }
        if let Some(deps) = v.get_mut("devDependencies").and_then(|d| d.as_object_mut()) {
            if remove_dep(deps, pkg) {
                changed = true;
            }
        }
        if let Some(deps) = v.get_mut("peerDependencies").and_then(|d| d.as_object_mut()) {
            if remove_dep(deps, pkg) {
                changed = true;
            }
        }
        if changed {
            write_package_json(pj, &v)?;
        }
    }
    Ok(())
}

/// Update lockfile to latest versions (optionally for specific package names).
pub fn update_packages(packages: &[String]) -> Result<(), String> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err("No package.json found in current directory".to_string());
    }
    if packages.is_empty() {
        let tree = lockfile_write::resolve_full_tree(pj)?;
        let lock_path = Path::new("package-lock.json");
        lockfile_write::write_package_lock(lock_path, pj, &tree)?;
        return Ok(());
    }
    let mut v = read_package_json(pj)?;
    let mut deps_map: HashMap<String, String> = HashMap::new();
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(deps) = v.get(key).and_then(|d| d.as_object()) {
            for (name, spec) in deps {
                if let Some(s) = spec.as_str() {
                    deps_map.insert(name.clone(), s.to_string());
                }
            }
        }
    }
    let mut changed = false;
    for pkg in packages {
        if !deps_map.contains_key(pkg) {
            continue;
        }
        let meta = registry::fetch_metadata(pkg)?;
        let version = registry::resolve_version(&meta, "latest")
            .ok_or_else(|| format!("Could not resolve latest for {}", pkg))?;
        for key in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(deps) = v.get_mut(key).and_then(|d| d.as_object_mut()) {
                if deps.contains_key(pkg) {
                    deps.insert(pkg.clone(), serde_json::Value::String(format!("^{}", version)));
                    changed = true;
                }
            }
        }
    }
    if changed {
        write_package_json(pj, &v)?;
    }
    let tree = lockfile_write::resolve_full_tree(pj)?;
    let lock_path = Path::new("package-lock.json");
    lockfile_write::write_package_lock(lock_path, pj, &tree)?;
    Ok(())
}

/// Explain why a package exists: show paths from root to target via lockfile dependencies.
pub fn why_package(package: &str) -> Result<Vec<String>, String> {
    let dir = Path::new(".");
    let resolved = lockfile::read_resolved_from_dir(dir)
        .ok_or("No lockfile found (package-lock.json or bun.lock)")?;
    if !resolved.contains_key(package) {
        return Err(format!("{} not found in lockfile", package));
    }
    let lock_path = if dir.join("package-lock.json").exists() {
        dir.join("package-lock.json")
    } else {
        return Err("why only supported for package-lock.json currently".to_string());
    };
    let s = fs::read_to_string(&lock_path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    let packages = v.get("packages").and_then(|p| p.as_object()).ok_or("Invalid lockfile")?;

    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for (key, val) in packages {
        let name = key.trim_start_matches("node_modules/");
        if name.is_empty() {
            continue;
        }
        let mut deps = Vec::new();
        if let Some(dep_obj) = val.get("dependencies").and_then(|d| d.as_object()) {
            for dep in dep_obj.keys() {
                deps.push(dep.to_string());
            }
        }
        edges.insert(name.to_string(), deps);
    }

    let mut results = Vec::new();
    let mut stack: Vec<(String, Vec<String>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (root_dep, _) in lockfile::read_package_json_deps(Path::new("package.json")).unwrap_or_default() {
        stack.push((root_dep.clone(), vec![root_dep.clone()]));
    }
    while let Some((current, path)) = stack.pop() {
        if current == package {
            results.push(path.join(" -> "));
            continue;
        }
        if !seen.insert(current.clone()) {
            continue;
        }
        if let Some(children) = edges.get(&current) {
            for child in children {
                let mut next_path = path.clone();
                next_path.push(child.clone());
                stack.push((child.clone(), next_path));
            }
        }
    }
    if results.is_empty() {
        results.push(format!("{} is a direct dependency or resolved by lockfile", package));
    }
    Ok(results)
}
