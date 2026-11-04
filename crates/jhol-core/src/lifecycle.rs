//! Native lifecycle script executor for post-install hooks.
//! Runs scripts from a package's package.json after extraction into node_modules.
//!
//! ## Script Receipts
//!
//! Approval to run install scripts is **content-addressed**: a receipt is
//! SHA-256 over `name@version`, the package's *exact* script commands, and the
//! tarball integrity (SRI) recorded in the lockfile. Receipts live in
//! `.jhol/script-receipts.json` (commit it like the lockfile).
//!
//! The security property npm and pnpm do not offer: a name-based allowlist
//! keeps trusting a package forever, even after a republished tarball swaps
//! its install script for something else. A receipt breaks the moment any of
//! {version, script text, tarball bytes} changes — the approval is bound to
//! the artifact, not the name.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// Which lifecycle scripts to run, in order.
const INSTALL_SCRIPTS: &[&str] = &["preinstall", "install", "postinstall"];

/// Where script receipts live, relative to the project root.
pub const RECEIPTS_REL_PATH: &str = ".jhol/script-receipts.json";

/// One package's consent state after a policy check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptOutcome {
    /// Receipt matched (or explicit one-shot consent): scripts executed.
    Ran,
    /// Approved-by-content but the package ships no install scripts.
    NoScripts,
    /// Scripts exist but no receipt: skipped. Carries the receipt hash that
    /// `jhol approve-scripts` would record.
    NeedsApproval(String),
    /// Explicitly not on the allowlist during a one-shot `--allow-build-scripts`
    /// run (the legacy name-based path).
    NotAllowlisted,
    Failed(String),
}

