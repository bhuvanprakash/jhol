use std::collections::HashSet;
use std::fs;
use std::path::Path;
use crate::utils::{self, NPM_INSTALL_TIMEOUT_SECS, NPM_SHOW_TIMEOUT_SECS};

/// Validates whether a package exists in NPM (with timeout)
fn is_valid_package(package: &str) -> bool {
    let output = utils::npm_show_timeout(package, NPM_SHOW_TIMEOUT_SECS);
    match output {
        Ok(out) => out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        Err(_) => false,
    }
}

fn base_name(package: &str) -> &str {
    package.split('@').next().unwrap_or(package)
}

/// Read version from node_modules/<base>/package.json
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

pub fn install_package(packages: &[&str], options: &InstallOptions) {
    let mut seen_packages = HashSet::new();

    let mut to_install_from_cache = Vec::new();
    let mut to_fetch = Vec::new();

    for package in packages {
        let base = base_name(package);
        if seen_packages.contains(base) {
            if !options.quiet {
                utils::log(&format!("Warning: Conflicting versions of {} detected!", base));
                println!("Warning: Multiple versions of {} are being installed. This may cause issues.", base);
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

        if !is_valid_package(package) {
            utils::log(&format!("Package '{}' is invalid or does not exist in NPM.", package));
            println!("Package '{}' is invalid or does not exist.", package);
            continue;
        }

        if !options.quiet {
            println!("Fetching {}...", package);
        }
        to_fetch.push(package.to_string());
    }

    // Install from cache: single npm install with all tarball paths
    if !to_install_from_cache.is_empty() {
        let paths: Vec<String> = to_install_from_cache.iter().map(|(_, p)| p.to_string_lossy().into_owned()).collect();
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
                utils::log_error(&format!("Failed to install from cache: {}", stderr));
                eprintln!("Failed to install one or more packages from cache.");
            }
            Err(e) => {
                utils::log_error(&format!("Error installing from cache: {}", e));
                eprintln!("Error: {}", e);
            }
        }
    }

    if to_fetch.is_empty() {
        return;
    }

    // Single npm install for all uncached packages
    let fetch_refs: Vec<&str> = to_fetch.iter().map(|s| s.as_str()).collect();
    let mut attempts = 3;
    while attempts > 0 {
        let output = utils::npm_install_timeout(&fetch_refs, NPM_INSTALL_TIMEOUT_SECS);

        match output {
            Ok(out) if out.status.success() => {
                for pkg in &to_fetch {
                    let base = base_name(pkg);
                    if let Some(version) = read_installed_version(base) {
                        if let Err(e) = utils::cache_package_tarball(base, &version) {
                            utils::log(&format!("Could not cache {}@{}: {}", base, version, e));
                        }
                    }
                    utils::log(&format!("Installed {} via NPM.", pkg));
                }
                break;
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                utils::log(&format!("Failed to install (attempt {}/3): {}", 4 - attempts, stderr));
                if !options.quiet {
                    eprintln!("Install failed, retrying in 2s...");
                }
            }
            Err(e) => {
                utils::log(&format!("Error running npm install (attempt {}/3): {}", 4 - attempts, e));
                if !options.quiet {
                    eprintln!("Error: {}, retrying in 2s...", e);
                }
            }
        }

        attempts -= 1;
        if attempts > 0 {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    if attempts == 0 {
        utils::log("Failed to install packages after 3 attempts.");
        if !options.quiet {
            eprintln!("Failed to install one or more packages. Check logs at ~/.jhol-cache/logs.txt");
        }
    }
}
