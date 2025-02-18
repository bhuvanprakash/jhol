use std::collections::HashSet;
use std::process::Command;
use crate::utils;

/// Validates whether a package exists in NPM before installation
fn is_valid_package(package: &str) -> bool {
    let output = Command::new("npm")
        .arg("show")
        .arg(package)
        .arg("name")
        .output();

    match output {
        Ok(out) => out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        Err(_) => false,
    }
}
pub fn install_package(packages: &[&str]) {
    let mut seen_packages = HashSet::new();
    
    for package in packages {
        let base_name = package.split('@').next().unwrap_or(package);
        let versioned_cache_key = package.replace("@", "-");
        
        if seen_packages.contains(base_name) {
            utils::log(&format!("Warning: Conflicting versions of {} detected!", base_name));
            println!("Warning: Multiple versions of {} are being installed. This may cause issues.", base_name);
        }
        seen_packages.insert(base_name.to_string());

        utils::log(&format!("Installing package: {}", package));

        if utils::is_package_cached(&versioned_cache_key) {
            println!("Installing {} from cache...", package);
            continue;
        }

        if !is_valid_package(package) {
            utils::log(&format!("Package '{}' is invalid or does not exist in NPM.", package));
            println!("Package '{}' is invalid or does not exist.", package);
            continue;
        }

        println!("Fetching {}...", package);
        println!("Package not found in Jhol (Tera package jhol main nhi hai). Trying NPM as fallback...");

        let mut attempts = 3;
        while attempts > 0 {
            let output = Command::new("npm")
                .arg("install")
                .arg(package)
                .output();

            if let Ok(out) = output {
                if out.status.success() {
                    utils::log(&format!("Installed {} via NPM!", package));
                    utils::cache_package(&versioned_cache_key);
                    break;
                } else {
                    utils::log(&format!("Failed to install {} (Attempt {}/3)", package, 4 - attempts));
                }
            } else {
                utils::log(&format!("Error while running NPM install for {} (Attempt {}/3)", package, 4 - attempts));
            }

            attempts -= 1;
            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        if attempts == 0 {
            utils::log(&format!("Failed to install {} after multiple attempts", package));
        }
    }
}
