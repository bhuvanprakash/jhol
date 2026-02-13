//! Native exec: run a binary from node_modules/.bin without npx/npm exec.

use std::path::{Path, PathBuf};

/// Find binary in node_modules/.bin. On Windows also looks for <binary>.cmd.
pub fn find_binary_in_node_modules(binary: &str, from_dir: &Path) -> Option<PathBuf> {
    let bin_dir = from_dir.join("node_modules").join(".bin");
    if !bin_dir.is_dir() {
        return None;
    }
    let exact = bin_dir.join(binary);
    if exact.exists() {
        return Some(exact);
    }
    #[cfg(windows)]
    {
        let cmd = bin_dir.join(format!("{}.cmd", binary));
        if cmd.exists() {
            return Some(cmd);
        }
    }
    None
}

/// Execute binary at path with args. On Windows, .cmd scripts are run via cmd /c.
pub fn exec_binary(
    binary_path: &Path,
    args: &[String],
    cwd: &Path,
) -> Result<std::process::ExitStatus, String> {
    #[cfg(unix)]
    {
        let status = std::process::Command::new(binary_path)
            .args(args)
            .current_dir(cwd)
            .status()
            .map_err(|e| format!("Failed to execute: {}", e))?;
        return Ok(status);
    }

    #[cfg(windows)]
    {
        let ext = binary_path.extension().and_then(|e| e.to_str());
        if ext == Some("cmd") || ext == Some("bat") {
            let mut cmd_args = vec!["/c".to_string(), binary_path.to_string_lossy().into_owned()];
            cmd_args.extend(args.iter().cloned());
            let status = std::process::Command::new("cmd")
                .args(&cmd_args)
                .current_dir(cwd)
                .status()
                .map_err(|e| format!("Failed to execute: {}", e))?;
            return Ok(status);
        }
        let status = std::process::Command::new(binary_path)
            .args(args)
            .current_dir(cwd)
            .status()
            .map_err(|e| format!("Failed to execute: {}", e))?;
        Ok(status)
    }
}
