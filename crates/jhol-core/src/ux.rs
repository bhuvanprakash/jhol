//! UX parity helpers: uninstall/update/why.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::lockfile;
use crate::lockfile_write;
use crate::registry;
use crate::utils;

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
/// Remove a package from `node_modules` and (optionally) from `package.json`.
///
/// Returns whether anything actually changed: `remove not-a-package` used to print
/// "Package(s) removed." having touched nothing, which is the kind of success message
/// that makes a wrong command look right.
pub fn uninstall(package: &str, update_package_json: bool) -> Result<bool, String> {
    let pkg = package.trim();
    if pkg.is_empty() {
        return Err("Package name required".to_string());
    }
    let pkg = utils::sanitize_package_name(pkg)
        .ok_or_else(|| format!("Invalid package name: {}", package))?;
    let pkg_for_path = pkg.clone();
    let mut removed_from_disk = false;
    let node_modules = Path::new("node_modules");
    if node_modules.exists() {
        let path = if pkg_for_path.starts_with('@') {
            let parts: Vec<&str> = pkg_for_path.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid scoped package: {}", pkg_for_path));
            }
            node_modules.join(parts[0]).join(parts[1])
        } else {
            node_modules.join(&pkg_for_path)
        };
        if path.exists() {
            fs::remove_dir_all(&path)
                .or_else(|_| fs::remove_file(&path))
                .map_err(|e| e.to_string())?;
            removed_from_disk = true;
        }
    }

    let mut changed = false;
    if update_package_json {
        let pj = Path::new("package.json");
        if !pj.exists() {
            return Err("No package.json found in current directory".to_string());
        }
        let mut v = read_package_json(pj)?;
        if let Some(deps) = v.get_mut("dependencies").and_then(|d| d.as_object_mut()) {
            if remove_dep(deps, &pkg) {
                changed = true;
            }
        }
        if let Some(deps) = v.get_mut("devDependencies").and_then(|d| d.as_object_mut()) {
            if remove_dep(deps, &pkg) {
                changed = true;
            }
        }
        if let Some(deps) = v
            .get_mut("peerDependencies")
            .and_then(|d| d.as_object_mut())
        {
            if remove_dep(deps, &pkg) {
                changed = true;
            }
        }
        if changed {
            write_package_json(pj, &v)?;
        }
    }
    // Regenerate the lockfile from the current node_modules state. The
    // lockfile is a derived view of what's installed; after removing a
    // package from disk, the lockfile must drop it too or `jhol ci` will
    // try to re-download the removed package. The lockfile writer is an
    // incremental updater that merges with the existing file — to get a
    // faithful view of the current node_modules, we delete the old lockfile
    // first so the updater builds it from scratch from the tree.
    let lock_path = Path::new("package-lock.json");
    let pj_path = Path::new("package.json");
    if lock_path.exists() && pj_path.exists() {
        if let Ok(packages) = crate::install::read_all_installed_packages(Path::new("node_modules"))
        {
            // Remove the old lockfile so the incremental updater builds a
            // fresh one from the tree (otherwise it merges and keeps the
            // removed package's entry).
            let _ = std::fs::remove_file(lock_path);
            let _ = crate::lockfile_write::write_package_lock(lock_path, pj_path, &packages);
            // Restore the lockfile path for any subsequent operations.
        }
    }
    // Rebuild `.bin` so the removed package's shims are pruned with it. Skipping
    // this left `node_modules/.bin/<tool>` pointing at a deleted file, and running
    // it failed with `cannot execute: No such file or directory`; npm removes the
    // entry on uninstall. `rebuild_bin_links` now also drops dangling shims.
    let _ = crate::bin_links::rebuild_bin_links(node_modules);
    Ok(removed_from_disk || changed)

}

