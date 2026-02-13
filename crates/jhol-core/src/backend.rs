//! Backend abstraction: Bun or npm. Install, doctor, and audit are now native by default.
//! Backend is only used when the user passes --fallback-backend (install) or similar opt-in.

use std::process::Command;

use crate::utils::{run_command_timeout, NPM_INSTALL_TIMEOUT_SECS};

const OUTDATED_TIMEOUT_SECS: u64 = 30;

/// Which package manager backend to use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Bun,
    Npm,
}

/// Detect if `bun` is available in PATH.
pub fn bun_available() -> bool {
    Command::new("bun")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Resolve backend: if explicit is Some use it (and check availability); else default to Bun if available, else Npm.
pub fn resolve_backend(explicit: Option<Backend>) -> Backend {
    match explicit {
        Some(Backend::Bun) => {
            if bun_available() {
                Backend::Bun
            } else {
                Backend::Npm
            }
        }
        Some(Backend::Npm) => Backend::Npm,
        None => {
            if bun_available() {
                Backend::Bun
            } else {
                Backend::Npm
            }
        }
    }
}

/// Install packages via backend. specs are e.g. ["lodash@4.17.21", "react@18"].
pub fn backend_install(specs: &[&str], backend: Backend, lockfile_only: bool) -> Result<(), String> {
    if specs.is_empty() {
        return Ok(());
    }
    match backend {
        Backend::Bun => {
            let mut args = vec!["add"];
            if lockfile_only {
                args.push("--lockfile-only");
            }
            for s in specs {
                args.push(s);
            }
            let out = run_command_timeout("bun", &args, NPM_INSTALL_TIMEOUT_SECS)
                .map_err(|e| format!("bun add: {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("bun add failed: {}", stderr));
            }
            Ok(())
        }
        Backend::Npm => {
            let mut args = vec!["install"];
            if lockfile_only {
                args.push("--package-lock-only");
            }
            for s in specs {
                args.push(s);
            }
            let out = run_command_timeout("npm", &args, NPM_INSTALL_TIMEOUT_SECS)
                .map_err(|e| format!("npm install: {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("npm install failed: {}", stderr));
            }
            Ok(())
        }
    }
}

/// Install from package.json only (no spec list). Used for `jhol install` with no args.
pub fn backend_install_from_package_json(
    backend: Backend,
    lockfile_only: bool,
) -> Result<(), String> {
    match backend {
        Backend::Bun => {
            let mut args = vec!["install"];
            if lockfile_only {
                args.push("--lockfile-only");
            }
            let out = run_command_timeout("bun", &args, NPM_INSTALL_TIMEOUT_SECS)
                .map_err(|e| format!("bun install: {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("bun install failed: {}", stderr));
            }
            Ok(())
        }
        Backend::Npm => {
            let mut args = vec!["install"];
            if lockfile_only {
                args.push("--package-lock-only");
            }
            let out = run_command_timeout("npm", &args, NPM_INSTALL_TIMEOUT_SECS)
                .map_err(|e| format!("npm install: {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("npm install failed: {}", stderr));
            }
            Ok(())
        }
    }
}

/// Run outdated check; returns JSON object mapping package name to { current, wanted, latest }.
/// For Bun we use npm outdated (Bun doesn't have a direct equivalent with JSON); or we could use registry + lockfile.
pub fn backend_outdated_json(backend: Backend) -> Option<serde_json::Value> {
    match backend {
        Backend::Npm => {
            let out = run_command_timeout("npm", &["outdated", "--json"], OUTDATED_TIMEOUT_SECS).ok()?;
            let s = String::from_utf8_lossy(&out.stdout);
            serde_json::from_str(&s).ok()
        }
        Backend::Bun => {
            // Bun doesn't have `bun outdated --json`. Use npm outdated for compatibility when bun is backend
            // (project may still have package-lock.json or we run in node_modules context).
            let out = run_command_timeout("npm", &["outdated", "--json"], OUTDATED_TIMEOUT_SECS).ok()?;
            let s = String::from_utf8_lossy(&out.stdout);
            serde_json::from_str(&s).ok()
        }
    }
}

/// Fix outdated packages by installing latest. packages = list of package names.
pub fn backend_fix_packages(packages: &[String], backend: Backend, quiet: bool) -> Result<(), String> {
    if packages.is_empty() {
        return Ok(());
    }
    let specs: Vec<String> = packages.iter().map(|p| format!("{}@latest", p)).collect();
    let refs: Vec<&str> = specs.iter().map(String::as_str).collect();
    if !quiet {
        println!("Updating {} package(s) via {}...", packages.len(), backend_name(backend));
    }
    backend_install(&refs, backend, false)
}

fn backend_name(b: Backend) -> &'static str {
    match b {
        Backend::Bun => "bun",
        Backend::Npm => "npm",
    }
}

const AUDIT_TIMEOUT_SECS: u64 = 60;

/// Run audit (check for vulnerabilities). Returns raw JSON bytes from backend.
/// Backend may exit non-zero when vulns are found; we still return stdout.
pub fn backend_audit(backend: Backend) -> Result<Vec<u8>, String> {
    match backend {
        Backend::Bun => {
            let out = run_command_timeout("bun", &["audit", "--json"], AUDIT_TIMEOUT_SECS)
                .map_err(|e| format!("bun audit: {}", e))?;
            Ok(out.stdout)
        }
        Backend::Npm => {
            let out = run_command_timeout("npm", &["audit", "--json"], AUDIT_TIMEOUT_SECS)
                .map_err(|e| format!("npm audit: {}", e))?;
            Ok(out.stdout)
        }
    }
}

/// Run audit fix. Returns success and stderr for messaging.
pub fn backend_audit_fix(backend: Backend) -> Result<(), String> {
    match backend {
        Backend::Bun => {
            let out = run_command_timeout("bun", &["audit", "fix"], AUDIT_TIMEOUT_SECS)
                .map_err(|e| format!("bun audit fix: {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("bun audit fix failed: {}", stderr));
            }
            Ok(())
        }
        Backend::Npm => {
            let out = run_command_timeout("npm", &["audit", "fix"], AUDIT_TIMEOUT_SECS)
                .map_err(|e| format!("npm audit fix: {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("npm audit fix failed: {}", stderr));
            }
            Ok(())
        }
    }
}

/// Install from cache (tarball paths) using backend. Both bun and npm accept local paths.
pub fn backend_install_tarballs(paths: &[std::path::PathBuf], backend: Backend) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let path_strs: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let refs: Vec<&str> = path_strs.iter().map(String::as_str).collect();
    match backend {
        Backend::Bun => {
            let mut args = vec!["add"];
            args.extend(refs);
            let out = run_command_timeout("bun", &args, NPM_INSTALL_TIMEOUT_SECS)
                .map_err(|e| format!("bun add (cache): {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("bun add failed: {}", stderr));
            }
            Ok(())
        }
        Backend::Npm => {
            let mut args = vec!["install"];
            args.extend(refs);
            let out = run_command_timeout("npm", &args, NPM_INSTALL_TIMEOUT_SECS)
                .map_err(|e| format!("npm install (cache): {}", e))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!("npm install failed: {}", stderr));
            }
            Ok(())
        }
    }
}
