//! Native script runner: run package.json scripts without npm or Bun.

use std::collections::HashMap;
use std::path::Path;

/// Read script command from package.json. Returns the script string or error.
pub fn get_script_command(script_name: &str, package_json_path: &Path) -> Result<String, String> {
    let s = std::fs::read_to_string(package_json_path)
        .map_err(|e| format!("Could not read package.json: {}", e))?;
    let v: serde_json::Value =
        serde_json::from_str(&s).map_err(|e| format!("Invalid package.json: {}", e))?;
    let scripts = v
        .get("scripts")
        .and_then(|s| s.as_object())
        .ok_or_else(|| "package.json has no \"scripts\" object.".to_string())?;
    let cmd = scripts
        .get(script_name)
        .and_then(|c| c.as_str())
        .map(String::from)
        .ok_or_else(|| format!("Missing script \"{}\" in package.json.", script_name))?;
    if cmd.trim().is_empty() {
        return Err(format!("Script \"{}\" is empty.", script_name));
    }
    Ok(cmd)
}

/// Run a package.json script: set PATH to include node_modules/.bin, optional npm_* env, then shell.
pub fn run_script(
    script_name: &str,
    cwd: &Path,
    extra_env: Option<HashMap<String, String>>,
) -> Result<std::process::ExitStatus, String> {
    let package_json_path = cwd.join("package.json");
    if !package_json_path.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let script_cmd = get_script_command(script_name, &package_json_path)?;

    let bin_dir = cwd.join("node_modules").join(".bin");
    let path_env = std::env::var("PATH").unwrap_or_default();
    let new_path = if bin_dir.exists() {
        let bin_str = bin_dir.to_string_lossy();
        #[cfg(unix)]
        let sep = ":";
        #[cfg(windows)]
        let sep = ";";
        format!("{}{}{}", bin_str, sep, path_env)
    } else {
        path_env
    };

    let mut env: HashMap<String, String> = extra_env.unwrap_or_default();
    env.insert("PATH".to_string(), new_path);

    if let Ok(s) = std::fs::read_to_string(&package_json_path) {
        if let Ok(pkg_json) = serde_json::from_str::<serde_json::Value>(&s) {
        if let Some(name) = pkg_json.get("name").and_then(|n| n.as_str()) {
            env.insert("npm_package_name".to_string(), name.to_string());
        }
        if let Some(ver) = pkg_json.get("version").and_then(|v| v.as_str()) {
            env.insert("npm_package_version".to_string(), ver.to_string());
        }
        }
    }

    #[cfg(unix)]
    let (shell, shell_arg, script_arg) = {
        let sh = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        (sh, "-c", script_cmd)
    };

    #[cfg(windows)]
    let (shell, shell_arg, script_arg) = {
        ("cmd".to_string(), "/c", script_cmd)
    };

    let status = std::process::Command::new(&shell)
        .arg(shell_arg)
        .arg(script_arg)
        .current_dir(cwd)
        .envs(env)
        .status()
        .map_err(|e| format!("Failed to run script: {}", e))?;

    Ok(status)
}