/// Compute the content-addressed receipt for a package's install scripts.
///
/// `integrity` is the SRI from the lockfile (empty string when unknown);
/// including it means a republished tarball with different bytes invalidates
/// a previously granted approval even if the scripts text is identical.
pub fn script_receipt(name: &str, version: &str, scripts_json: &str, integrity: &str) -> String {
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.update(b"\0");
    h.update(version.as_bytes());
    h.update(b"\0");
    h.update(scripts_json.as_bytes());
    h.update(b"\0");
    h.update(integrity.as_bytes());
    let digest = h.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn receipts_file() -> PathBuf {
    PathBuf::from(RECEIPTS_REL_PATH)
}

/// Load the project's script receipts. Missing file = no approvals.
pub fn load_receipts(project: &Path) -> HashMap<String, String> {
    let path = project.join(receipts_file());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Persist the project's script receipts (sorted keys for stable diffs).
/// Marker of scripts already executed for a receipt (name@version -> hash).
/// npm semantics: scripts run when a package is (re)installed, not on every
/// no-op install. Keyed to the receipt so a changed artifact re-runs.
fn runs_file() -> PathBuf {
    PathBuf::from(".jhol/script-runs.json")
}

pub fn load_run_markers(project: &Path) -> HashMap<String, String> {
    let path = project.join(runs_file());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_run_markers(project: &Path, runs: &HashMap<String, String>) {
    let path = project.join(runs_file());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut keys: Vec<&String> = runs.keys().collect();
    keys.sort();
    let mut body = serde_json::Map::new();
    for k in keys {
        body.insert(k.clone(), serde_json::Value::String(runs[k].clone()));
    }
    if let Ok(text) = serde_json::to_string_pretty(&serde_json::Value::Object(body)) {
        let _ = std::fs::write(&path, text + "\n");
    }
}

pub fn save_receipts(project: &Path, receipts: &HashMap<String, String>) -> Result<(), String> {
    let path = project.join(receipts_file());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut keys: Vec<&String> = receipts.keys().collect();
    keys.sort();
    let mut body = serde_json::Map::new();
    for k in keys {
        body.insert(k.clone(), serde_json::Value::String(receipts[k].clone()));
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(body))
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| e.to_string())
}

/// The policy a script run is evaluated against.
#[derive(Debug, Default, Clone)]
pub struct ScriptPolicy {
    /// One-shot name allowlist (`--allow-build-scripts` legacy path).
    pub allowlist: HashSet<String>,
    /// Content-addressed approvals: `name@version` -> receipt hash.
    pub receipts: HashMap<String, String>,
}

impl ScriptPolicy {
    /// True when consent came from the legacy one-shot flag (scripts run
    /// unconditionally for allowlisted names on every such install).
    pub fn force_all(&self) -> bool {
        !self.allowlist.is_empty()
    }
}

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

/// Read a package's (version, scripts object) from its extracted package.json.
fn read_pkg_scripts(pkg_dir: &Path) -> Option<(String, serde_json::Value)> {
    let content = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    let pj: serde_json::Value = serde_json::from_str(&content).ok()?;
    let version = pj
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    let scripts = pj
        .get("scripts")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some((version, scripts))
}

fn has_install_scripts(scripts: &serde_json::Value) -> bool {
    INSTALL_SCRIPTS
        .iter()
        .chain(std::iter::once(&"prepare"))
        .any(|k| {
            scripts
                .get(*k)
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        })
}

/// Canonical JSON of the scripts object, for receipt hashing.
fn canonical_scripts(scripts: &serde_json::Value) -> String {
    serde_json::to_string(scripts).unwrap_or_default()
}

/// Run install scripts for every top-level (and scoped) package under
/// `node_modules`, evaluated against `policy`. Every package with install
/// scripts produces an outcome — the caller surfaces the summary, including
/// the exact command that would approve the skipped ones.
pub fn run_all_install_scripts(
    node_modules: &Path,
    project: &Path,
    integrity_of: &dyn Fn(&str, &str) -> Option<String>,
    policy: &ScriptPolicy,
) -> Vec<(String, ScriptOutcome)> {
    let mut results: Vec<(String, ScriptOutcome)> = Vec::new();

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
            let Ok(scoped) = std::fs::read_dir(&pkg_dir) else {
                continue;
            };
            for scoped_entry in scoped.flatten() {
                let scoped_name =
                    format!("{}/{}", name, scoped_entry.file_name().to_string_lossy());
                let scoped_dir = scoped_entry.path();
                let outcome = evaluate_package(
                    &scoped_dir,
                    &scoped_name,
                    node_modules,
                    project,
                    integrity_of,
                    policy,
                );
                results.push((scoped_name, outcome));
            }
        } else if pkg_dir.is_dir() {
            let outcome =
                evaluate_package(&pkg_dir, &name, node_modules, project, integrity_of, policy);
            results.push((name, outcome));
        }
    }
    results
}

fn evaluate_package(
    pkg_dir: &Path,
    name: &str,
    node_modules: &Path,
    project: &Path,
    integrity_of: &dyn Fn(&str, &str) -> Option<String>,
    policy: &ScriptPolicy,
) -> ScriptOutcome {
    let Some((version, scripts)) = read_pkg_scripts(pkg_dir) else {
        return ScriptOutcome::NoScripts;
    };
    if !has_install_scripts(&scripts) {
        return ScriptOutcome::NoScripts;
    }

    // Legacy one-shot path: an explicit allowlist run trusts names.
    if !policy.allowlist.is_empty() {
        if policy.allowlist.contains(name) {
            return match run_install_scripts(pkg_dir, node_modules, &policy.allowlist) {
                Ok(true) => ScriptOutcome::Ran,
                Ok(false) => ScriptOutcome::NoScripts,
                Err(e) => ScriptOutcome::Failed(e),
            };
        }
        return ScriptOutcome::NotAllowlisted;
    }

    // Receipt path: consent bound to version + script text + tarball bytes.
    let integrity = integrity_of(name, &version).unwrap_or_default();
    let expected = script_receipt(name, &version, &canonical_scripts(&scripts), &integrity);
    match policy.receipts.get(&format!("{}@{}", name, version)) {
        Some(granted) if *granted == expected => {
            // The receipt IS the consent: gate via a single-name set (an
            // empty allowlist would make run_install_scripts skip).
            let one: HashSet<String> = [name.to_string()].into_iter().collect();
            match run_install_scripts(pkg_dir, node_modules, &one) {
                Ok(ran) => {
                    if ran {
                        ScriptOutcome::Ran
                    } else {
                        ScriptOutcome::NoScripts
                    }
                }
                Err(e) => ScriptOutcome::Failed(e),
            }
        }
        _ => ScriptOutcome::NeedsApproval(expected),
    }
}

