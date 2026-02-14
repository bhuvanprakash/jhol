use std::path::Path;

use crate::lockfile;
use crate::registry;
use crate::utils;

/// Outdated entry: (name, current, wanted, latest)
pub type OutdatedEntry = (String, String, String);

/// Native outdated: read package.json + lockfile, fetch packuments, compare with latest.
pub fn native_outdated() -> Result<Vec<OutdatedEntry>, String> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let deps = lockfile::read_package_json_deps(pj).ok_or("Could not read package.json.")?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    let mut list = Vec::new();
    for (name, spec) in &deps {
        let current = resolved.as_ref().and_then(|r| r.get(name).cloned()).unwrap_or_else(|| "?".to_string());
        let meta = match registry::fetch_metadata(name) {
            Ok(m) => m,
            Err(_) => continue,
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
pub fn check_dependencies(quiet: bool, _backend: crate::backend::Backend) -> Result<(), String> {
    utils::log("Starting dependency check...");

    if !Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }

    if !quiet {
        println!("Scanning dependencies for updates...");
    }

    let list = match native_outdated() {
        Ok(l) => l,
        Err(e) => {
            if !quiet {
                println!("Could not check: {}", e);
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
pub fn fix_dependencies(quiet: bool, _backend: crate::backend::Backend) -> Result<(), String> {
    use crate::install;

    utils::log("Starting dependency fixes...");

    if !Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }

    let list = match native_outdated() {
        Ok(l) => l,
        Err(e) => {
            if !quiet {
                println!("Could not check: {}", e);
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
    let mut tree = crate::lockfile_write::resolve_full_tree(pj)?;
    for (name, _current, latest) in &list {
        let key = format!("node_modules/{}", name);
        if let Some(entry) = tree.get_mut(&key) {
            let meta = registry::fetch_metadata(name)?;
            let resolved_url = registry::get_tarball_url(&meta, latest)
                .ok_or_else(|| format!("No tarball for {}@{}", name, latest))?;
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
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree)?;

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
