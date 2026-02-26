use std::path::Path;

use crate::error_handling::{JholError, RecoveryStrategy};
use crate::lockfile;
use crate::registry;
use crate::utils;

/// Convert JholError to user-friendly string for display
fn format_jhol_error(error: &JholError) -> String {
    match error {
        JholError::Io { operation, path, source } => {
            if let Some(path) = path {
                format!("I/O error in {}: {} (path: {})", operation, source, path)
            } else {
                format!("I/O error in {}: {}", operation, source)
            }
        }
        JholError::Network { operation, url, status, source } => {
            let mut msg = format!("Network error in {}: {}", operation, source);
            if let Some(url) = url {
                msg.push_str(&format!(" (url: {})", url));
            }
            if let Some(status) = status {
                msg.push_str(&format!(" (status: {})", status));
            }
            msg
        }
        JholError::Registry { operation, package, version, source } => {
            let mut msg = format!("Registry error in {}: {}", operation, source);
            if let Some(package) = package {
                msg.push_str(&format!(" (package: {})", package));
            }
            if let Some(version) = version {
                msg.push_str(&format!(" (version: {})", version));
            }
            msg
        }
        JholError::Resolution { operation, package, conflict_details, source } => {
            let mut msg = format!("Resolution error in {}: {}", operation, source);
            if let Some(package) = package {
                msg.push_str(&format!(" (package: {})", package));
            }
            if let Some(details) = conflict_details {
                msg.push_str(&format!(" (details: {})", details));
            }
            msg
        }
        JholError::Cache { operation, key, source } => {
            if let Some(key) = key {
                format!("Cache error in {}: {} (key: {})", operation, source, key)
            } else {
                format!("Cache error in {}: {}", operation, source)
            }
        }
        JholError::Config { operation, field, source } => {
            if let Some(field) = field {
                format!("Configuration error in {}: {} (field: {})", operation, source, field)
            } else {
                format!("Configuration error in {}: {}", operation, source)
            }
        }
        JholError::Security { operation, path, reason } => {
            if let Some(path) = path {
                format!("Security error in {}: {} (path: {})", operation, reason, path)
            } else {
                format!("Security error in {}: {}", operation, reason)
            }
        }
        JholError::Performance { operation, duration, limit, source } => {
            let mut msg = format!("Performance error in {}: {}", operation, source);
            if let Some(duration) = duration {
                msg.push_str(&format!(" (duration: {}ms)", duration));
            }
            if let Some(limit) = limit {
                msg.push_str(&format!(" (limit: {}ms)", limit));
            }
            msg
        }
        JholError::Application { operation, details, source } => {
            if let Some(details) = details {
                format!("Application error in {}: {} (details: {})", operation, source, details)
            } else {
                format!("Application error in {}: {}", operation, source)
            }
        }
    }
}

/// Outdated entry: (name, current, wanted, latest)
pub type OutdatedEntry = (String, String, String);

/// Native outdated: read package.json + lockfile, fetch packuments, compare with latest.
pub fn native_outdated() -> Result<Vec<OutdatedEntry>, JholError> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(crate::error_handling::utils::io_error(
            "native_outdated",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found")
        ));
    }
    let deps = lockfile::read_package_json_deps(pj)
        .ok_or_else(|| crate::error_handling::utils::config_error("read_package_json_deps", Some("dependencies"), "Could not read package.json"))?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    let mut list = Vec::new();
    for (name, spec) in &deps {
        let current = resolved.as_ref().and_then(|r| r.get(name).cloned()).unwrap_or_else(|| "?".to_string());
        let meta = match registry::fetch_metadata(name) {
            Ok(m) => m,
            Err(e) => {
                utils::log(&format!("Could not fetch metadata for {}: {}", name, e));
                continue;
            }
        };
        let latest = registry::resolve_version(&meta, "latest").unwrap_or_else(|| current.clone());
        let wanted = registry::resolve_version(&meta, spec).unwrap_or_else(|| current.clone());
        if wanted != latest || current != latest {
            list.push((name.clone(), current, latest));
        }
    }
    Ok(list)
}

/// Scans dependencies using native registry and reports which are outdated
pub fn check_dependencies(quiet: bool, _backend: crate::backend::Backend) -> Result<(), JholError> {
    utils::log("Starting dependency check...");

    if !Path::new("package.json").exists() {
        return Err(crate::error_handling::utils::io_error(
            "check_dependencies",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found")
        ));
    }

    if !quiet {
        println!("Scanning dependencies for updates...");
    }

    let list = match native_outdated() {
        Ok(l) => l,
        Err(e) => {
            if !quiet {
                println!("Could not check: {}", format_jhol_error(&e));
            }
            return Err(e);
        }
    };

    if list.is_empty() {
        if !quiet {
            println!("All dependencies are up-to-date!");
        }
        return Ok(());
    }

    if !quiet {
        println!("Found {} outdated dependency(ies):", list.len());
        for (pkg, cur, latest) in &list {
            println!("  {}: {} -> {}", pkg, cur, latest);
        }
    }

    utils::log(&format!("Outdated: {:?}", list.iter().map(|(p, _, _)| p).collect::<Vec<_>>()));
    Ok(())
}

