//! Audit (vulnerabilities) and SBOM generation.

use std::path::Path;

use crate::lockfile;
use crate::osv;
use crate::utils;

/// Run native audit via OSV and return vulnerability list.
pub fn native_audit() -> Result<Vec<osv::VulnRecord>, String> {
    let resolved = lockfile::read_resolved_from_dir(Path::new("."))
        .ok_or("No package-lock.json or bun.lock found. Run install first.")?;
    let mut all = Vec::new();
    for (name, version) in &resolved {
        match osv::query_vulnerabilities(name, version) {
            Ok(vulns) => all.extend(vulns),
            Err(_) => continue,
        }
    }
    Ok(all)
}

/// Run audit and return raw JSON bytes (for --json output). Uses native OSV.
pub fn run_audit_raw(_backend: crate::backend::Backend) -> Result<Vec<u8>, String> {
    let vulns = native_audit()?;
    let arr: Vec<serde_json::Value> = vulns
        .into_iter()
        .map(|v| {
            serde_json::json!({
                "id": v.id,
                "summary": v.summary,
                "severity": v.severity,
                "package": v.package_name,
                "version": v.package_version,
            })
        })
        .collect();
    let out = serde_json::json!({ "vulnerabilities": arr });
    serde_json::to_vec(&out).map_err(|e| e.to_string())
}

/// Run audit and print summary. Uses native OSV.
pub fn run_audit(_backend: crate::backend::Backend, quiet: bool) -> Result<(), String> {
    if !Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    utils::log("Running audit...");
    let vulns = native_audit()?;
    if quiet {
        if !vulns.is_empty() {
            return Err(format!("{} vulnerability(ies) found.", vulns.len()));
        }
        return Ok(());
    }
    if vulns.is_empty() {
        println!("No vulnerabilities found.");
        return Ok(());
    }
    println!("Found {} vulnerability(ies):", vulns.len());
    for v in &vulns {
        let sev = v.severity.as_deref().unwrap_or("unknown");
        println!("  {}@{} ({}): {} - {}", v.package_name, v.package_version, sev, v.id, v.summary);
    }
    Ok(())
}

/// Run audit fix: print upgrade suggestions (no backend). No automatic fix.
pub fn run_audit_fix(_backend: crate::backend::Backend, quiet: bool) -> Result<(), String> {
    if !Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    utils::log("Running audit fix...");
    let vulns = native_audit()?;
    if vulns.is_empty() {
        if !quiet {
            println!("No vulnerabilities to fix.");
        }
        return Ok(());
    }
    if !quiet {
        println!("Vulnerable packages (update manually or run jhol install <pkg>@latest):");
        let mut seen = std::collections::HashSet::new();
        for v in &vulns {
            let key = (v.package_name.as_str(), v.package_version.as_str());
            if seen.insert(key) {
                println!("  {}@{} - {}", v.package_name, v.package_version, v.id);
            }
        }
    }
    Ok(())
}

/// Run audit and fail if vulnerabilities are found. Useful for CI gating.
pub fn run_audit_gate(_backend: crate::backend::Backend) -> Result<(), String> {
    let vulns = native_audit()?;
    if vulns.is_empty() {
        return Ok(());
    }
    Err(format!("audit gate failed: {} vulnerability(ies) found", vulns.len()))
}

/// SBOM format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SbomFormat {
    CycloneDx,
    Simple,
}

/// Generate SBOM from package.json + lockfile. Returns JSON string.
pub fn generate_sbom(format: SbomFormat) -> Result<String, String> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err("No package.json in current directory.".to_string());
    }
    let deps = lockfile::read_package_json_deps(pj).ok_or("Could not read package.json.")?;
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    let specs = lockfile::resolve_deps_for_install(&deps, resolved.as_ref());
    let components: Vec<serde_json::Value> = specs
        .iter()
        .map(|s| {
            let (name, version) = if let Some(i) = s.rfind('@') {
                (s[..i].to_string(), s[i + 1..].to_string())
            } else {
                (s.clone(), "?".to_string())
            };
            (name, version)
        })
        .map(|(name, version)| {
            serde_json::json!({
                "name": name,
                "version": version,
            })
        })
        .collect();
    let out = match format {
        SbomFormat::CycloneDx => {
            serde_json::json!({
                "bomFormat": "CycloneDX",
                "specVersion": "1.4",
                "version": 1,
                "components": components,
            })
        }
        SbomFormat::Simple => serde_json::json!(components),
    };
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}
