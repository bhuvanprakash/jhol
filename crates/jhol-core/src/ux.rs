//! UX parity helpers: uninstall/update/why.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::lockfile;
use crate::lockfile_write;
use crate::registry;

fn read_root_name_version() -> (String, String) {
    let pj = Path::new("package.json");
    let Ok(s) = fs::read_to_string(pj) else {
        return ("project".to_string(), "0.0.0".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
        return ("project".to_string(), "0.0.0".to_string());
    };
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("project")
        .to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    (name, version)
}

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

/// Import lockfile formats (initially bun.lock -> package-lock.json).
pub fn import_lockfile(from: &str) -> Result<String, String> {
    let cwd = Path::new(".");
    let source = match from {
        "bun" => "bun",
        "npm" => "npm",
        _ => {
            if cwd.join("bun.lock").exists() {
                "bun"
            } else if cwd.join("package-lock.json").exists() {
                "npm"
            } else {
                return Err("No supported lockfile found (bun.lock or package-lock.json)".to_string());
            }
        }
    };

    if source == "npm" {
        if cwd.join("package-lock.json").exists() {
            return Ok("package-lock.json already present; nothing to import".to_string());
        }
        return Err("--from npm is currently a no-op unless package-lock.json exists".to_string());
    }

    let bun = cwd.join("bun.lock");
    if !bun.exists() {
        return Err("bun.lock not found".to_string());
    }
    let resolved = lockfile::read_bun_lock_resolved(&bun)
        .ok_or("Failed to parse bun.lock as JSON lockfile")?;
    if resolved.is_empty() {
        return Err("bun.lock has no resolved packages to import".to_string());
    }

    let deps = lockfile::read_package_json_deps(Path::new("package.json")).unwrap_or_default();
    let (root_name, root_version) = read_root_name_version();

    let mut packages = serde_json::Map::new();
    let mut root_deps = serde_json::Map::new();
    for (name, _spec) in &deps {
        if let Some(ver) = resolved.get(name) {
            root_deps.insert(name.clone(), serde_json::Value::String(ver.clone()));
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

    for (name, version) in &resolved {
        let key = format!("node_modules/{}", name);
        let resolved_url = lockfile::tarball_url_from_registry(name, version);
        packages.insert(
            key,
            serde_json::json!({
                "version": version,
                "resolved": resolved_url,
                "requires": false,
                "dependencies": serde_json::Map::<String, serde_json::Value>::new(),
            }),
        );
    }

    let out = serde_json::json!({
        "name": root_name,
        "version": root_version,
        "lockfileVersion": 3,
        "packages": packages,
    });
    let pretty = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
    fs::write(cwd.join("package-lock.json"), pretty).map_err(|e| e.to_string())?;
    Ok(format!(
        "Imported {} resolved entries from bun.lock into package-lock.json",
        resolved.len()
    ))
}
