//! Read package-lock.json and bun.lock for resolved versions (deterministic installs).

use std::collections::HashMap;
use std::path::Path;

fn package_name_from_lockfile_key(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }

    let segments: Vec<&str> = key
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return None;
    }

    // npm lockfiles can use either `node_modules/<pkg>` keys or plain package keys.
    let tail = if let Some(nm_idx) = segments.iter().rposition(|seg| *seg == "node_modules") {
        &segments[nm_idx + 1..]
    } else {
        &segments[..]
    };
    if tail.is_empty() {
        return None;
    }

    let first = tail[0];
    let name = if first.starts_with('@') {
        let second = tail.get(1)?;
        format!("{}/{}", first, second)
    } else if tail.len() == 1 {
        first.to_string()
    } else {
        // Unrecognized non-scoped nested path without node_modules marker.
        return None;
    };

    if name.is_empty() { None } else { Some(name) }
}

#[derive(Clone, Debug)]
pub struct LockfileInstallEntry {
    pub package: String,
    pub version: String,
    pub spec: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub install_path: String,
    pub top_level: bool,
}

fn is_safe_lockfile_path(raw: &str) -> bool {
    // Lockfile keys are logical paths; normalize extra slashes before safety checks.
    let normalized = raw.trim_matches('/');
    if normalized.is_empty() {
        return false;
    }
    let p = std::path::Path::new(normalized);
    !p.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

fn is_top_level_lockfile_path(path: &str) -> bool {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return false;
    }
    if !trimmed.starts_with("node_modules/") {
        // Plain package keys in lockfile v3 are top-level by definition.
        if trimmed.starts_with('@') {
            return trimmed.split('/').filter(|seg| !seg.is_empty()).count() == 2;
        }
        return !trimmed.contains('/');
    }
    let rest = &trimmed["node_modules/".len()..];
    !rest.contains("/node_modules/")
}

fn normalize_lock_install_path(raw: &str) -> String {
    raw.trim_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn current_npm_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn current_npm_cpu() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" | "i386" | "i686" => "ia32",
        "aarch64" => "arm64",
        other => other,
    }
}

fn field_allows_current(value: Option<&serde_json::Value>, current: &str) -> bool {
    let Some(value) = value else { return true; };
    let Some(list) = value.as_array() else {
        return value.as_str().map(|s| s == current).unwrap_or(true);
    };

    let mut positives = Vec::new();
    for entry in list {
        let Some(raw) = entry.as_str() else { continue; };
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(neg) = token.strip_prefix('!') {
            if neg == current {
                return false;
            }
        } else {
            positives.push(token);
        }
    }

    positives.is_empty() || positives.iter().any(|p| *p == current)
}

fn package_supported_on_current_platform(val: &serde_json::Value) -> bool {
    field_allows_current(val.get("os"), current_npm_os())
        && field_allows_current(val.get("cpu"), current_npm_cpu())
}

/// Lockfile kind detected in a directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockfileKind {
    None,
    NpmShrinkwrap,
    Npm,
    Bun,
}

