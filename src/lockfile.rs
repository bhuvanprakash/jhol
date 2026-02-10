//! Read package-lock.json for resolved versions (deterministic installs).

use std::collections::HashMap;
use std::path::Path;

/// Read package.json and return (dependencies, devDependencies) as name -> version spec.
pub fn read_package_json_deps(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let mut deps = HashMap::new();
    if let Some(d) = v.get("dependencies").and_then(|d| d.as_object()) {
        for (k, v) in d {
            if let Some(s) = v.as_str() {
                deps.insert(k.clone(), s.to_string());
            }
        }
    }
    if let Some(d) = v.get("devDependencies").and_then(|d| d.as_object()) {
        for (k, v) in d {
            if let Some(s) = v.as_str() {
                deps.insert(k.clone(), s.to_string());
            }
        }
    }
    Some(deps)
}

/// Read package-lock.json and return resolved versions: package name -> exact version.
/// Supports lockfileVersion 2 and 3 (packages key).
pub fn read_lockfile_resolved(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut resolved = HashMap::new();
    for (key, val) in packages {
        let version = val.get("version")?.as_str()?;
        // key is "" for root, "node_modules/foo" or "node_modules/@scope/foo"
        let name = key.trim_start_matches("node_modules/");
        if name.is_empty() {
            continue;
        }
        resolved.insert(name.to_string(), version.to_string());
    }
    Some(resolved)
}

/// Merge package.json deps with lockfile: for each dep, use lockfile version if present.
/// Returns list of "name@version" for install.
pub fn resolve_deps_for_install(
    package_json_deps: &HashMap<String, String>,
    lockfile_resolved: Option<&HashMap<String, String>>,
) -> Vec<String> {
    let mut out = Vec::with_capacity(package_json_deps.len());
    for (name, spec) in package_json_deps {
        let version = lockfile_resolved
            .and_then(|r| r.get(name).cloned())
            .unwrap_or_else(|| spec.clone());
        out.push(format!("{}@{}", name, version));
    }
    out
}