/// Fix outdated dependencies: update lockfile to latest and run native install
pub fn fix_dependencies(quiet: bool, _backend: crate::backend::Backend) -> Result<(), JholError> {
    use crate::install;

    utils::log("Starting dependency fixes...");

    if !Path::new("package.json").exists() {
        return Err(crate::error_handling::utils::io_error(
            "fix_dependencies",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found")
        ));
    }

    let list = match native_outdated() {
        Ok(l) => l,
        Err(e) => {
            if !quiet {
                println!("Could not check: {}", format_jhol_error(&e));
            }
            return Err(e);
        }
    };

    if list.is_empty() {
        if !quiet {
            println!("No fixes needed - all dependencies are up-to-date!");
        }
        return Ok(());
    }

    if !quiet {
        println!("Applying fixes for {} package(s)...", list.len());
    }

    let pj = Path::new("package.json");
    let mut tree = crate::lockfile_write::resolve_full_tree(pj)
        .map_err(|e| crate::error_handling::utils::config_error("resolve_full_tree", None, &e))?;
    for (name, _current, latest) in &list {
        let key = format!("node_modules/{}", name);
        if let Some(entry) = tree.get_mut(&key) {
            let meta = registry::fetch_metadata(name)
                .map_err(|e| crate::error_handling::utils::registry_error_with_package("fetch_metadata", name, None, &e.to_string()))?;
            let resolved_url = registry::get_tarball_url(&meta, latest)
                .ok_or_else(|| crate::error_handling::utils::registry_error_with_package("get_tarball_url", name, Some(latest), "No tarball available"))?;
            let integrity = meta
                .get("versions")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get(latest))
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("dist"))
                .and_then(|d| d.as_object())
                .and_then(|d| d.get("integrity"))
                .and_then(|i| i.as_str())
                .map(String::from);
            entry.version = latest.clone();
            entry.resolved = resolved_url;
            entry.integrity = integrity;
        }
    }

    let lock_path = Path::new("package-lock.json");
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree)
        .map_err(|e| crate::error_handling::utils::io_error("write_package_lock", Some("package-lock.json"), std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

    let opts = install::InstallOptions {
        from_lockfile: true,
        ..Default::default()
    };
    let specs = install::resolve_install_from_package_json(false)?;
    let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
    install::install_package(&refs, &opts)?;

    utils::log("Dependency fixes completed.");
    if !quiet {
        println!("Fixes applied.");
    }
    Ok(())
}

/// Explain project health and compatibility diagnostics in a compact report.
pub fn explain_project_health() -> Result<String, String> {
    let cwd = Path::new(".");
    let lock_kind = lockfile::detect_lockfile(cwd);
    let lock_name = match lock_kind {
        lockfile::LockfileKind::NpmShrinkwrap => "npm-shrinkwrap.json",
        lockfile::LockfileKind::Npm => "package-lock.json",
        lockfile::LockfileKind::Bun => "bun.lock",
        lockfile::LockfileKind::None => "none",
    };
    let deps = lockfile::read_package_json_deps(Path::new("package.json")).unwrap_or_default();
    let telemetry = utils::read_fallback_telemetry();
    let total_fallbacks = telemetry
        .get("totalFallbacks")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let integrity_ok = lockfile::lockfile_integrity_complete(cwd);
    let workspace_count = crate::workspaces::list_workspace_roots(cwd)
        .map(|v| v.len())
        .unwrap_or(0);
    let registry = crate::config::effective_registry_url(cwd);

    let mut out = String::new();
    out.push_str("Jhol doctor --explain\n");
    out.push_str("=====================\n");
    out.push_str(&format!("Lockfile: {}\n", lock_name));
    out.push_str(&format!("Dependencies in package.json: {}\n", deps.len()));
    out.push_str(&format!("Workspace packages detected: {}\n", workspace_count));
    out.push_str(&format!("Registry: {}\n", registry));
    out.push_str(&format!(
        "Lockfile integrity complete: {}\n",
        if integrity_ok { "yes" } else { "no" }
    ));
    out.push_str(&format!("Native fallback count (local telemetry): {}\n", total_fallbacks));
    if !integrity_ok {
        out.push_str("Hint: run `jhol install` once to regenerate lockfile integrity entries.\n");
    }
    if total_fallbacks > 0 {
        out.push_str("Hint: run `jhol cache telemetry` for fallback breakdown by reason/package.\n");
    }
    if matches!(lock_kind, lockfile::LockfileKind::Bun) {
        out.push_str("Hint: run `jhol import-lock --from bun` to generate package-lock.json for npm-compatible workflows.\n");
    }
    Ok(out)
}