// ---- approve / revoke ----

/// Lockfile integrity map for receipt hashing ("node_modules/<name>" -> SRI).
/// Small dedicated reader: keeps lifecycle.rs standalone from install.rs.
fn lockfile_integrity(project: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(project.join("package-lock.json")) else {
        return map;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return map;
    };
    if let Some(packages) = v.get("packages").and_then(|p| p.as_object()) {
        for (k, entry) in packages {
            if let Some(integ) = entry.get("integrity").and_then(|i| i.as_str()) {
                map.insert(k.clone(), integ.to_string());
            }
        }
    }
    map
}

/// Grant content-addressed script consent. `asked` filters by package name or
/// `name@version`; `all` approves every package that currently ships install
/// scripts. Receipts are written to `.jhol/script-receipts.json`.
pub fn approve_scripts(project: &Path, asked: &[String], all: bool) -> Result<Vec<String>, String> {
    if !all && asked.is_empty() {
        return Err(
            "specify packages to approve, or pass --all to approve every package with install scripts"
                .to_string(),
        );
    }
    let node_modules = project.join("node_modules");
    let integrity = lockfile_integrity(project);
    let mut receipts = load_receipts(project);
    let mut granted = Vec::new();

    fn walk(
        dir: &Path,
        prefix: &str,
        integrity: &HashMap<String, String>,
        asked: &[String],
        all: bool,
        receipts: &mut HashMap<String, String>,
        granted: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let base = entry.file_name().to_string_lossy().to_string();
            if base.starts_with('.') || base == ".bin" {
                continue;
            }
            let pkg_dir = entry.path();
            let full = if prefix.is_empty() {
                base.clone()
            } else {
                format!("{}/{}", prefix, base)
            };
            if base.starts_with('@') && pkg_dir.is_dir() {
                walk(&pkg_dir, &base, integrity, asked, all, receipts, granted);
                continue;
            }
            if !pkg_dir.is_dir() {
                continue;
            }
            let Some((version, scripts)) = read_pkg_scripts(&pkg_dir) else {
                continue;
            };
            if !has_install_scripts(&scripts) {
                continue;
            }
            let wanted = all
                || asked
                    .iter()
                    .any(|a| a == &full || a == &format!("{}@{}", full, version));
            if !wanted {
                continue;
            }
            let integ = integrity
                .get(&format!("node_modules/{}", full))
                .cloned()
                .unwrap_or_default();
            let hash = script_receipt(&full, &version, &canonical_scripts(&scripts), &integ);
            let key = format!("{}@{}", full, version);
            if receipts.get(&key) != Some(&hash) {
                receipts.insert(key.clone(), hash);
            }
            granted.push(key);
        }
    }
    walk(
        &node_modules,
        "",
        &integrity,
        asked,
        all,
        &mut receipts,
        &mut granted,
    );
    granted.sort();
    granted.dedup();
    save_receipts(project, &receipts)?;
    Ok(granted)
}

/// Revoke consent. `asked` matches by package name (all versions) or
/// `name@version`.
pub fn revoke_scripts(project: &Path, asked: &[String]) -> Result<Vec<String>, String> {
    let mut receipts = load_receipts(project);
    let mut removed = Vec::new();
    let keys: Vec<String> = receipts.keys().cloned().collect();
    for key in keys {
        let name = key.rsplit_once('@').map(|(n, _)| n).unwrap_or(&key);
        if asked.iter().any(|a| a == name || a == &key) {
            receipts.remove(&key);
            removed.push(key);
        }
    }
    if !removed.is_empty() {
        save_receipts(project, &receipts)?;
    }
    removed.sort();
    Ok(removed)
}