/// Update lockfile to latest versions (optionally for specific package names).
/// Update dependencies **within the ranges `package.json` declares**, then make the tree
/// match.
///
/// What this used to do, and must not: resolve each named package at `latest`, write
/// `^<latest>` into the manifest — turning a deliberate `4.17.1` pin into `^5.3.0` — and
/// refresh the lockfile while installing nothing. It printed "Update complete." over a tree
/// that was still the old version and a lockfile that described the new one, so the next
/// `verify` failed.
///
/// The contract now: the manifest is the range authority. `update` re-resolves inside those
/// ranges (newest satisfying), rewrites the lockfile, and materialises the result. Moving a
/// package to a new major is `jhol install <pkg>@latest`, which says so.
pub fn update_packages(packages: &[String]) -> Result<(), String> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err("No package.json found in current directory".to_string());
    }
    if !packages.is_empty() {
        let v = read_package_json(pj)?;
        let declared: Vec<String> = ["dependencies", "devDependencies", "peerDependencies"]
            .iter()
            .filter_map(|k| v.get(k).and_then(|d| d.as_object()))
            .flat_map(|d| d.keys().cloned())
            .collect();
        for p in packages {
            if !declared.iter().any(|d| d == p) {
                return Err(format!(
                    "{} is not a dependency in package.json: this command only updates what the \
                     manifest already asks for, use `jhol install {}@latest` to change the range",
                    p, p
                ));
            }
        }
    }

    // Re-resolve from the manifest (newest satisfying each declared range), then install
    // it. Writing a lockfile that describes something other than the tree on disk is the
    // defect this function shipped with.
    let tree = lockfile_write::resolve_full_tree_nested(pj)?;
    lockfile_write::write_package_lock(Path::new("package-lock.json"), pj, &tree)?;
    let specs = crate::install::resolve_install_from_package_json_omit(
        false,
        crate::omit::Omit::NONE.with_env(false),
    )
    .map_err(|e| e.to_string())?;
    let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
    // Materialise from the lockfile just written, exactly as `ci` does: the lockfile is
    // the authority for *paths* (nested instances included), so installing from the
    // manifest's ranges instead loses the nested copies and leaves the tree short.
    let opts = crate::install::InstallOptions {
        from_lockfile: true,
        native_only: true,
        no_scripts: true,
        ..Default::default()
    };
    crate::install::install_package(&refs, &opts).map_err(|e| e.to_string())?;
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
    let packages = v
        .get("packages")
        .and_then(|p| p.as_object())
        .ok_or("Invalid lockfile")?;

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
    for (root_dep, _) in
        lockfile::read_package_json_deps(Path::new("package.json")).unwrap_or_default()
    {
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
        results.push(format!(
            "{} is a direct dependency or resolved by lockfile",
            package
        ));
    }
    Ok(results)
}

/// Import lockfile formats (initially bun.lock -> package-lock.json).
pub fn import_lockfile(from: &str) -> Result<String, String> {
    let cwd = Path::new(".");
    let source = match from {
        "bun" => "bun",
        "npm" => "npm",
        // Be explicit rather than searching for a different lockfile and reporting
        // that none was found: the user named a format we do not read yet.
        "yarn" => "yarn",
        "pnpm" => "pnpm",
        _ => {
            if cwd.join("bun.lock").exists() {
                "bun"
            } else if cwd.join("package-lock.json").exists() {
                "npm"
            } else if cwd.join("yarn.lock").exists() {
                "yarn"
            } else if cwd.join("pnpm-lock.yaml").exists() {
                "pnpm"
            } else {
                return Err(
                    "No supported lockfile found (package-lock.json, bun.lock, yarn.lock, \
                     pnpm-lock.yaml)"
                        .to_string(),
                );
            }
        }
    };

    if source == "npm" {
        if cwd.join("package-lock.json").exists() {
            return Ok("package-lock.json already present; nothing to import".to_string());
        }
        return Err("--from npm is currently a no-op unless package-lock.json exists".to_string());
    }

    // Both bun.lock (JSONC) and yarn.lock (v1 / Berry) reduce to `name -> version`.
    let (lock_path, source_label) = match source {
        "bun" => (cwd.join("bun.lock"), "bun.lock"),
        "yarn" => (cwd.join("yarn.lock"), "yarn.lock"),
        _ => (cwd.join("pnpm-lock.yaml"), "pnpm-lock.yaml"),
    };
    if !lock_path.exists() {
        return Err(format!("{} not found", source_label));
    }
    let resolved = match source {
        "bun" => lockfile::read_bun_lock_resolved(&lock_path)
            .ok_or("Failed to parse bun.lock as JSON lockfile")?,
        "yarn" => {
            lockfile::read_yarn_lock_resolved(&lock_path).ok_or("Failed to parse yarn.lock")?
        }
        _ => {
            lockfile::read_pnpm_lock_resolved(&lock_path).ok_or("Failed to parse pnpm-lock.yaml")?
        }
    };
    if resolved.is_empty() {
        return Err(format!(
            "{} has no resolved packages to import",
            source_label
        ));
    }

    let deps = lockfile::read_package_json_deps(Path::new("package.json")).unwrap_or_default();
    let (root_name, root_version) = read_root_name_version();

    let mut packages = serde_json::Map::new();
    let mut root_deps = serde_json::Map::new();
    for name in deps.keys() {
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
        "Imported {} resolved entries from {} into package-lock.json",
        resolved.len(),
        source_label
    ))
}
