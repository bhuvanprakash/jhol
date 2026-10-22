//! Toolchain / environment discovery (blueprint §4 `jhol-env`).
//!
//! Records observed host tools; does not install them yet.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainReport {
    pub os: String,
    pub arch: String,
    pub tools: Vec<ToolPresence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPresence {
    pub name: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

fn probe(name: &str, args: &[&str]) -> ToolPresence {
    let available = which::which_like(name);
    let version = if available {
        Command::new(name).args(args).output().ok().and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            let line = s.lines().next().unwrap_or("").trim();
            if line.is_empty() {
                let e = String::from_utf8_lossy(&o.stderr);
                let line = e.lines().next().unwrap_or("").trim();
                if line.is_empty() {
                    None
                } else {
                    Some(line.to_string())
                }
            } else {
                Some(line.to_string())
            }
        })
    } else {
        None
    };
    ToolPresence {
        name: name.to_string(),
        available,
        version,
    }
}

mod which {
    use std::env;
    use std::path::PathBuf;

    pub fn which_like(name: &str) -> bool {
        if let Ok(path) = env::var("PATH") {
            for dir in env::split_paths(&path) {
                let candidate: PathBuf = dir.join(name);
                if candidate.is_file() {
                    return true;
                }
                #[cfg(windows)]
                {
                    let exe = dir.join(format!("{name}.exe"));
                    if exe.is_file() {
                        return true;
                    }
                }
            }
        }
        false
    }
}

pub fn probe_host() -> ToolchainReport {
    ToolchainReport {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        tools: vec![
            probe("node", &["--version"]),
            probe("npm", &["--version"]),
            probe("bun", &["--version"]),
            probe("python3", &["--version"]),
            probe("uv", &["--version"]),
            probe("cargo", &["--version"]),
            probe("rustc", &["--version"]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_has_host_fields() {
        let r = probe_host();
        assert!(!r.os.is_empty());
        assert!(!r.arch.is_empty());
        assert!(!r.tools.is_empty());
    }
}
