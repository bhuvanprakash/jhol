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

/// Read package-lock.json and return map name@version -> resolved tarball URL (for zero-packument install).
pub fn read_lockfile_resolved_urls(path: &Path) -> Option<HashMap<String, String>> {
    read_lockfile_resolved_urls_with_integrity(path).map(|(urls, _)| urls)
}

/// Read package-lock.json and return (urls, integrity) maps. Integrity key is name@version -> SRI string.
pub fn read_lockfile_resolved_urls_with_integrity(
    path: &Path,
) -> Option<(HashMap<String, String>, HashMap<String, String>)> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut urls = HashMap::new();
    let mut integrity = HashMap::new();
    for (key, val) in packages {
        let name = key.trim_start_matches("node_modules/");
        if name.is_empty() {
            continue;
        }
        let version = val.get("version")?.as_str()?;
        let resolved = val.get("resolved")?.as_str()?;
        if resolved.ends_with(".tgz") {
            let pkg_key = format!("{}@{}", name, version);
            urls.insert(pkg_key.clone(), resolved.to_string());
            if let Some(sri) = val.get("integrity").and_then(|i| i.as_str()) {
                integrity.insert(pkg_key, sri.to_string());
            }
        }
    }
    Some((urls, integrity))
}

/// Build npm registry tarball URL for a package version (no packument needed).
/// Scoped: @scope/pkg -> https://registry.npmjs.org/@scope%2Fpkg/-/pkg-1.0.0.tgz
pub fn tarball_url_from_registry(name: &str, version: &str) -> String {
    const REGISTRY: &str = "https://registry.npmjs.org";
    let encoded = if name.starts_with('@') {
        name.replace('/', "%2F")
    } else {
        name.to_string()
    };
    let tarball_name = if name.starts_with('@') {
        name.split('/').last().unwrap_or(name).to_string()
    } else {
        name.to_string()
    };
    format!(
        "{}/{}/-/{}-{}.tgz",
        REGISTRY.trim_end_matches('/'),
        encoded,
        tarball_name,
        version
    )
}

/// Read resolved tarball URLs from dir: package-lock has "resolved"; bun.lock we build via tarball_url_from_registry.
pub fn read_resolved_urls_from_dir(dir: &Path) -> Option<HashMap<String, String>> {
    read_resolved_urls_and_integrity_from_dir(dir).map(|(urls, _)| urls)
}

/// Read resolved URLs and integrity (when available, e.g. package-lock.json). For bun.lock, integrity is empty.
pub fn read_resolved_urls_and_integrity_from_dir(
    dir: &Path,
) -> Option<(HashMap<String, String>, HashMap<String, String>)> {
    let npm_lock = dir.join("package-lock.json");
    let bun_lock = dir.join("bun.lock");
    if npm_lock.exists() {
        return read_lockfile_resolved_urls_with_integrity(&npm_lock);
    }
    if bun_lock.exists() {
        let resolved = read_bun_lock_resolved(&bun_lock)?;
        let mut urls = HashMap::new();
        for (name, version) in resolved {
            let spec = format!("{}@{}", name, version);
            urls.insert(spec, tarball_url_from_registry(&name, &version));
        }
        return Some((urls, HashMap::new()));
    }
    None
}

/// Check whether all resolved lockfile entries include integrity strings (package-lock.json only).
pub fn lockfile_integrity_complete(dir: &Path) -> bool {
    let npm_lock = dir.join("package-lock.json");
    if !npm_lock.exists() {
        return true;
    }
    let Ok(s) = std::fs::read_to_string(&npm_lock) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
        return false;
    };
    let Some(packages) = v.get("packages").and_then(|p| p.as_object()) else {
        return false;
    };
    for (key, val) in packages {
        let name = key.trim_start_matches("node_modules/");
        if name.is_empty() {
            continue;
        }
        if val.get("integrity").and_then(|i| i.as_str()).is_none() {
            return false;
        }
    }
    true
}

/// Read all resolved specs (name@version) from lockfile in dir.
/// For package-lock.json: uses `packages` entries with `version`.
/// For bun.lock: uses parsed resolved name/version pairs.
pub fn read_all_resolved_specs_from_dir(dir: &Path) -> Option<Vec<String>> {
    let npm_lock = dir.join("package-lock.json");
    let bun_lock = dir.join("bun.lock");
    if npm_lock.exists() {
        let resolved = read_lockfile_resolved(&npm_lock)?;
        let mut specs: Vec<String> = resolved
            .into_iter()
            .map(|(name, version)| format!("{}@{}", name, version))
            .collect();
        specs.sort();
        specs.dedup();
        return Some(specs);
    }
    if bun_lock.exists() {
        let resolved = read_bun_lock_resolved(&bun_lock)?;
        let mut specs: Vec<String> = resolved
            .into_iter()
            .map(|(name, version)| format!("{}@{}", name, version))
            .collect();
        specs.sort();
        specs.dedup();
        return Some(specs);
    }
    None
}
