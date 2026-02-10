//! Read package-lock.json and bun.lock for resolved versions (deterministic installs).

use std::collections::HashMap;
use std::path::Path;

/// Lockfile kind detected in a directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockfileKind {
    None,
    Npm,
    Bun,
}

/// Return which lockfile is present in the given directory (package-lock.json or bun.lock).
pub fn detect_lockfile(dir: &Path) -> LockfileKind {
    if dir.join("bun.lock").exists() {
        LockfileKind::Bun
    } else if dir.join("package-lock.json").exists() {
        LockfileKind::Npm
    } else {
        LockfileKind::None
    }
}

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

/// Read bun.lock (text JSON format) and return resolved versions: package name -> exact version.
/// Bun lockfile has "packages" object; keys can be "npm:name@version" or "name@version".
pub fn read_bun_lock_resolved(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut resolved = HashMap::new();
    for (key, _val) in packages {
        let rest = key.strip_prefix("npm:").unwrap_or(key);
        // Scoped: @scope/pkg@1.0.0 -> name = @scope/pkg, version = 1.0.0 (rfind from left of last @)
        let at_pos = rest.rfind('@')?;
        if at_pos == 0 {
            continue; // @ at start is scope, need another @
        }
        let name = rest[..at_pos].to_string();
        let version = rest[at_pos + 1..].to_string();
        if !version.is_empty() && !name.is_empty() {
            resolved.insert(name, version);
        }
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

/// Read resolved versions from whichever lockfile exists in dir (package-lock.json or bun.lock).
pub fn read_resolved_from_dir(dir: &Path) -> Option<HashMap<String, String>> {
    let bun_lock = dir.join("bun.lock");
    let npm_lock = dir.join("package-lock.json");
    if bun_lock.exists() {
        read_bun_lock_resolved(&bun_lock)
    } else if npm_lock.exists() {
        read_lockfile_resolved(&npm_lock)
    } else {
        None
    }
}