/// Return which lockfile is present in the given directory (package-lock.json or bun.lock).
pub fn detect_lockfile(dir: &Path) -> LockfileKind {
    if dir.join("bun.lock").exists() {
        LockfileKind::Bun
    } else if dir.join("npm-shrinkwrap.json").exists() {
        LockfileKind::NpmShrinkwrap
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
    if let Some(d) = v.get("optionalDependencies").and_then(|d| d.as_object()) {
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
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        resolved.insert(name, version.to_string());
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
    let mut names: Vec<&String> = package_json_deps.keys().collect();
    names.sort();

    let mut out = Vec::with_capacity(package_json_deps.len());
    for name in names {
        let spec = package_json_deps.get(name).cloned().unwrap_or_default();
        let version = lockfile_resolved
            .and_then(|r| r.get(name).cloned())
            .unwrap_or(spec);
        out.push(format!("{}@{}", name, version));
    }
    out
}

/// Read resolved versions from whichever lockfile exists in dir (package-lock.json or bun.lock).
pub fn read_resolved_from_dir(dir: &Path) -> Option<HashMap<String, String>> {
    let bun_lock = dir.join("bun.lock");
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    let npm_lock = dir.join("package-lock.json");
    if bun_lock.exists() {
        read_bun_lock_resolved(&bun_lock)
    } else if npm_shrinkwrap.exists() {
        read_lockfile_resolved(&npm_shrinkwrap)
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
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
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
    let registry = crate::config::effective_registry_url(Path::new("."));
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
        registry.trim_end_matches('/'),
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
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    let npm_lock = dir.join("package-lock.json");
    let bun_lock = dir.join("bun.lock");
    if npm_shrinkwrap.exists() {
        return read_lockfile_resolved_urls_with_integrity(&npm_shrinkwrap);
    }
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
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    let npm_lock = dir.join("package-lock.json");
    let lock_path = if npm_shrinkwrap.exists() {
        npm_shrinkwrap
    } else {
        npm_lock
    };
    if !lock_path.exists() {
        return true;
    }
    let Ok(s) = std::fs::read_to_string(&lock_path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
        return false;
    };
    let Some(packages) = v.get("packages").and_then(|p| p.as_object()) else {
        return false;
    };
    for (key, val) in packages {
        if package_name_from_lockfile_key(key).is_none() {
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
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    let npm_lock = dir.join("package-lock.json");
    let bun_lock = dir.join("bun.lock");
    if npm_shrinkwrap.exists() {
        let resolved = read_lockfile_resolved(&npm_shrinkwrap)?;
        let mut specs: Vec<String> = resolved
            .into_iter()
            .map(|(name, version)| format!("{}@{}", name, version))
            .collect();
        specs.sort();
        specs.dedup();
        return Some(specs);
    }
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


pub fn read_npm_lock_install_entries(path: &Path) -> Option<Vec<LockfileInstallEntry>> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut entries = Vec::new();

    for (key, val) in packages {
        if key.is_empty() {
            continue;
        }
        if !is_safe_lockfile_path(key) {
            continue;
        }
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        let version = val.get("version")?.as_str()?.to_string();
        if !package_supported_on_current_platform(val)
            && val.get("optional").and_then(|o| o.as_bool()).unwrap_or(false)
        {
            continue;
        }
        let resolved = val
            .get("resolved")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| tarball_url_from_registry(&name, &version));
        let integrity = val.get("integrity").and_then(|i| i.as_str()).map(|s| s.to_string());
        let install_path = normalize_lock_install_path(key);

        entries.push(LockfileInstallEntry {
            package: name.clone(),
            version: version.clone(),
            spec: format!("{}@{}", name, version),
            resolved,
            integrity,
            install_path: install_path.clone(),
            top_level: is_top_level_lockfile_path(&install_path),
        });
    }

    entries.sort_by(|a, b| a.install_path.cmp(&b.install_path).then(a.package.cmp(&b.package)).then(a.version.cmp(&b.version)));
    Some(entries)
}

pub fn read_lockfile_install_entries_from_dir(dir: &Path) -> Option<Vec<LockfileInstallEntry>> {
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    if npm_shrinkwrap.exists() {
        return read_npm_lock_install_entries(&npm_shrinkwrap);
    }
    let npm_lock = dir.join("package-lock.json");
    if npm_lock.exists() {
        return read_npm_lock_install_entries(&npm_lock);
    }

    let bun_lock = dir.join("bun.lock");
    if bun_lock.exists() {
        let resolved = read_bun_lock_resolved(&bun_lock)?;
        let mut entries = Vec::new();
        for (name, version) in resolved {
            let top = format!("node_modules/{}", name);
            entries.push(LockfileInstallEntry {
                package: name.clone(),
                version: version.clone(),
                spec: format!("{}@{}", name, version),
                resolved: tarball_url_from_registry(&name, &version),
                integrity: None,
                install_path: top,
                top_level: true,
            });
        }
        entries.sort_by(|a, b| a.install_path.cmp(&b.install_path).then(a.package.cmp(&b.package)).then(a.version.cmp(&b.version)));
        return Some(entries);
    }

    None
}


#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use super::package_name_from_lockfile_key;

    #[test]
    fn parse_lockfile_key_nested() {
        assert_eq!(
            package_name_from_lockfile_key("node_modules/a/node_modules/b"),
            Some("b".to_string())
        );
    }

    #[test]
    fn parse_lockfile_key_scoped_nested() {
        assert_eq!(
            package_name_from_lockfile_key("node_modules/a/node_modules/@scope/pkg"),
            Some("@scope/pkg".to_string())
        );
    }

    #[test]
    fn parse_lockfile_key_redundant_slashes() {
        assert_eq!(
            package_name_from_lockfile_key("//node_modules/a//node_modules//@scope/pkg//"),
            Some("@scope/pkg".to_string())
        );
    }

    #[test]
    fn top_level_path_detection() {
        assert!(super::is_top_level_lockfile_path("node_modules/react"));
        assert!(!super::is_top_level_lockfile_path("node_modules/a/node_modules/react"));
    }

    #[test]
    fn safe_path_rejects_parent_dir() {
        assert!(!super::is_safe_lockfile_path("../node_modules/react"));
        assert!(super::is_safe_lockfile_path("node_modules/react"));
    
    #[test]
    fn resolve_deps_for_install_is_stable_sorted() {
        let mut deps = HashMap::new();
        deps.insert("z".to_string(), "^1.0.0".to_string());
        deps.insert("a".to_string(), "^1.0.0".to_string());
        let out = super::resolve_deps_for_install(&deps, None);
        assert_eq!(out, vec!["a@^1.0.0".to_string(), "z@^1.0.0".to_string()]);
    }

    #[test]
    fn platform_filter_skips_optional_unsupported() {
        let v = serde_json::json!({
            "optional": true,
            "os": ["!darwin"],
        });
        let should_install = super::package_supported_on_current_platform(&v)
            || !v.get("optional").and_then(|o| o.as_bool()).unwrap_or(false);
        // Test is deterministic on macOS hosts, still valid elsewhere.
        if super::current_npm_os() == "darwin" {
            assert!(!should_install);
        }
    }
}

    #[test]
    fn resolve_deps_for_install_is_stable_sorted() {
        let mut deps = HashMap::new();
        deps.insert("z".to_string(), "^1.0.0".to_string());
        deps.insert("a".to_string(), "^1.0.0".to_string());
        let out = super::resolve_deps_for_install(&deps, None);
        assert_eq!(out, vec!["a@^1.0.0".to_string(), "z@^1.0.0".to_string()]);
    }

    #[test]
    fn platform_filter_skips_optional_unsupported() {
        let v = serde_json::json!({
            "optional": true,
            "os": ["!darwin"],
        });
        let should_install = super::package_supported_on_current_platform(&v)
            || !v.get("optional").and_then(|o| o.as_bool()).unwrap_or(false);
        // Test is deterministic on macOS hosts, still valid elsewhere.
        if super::current_npm_os() == "darwin" {
            assert!(!should_install);
        }
    }
}
