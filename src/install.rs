use std::collections::HashSet;
use std::fs;
use std::path::Path;
use crate::lockfile;
use crate::registry;
use crate::utils::{self, NPM_INSTALL_TIMEOUT_SECS, NPM_SHOW_TIMEOUT_SECS};

/// Package name without version: lodash@4 -> lodash, @scope/pkg@1.0 -> @scope/pkg
fn base_name(package: &str) -> &str {
    if package.starts_with('@') {
        if let Some(idx) = package.rfind('@') {
            if idx > 0 {
                return &package[..idx];
            }
        }
        package
    } else if let Some(idx) = package.find('@') {
        &package[..idx]
    } else {
        package
    }
}

/// Read version from node_modules/<base>/package.json (base may be @scope/pkg)
fn read_installed_version(base: &str) -> Option<String> {
    let path = Path::new("node_modules").join(base).join("package.json");
    let s = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("version")?.as_str().map(String::from)
}

pub struct InstallOptions {
    pub no_cache: bool,
    pub quiet: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self { no_cache: false, quiet: false }
    }
}

/// Install dependencies from package.json (and optional package-lock.json). Returns list of specs to install.
pub fn resolve_install_from_package_json() -> Result<Vec<String>, String> {
    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let deps = lockfile::read_package_json_deps(pj_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let lock_path = Path::new("package-lock.json");
    let resolved = lock_path.exists()
        .then(|| lockfile::read_lockfile_resolved(lock_path))
        .flatten();
    Ok(lockfile::resolve_deps_for_install(&deps, resolved.as_ref()))
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with npm fallback.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), String> {
    let mut seen_packages = HashSet::new();
    let mut to_install_from_cache = Vec::new();
    let mut to_fetch = Vec::new();

    for package in packages {
        let base = base_name(package);
        if seen_packages.contains(base) {
            if !options.quiet {
                println!("Warning: Multiple versions of {} requested.", base);
            }
        }
        seen_packages.insert(base.to_string());
        utils::log(&format!("Installing package: {}", package));

        if !options.no_cache {
            if let Some(tarball) = utils::get_cached_tarball(package) {
                if !options.quiet {
                    println!("Installing {} from cache...", package);
                }
                to_install_from_cache.push((package.to_string(), tarball));
                continue;
            }
        }
        to_fetch.push(package.to_string());
    }

    // Parallel validation for packages we need to fetch
    if !to_fetch.is_empty() {
        let results = utils::parallel_validate_packages(&to_fetch, NPM_SHOW_TIMEOUT_SECS);
        let invalid: Vec<String> = results.iter().filter(|(_, ok)| !*ok).map(|(p, _)| p.clone()).collect();
        if !invalid.is_empty() {
            return Err(format!("Package(s) not found or invalid: {}", invalid.join(", ")));
        }
    }

    // Install from cache: single npm install with all tarball paths
    if !to_install_from_cache.is_empty() {
        let paths: Vec<String> = to_install_from_cache
            .iter()
            .map(|(_, p)| p.to_string_lossy().into_owned())
            .collect();
        let mut args: Vec<&str> = vec!["install"];
        for p in &paths {
            args.push(p.as_str());
        }
        let output = utils::run_command_timeout("npm", &args, NPM_INSTALL_TIMEOUT_SECS);
        match output {
            Ok(out) if out.status.success() => {
                for (pkg, _) in &to_install_from_cache {
                    utils::log(&format!("Installed {} from cache.", pkg));
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("Failed to install from cache: {}", stderr));
            }
            Err(e) => return Err(format!("Error installing from cache: {}", e)),
        }
    }

    if to_fetch.is_empty() {
        return Ok(());
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| e.to_string())?;

    let mut npm_fallback = Vec::new();
    for pkg in &to_fetch {
        if options.no_cache {
            npm_fallback.push(pkg.clone());
            continue;
        }
        match registry::install_package_native(pkg, node_modules, &cache_dir, options) {
            Ok(()) => {}
            Err(_) => {
                npm_fallback.push(pkg.clone());
            }
        }
    }

    if npm_fallback.is_empty() {
        return Ok(());
    }

    // Fallback: npm install for any that native failed
    let fetch_refs: Vec<&str> = npm_fallback.iter().map(|s| s.as_str()).collect();
    let mut attempts = 3;
    loop {
        let output = utils::npm_install_timeout(&fetch_refs, NPM_INSTALL_TIMEOUT_SECS);
        match output {
            Ok(out) if out.status.success() => {
                for pkg in &npm_fallback {
                    let base = base_name(pkg);
                    if let Some(version) = read_installed_version(base) {
                        let _ = utils::cache_package_tarball(base, &version);
                    }
                    utils::log(&format!("Installed {} via NPM.", pkg));
                }
                return Ok(());
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if attempts <= 1 {
                    return Err(format!("npm install failed: {}", stderr));
                }
                if !options.quiet {
                    eprintln!("Install failed, retrying in 2s...");
                }
            }
            Err(e) => {
                if attempts <= 1 {
                    return Err(format!("Error: {}", e));
                }
                if !options.quiet {
                    eprintln!("Error: {}, retrying in 2s...", e);
                }
            }
        }
        attempts -= 1;
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}
