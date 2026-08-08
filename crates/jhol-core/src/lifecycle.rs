//! Native lifecycle script executor for post-install hooks.
//! Runs scripts from a package's package.json after extraction into node_modules.

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitStatus;

/// Which lifecycle scripts to run, in order.
const INSTALL_SCRIPTS: &[&str] = &["preinstall", "install", "postinstall"];

/// Run lifecycle scripts for a single extracted package.
///
/// - `pkg_dir`: the extracted package directory (e.g. `node_modules/husky`)
/// - `node_modules`: the root node_modules directory
/// - `allowed`: set of package names allowed to run scripts
///
/// Returns Ok(true) if any scripts were run, Ok(false) if skipped (not allowed or no scripts),
/// Err if a script failed.
pub fn run_install_scripts(
    pkg_dir: &Path,
    node_modules: &Path,
    allowed: &HashSet<String>,
) -> Result<bool, String> {
    if allowed.is_empty() {
        return Ok(false);
    }
    let pkg_json_path = pkg_dir.join("package.json");
    if !pkg_json_path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&pkg_json_path)
        .map_err(|e| format!("Failed to read package.json: {}", e))?;
    let pkg_json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Invalid package.json: {}", e))?;

    let pkg_name = pkg_json
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

    // Only run scripts if this package is in the allowlist
    if !allowed.is_empty() && !allowed.contains(&pkg_name) {
        return Ok(false);
    }

    let scripts = match pkg_json.get("scripts").and_then(|s| s.as_object()) {
        Some(s) => s.clone(),
        None => return Ok(false),
    };

    // Determine which scripts to run
    let mut ran_any = false;
    for &script_name in INSTALL_SCRIPTS {
        let Some(script_cmd) = scripts.get(script_name).and_then(|s| s.as_str()) else {
            continue;
        };
        let script_cmd = script_cmd.trim();
        if script_cmd.is_empty() {
            continue;
        }

        run_single_script(script_name, script_cmd, pkg_dir, node_modules, &pkg_name)?;
        ran_any = true;
    }

    // Run `prepare` only if no install scripts were found
    if !ran_any {
        if let Some(prepare_cmd) = scripts.get("prepare").and_then(|s| s.as_str()) {
            let prepare_cmd = prepare_cmd.trim();
            if !prepare_cmd.is_empty() {
                run_single_script("prepare", prepare_cmd, pkg_dir, node_modules, &pkg_name)?;
                ran_any = true;
            }
        }
    }

    Ok(ran_any)
}

fn run_single_script(
    script_name: &str,
    script_cmd: &str,
    pkg_dir: &Path,
    node_modules: &Path,
    pkg_name: &str,
) -> Result<ExitStatus, String> {
    // Build PATH with node_modules/.bin prepended
    let bin_dir = node_modules.join(".bin");
    let current_path = std::env::var("PATH").unwrap_or_default();
    let new_path = if bin_dir.exists() {
        #[cfg(unix)]
        let sep = ":";
        #[cfg(windows)]
        let sep = ";";
        format!("{}{}{}", bin_dir.display(), sep, current_path)
    } else {
        current_path
    };

    // npm_lifecycle_event and npm_package_* vars for script compatibility
    let mut cmd = {
        #[cfg(unix)]
        {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(script_cmd);
            c
        }
        #[cfg(windows)]
        {
            let mut c = std::process::Command::new("cmd");
            c.arg("/c").arg(script_cmd);
            c
        }
    };

    cmd.current_dir(pkg_dir)
        .env("PATH", &new_path)
        .env("npm_lifecycle_event", script_name)
        .env("npm_package_name", pkg_name)
        .env(
            "npm_config_node_modules",
            node_modules.to_string_lossy().as_ref(),
        );

    // Set NODE_PATH to include the package's own node_modules if present
    let pkg_nm = pkg_dir.join("node_modules");
    if pkg_nm.exists() {
        cmd.env("NODE_PATH", pkg_nm.to_string_lossy().as_ref());
    }

    let status = cmd.status().map_err(|e| {
        format!(
            "Failed to launch {} script for {}: {}",
            script_name, pkg_name, e
        )
    })?;

    if !status.success() {
        return Err(format!(
            "Lifecycle script '{}' for package '{}' failed with exit code {:?}",
            script_name,
            pkg_name,
            status.code()
        ));
    }

    Ok(status)
}

/// Run lifecycle scripts for ALL packages in node_modules that are in the allowlist.
/// Called after a full install completes.
///
/// Returns a list of (package_name, result) pairs for scripts that ran.
pub fn run_all_install_scripts(
    node_modules: &Path,
    allowed: &HashSet<String>,
) -> Vec<(String, Result<bool, String>)> {
    if allowed.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    // Walk top-level packages
    let Ok(entries) = std::fs::read_dir(node_modules) else {
        return results;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == ".bin" {
            continue;
        }

        let pkg_dir = entry.path();

        if name.starts_with('@') && pkg_dir.is_dir() {
            // Scoped package directory — recurse one level
            let Ok(scoped) = std::fs::read_dir(&pkg_dir) else {
                continue;
            };
            for scoped_entry in scoped.flatten() {
                let scoped_name =
                    format!("{}/{}", name, scoped_entry.file_name().to_string_lossy());
                let scoped_dir = scoped_entry.path();
                if !allowed.contains(name.as_str()) && !allowed.contains(scoped_name.as_str()) {
                    continue;
                }
                let result = run_install_scripts(&scoped_dir, node_modules, allowed);
                results.push((scoped_name, result));
            }
        } else {
            if !allowed.contains(name.as_str()) {
                continue;
            }
            let result = run_install_scripts(&pkg_dir, node_modules, allowed);
            results.push((name, result));
        }
    }

    results
}
