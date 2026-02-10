use crate::utils::{self, run_command_timeout, NPM_INSTALL_TIMEOUT_SECS};

const NPM_OUTDATED_TIMEOUT_SECS: u64 = 30;

/// Run npm outdated --json. Returns map of package -> { current, wanted, latest }.
fn get_outdated_json() -> Option<serde_json::Value> {
    let output = run_command_timeout("npm", &["outdated", "--json"], NPM_OUTDATED_TIMEOUT_SECS).ok()?;
    // npm outdated exits with 1 when there are outdated packages, so we ignore status and parse stdout
    let s = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&s).ok()
}

/// Scans dependencies using `npm outdated` and reports which are outdated
pub fn check_dependencies(quiet: bool) -> Result<(), String> {
    utils::log("Starting dependency check...");

    if !std::path::Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }

    if !quiet {
        println!("Scanning dependencies for updates...");
    }

    let outdated = match get_outdated_json() {
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
pub fn fix_dependencies(quiet: bool) -> Result<(), String> {
    utils::log("Starting dependency fixes...");

    if !std::path::Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }

    let outdated = match get_outdated_json() {
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

    apply_fixes(&packages, quiet);
    utils::log("Dependency fixes completed.");
    if !quiet {
        println!("Fixes applied.");
    }
    Ok(())
}

fn apply_fixes(packages: &[String], quiet: bool) {
    if packages.is_empty() {
        return;
    }
    // Single npm install with all packages@latest (faster than one-by-one)
    let specs: Vec<String> = packages.iter().map(|p| format!("{}@latest", p)).collect();
    let args: Vec<&str> = specs.iter().map(String::as_str).collect();
    let mut npm_args = vec!["install"];
    npm_args.extend(args.iter().copied());

    if !quiet {
        println!("Updating {} package(s) in one go...", packages.len());
    }
    let output = run_command_timeout("npm", &npm_args, NPM_INSTALL_TIMEOUT_SECS);

    match output {
        Ok(out) if out.status.success() => {
            utils::log("All updates applied.");
            if !quiet {
                println!("Done. All {} package(s) updated.", packages.len());
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            utils::log_error(&format!("npm install failed: {}", stderr));
            eprintln!("Update failed. You can try updating packages one by one.");
        }
        Err(e) => {
            utils::log_error(&format!("Error: {}", e));
            eprintln!("Error: {}", e);
        }
    }
}
