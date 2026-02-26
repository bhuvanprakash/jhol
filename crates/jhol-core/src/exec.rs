//! Native exec: run a binary from node_modules/.bin without npx/npm exec.

use std::path::{Path, PathBuf};

/// Find binary in node_modules/.bin.
/// Resolves like npm: current package first, then parent workspaces/root.
pub fn find_binary_in_node_modules(binary: &str, from_dir: &Path) -> Option<PathBuf> {
    let mut dir = Some(from_dir);
    while let Some(current) = dir {
        let bin_dir = current.join("node_modules").join(".bin");
        if bin_dir.is_dir() {
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
                let ps1 = bin_dir.join(format!("{}.ps1", binary));
                if ps1.exists() {
                    return Some(ps1);
                }
            }
        }
        dir = current.parent();
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


#[cfg(test)]
mod tests {
    use super::find_binary_in_node_modules;

    #[test]
    fn finds_binary_in_parent_workspace_root() {
        let td = tempfile::tempdir().expect("tmp");
        let root = td.path();
        let ws_pkg = root.join("packages").join("app");
        std::fs::create_dir_all(ws_pkg.join("src")).expect("workspace dirs");

        let bin_dir = root.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).expect("bin dir");
        let bin_name = if cfg!(windows) { "demo.cmd" } else { "demo" };
        std::fs::write(bin_dir.join(bin_name), "echo demo").expect("write bin");

        let found = find_binary_in_node_modules("demo", &ws_pkg).expect("find binary");
        assert!(found.ends_with(bin_name));
    }

    #[test]
    fn prefers_nearest_workspace_bin_over_parent() {
        let td = tempfile::tempdir().expect("tmp");
        let root = td.path();
        let ws_pkg = root.join("packages").join("app");
        std::fs::create_dir_all(&ws_pkg).expect("workspace dir");

        let root_bin = root.join("node_modules").join(".bin");
        std::fs::create_dir_all(&root_bin).expect("root bin");
        let local_bin = ws_pkg.join("node_modules").join(".bin");
        std::fs::create_dir_all(&local_bin).expect("local bin");

        let root_name = if cfg!(windows) { "lint.cmd" } else { "lint" };
        std::fs::write(root_bin.join(root_name), "root").expect("write root");
        std::fs::write(local_bin.join(root_name), "local").expect("write local");

        let found = find_binary_in_node_modules("lint", &ws_pkg).expect("find binary");
        assert!(found.starts_with(&local_bin));
    }
}
