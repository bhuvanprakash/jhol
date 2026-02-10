use crate::backend::{self, Backend};
use crate::utils;

/// Scans dependencies using backend outdated and reports which are outdated
pub fn check_dependencies(quiet: bool, backend: Backend) -> Result<(), String> {
    utils::log("Starting dependency check...");

    if !std::path::Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }

    if !quiet {
        println!("Scanning dependencies for updates...");
    }

    let outdated = match backend::backend_outdated_json(backend) {
        Some(obj) => obj,
        None => {
            if !quiet {
                println!("All dependencies are up-to-date (or npm outdated could not run).");
            }
            return Ok(());
        }
    };

    let obj = match outdated.as_object() {
        Some(o) => o,
        None => {
            if !quiet {
                println!("All dependencies are up-to-date!");
            }
            return Ok(());
        }
    };

    if obj.is_empty() {
        if !quiet {
            println!("All dependencies are up-to-date!");
        }
        return Ok(());
    }

    let mut list: Vec<(String, String, String)> = Vec::new();
    for (pkg, val) in obj {
        let cur = val.get("current").and_then(|v| v.as_str()).unwrap_or("?");
        let wanted = val.get("wanted").and_then(|v| v.as_str()).unwrap_or("?");
        let latest = val.get("latest").and_then(|v| v.as_str()).unwrap_or(wanted);
        list.push((pkg.clone(), cur.to_string(), latest.to_string()));
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

/// Fix outdated dependencies by installing latest versions
pub fn fix_dependencies(quiet: bool, backend: Backend) -> Result<(), String> {
    utils::log("Starting dependency fixes...");

    if !std::path::Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }

    let outdated = match backend::backend_outdated_json(backend) {
        Some(obj) => obj,
        None => {
            if !quiet {
                println!("No outdated packages (or npm outdated could not run).");
            }
            return Ok(());
        }
    };

    let obj = match outdated.as_object() {
        Some(o) => o,
        None => {
            if !quiet {
                println!("No fixes needed.");
            }
            return Ok(());
        }
    };

    if obj.is_empty() {
        if !quiet {
            println!("No fixes needed - all dependencies are up-to-date!");
        }
        return Ok(());
    }

    let packages: Vec<String> = obj.keys().cloned().collect();
    if !quiet {
        println!("Applying fixes for {} package(s)...", packages.len());
    }

    backend::backend_fix_packages(&packages, backend, quiet)?;
    utils::log("Dependency fixes completed.");
    if !quiet {
        println!("Fixes applied.");
    }
    Ok(())
}
