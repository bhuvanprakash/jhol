//! Audit (vulnerabilities) and SBOM generation.

use std::path::Path;

use crate::backend::{self, Backend};
use crate::lockfile;
use crate::utils;

/// Run audit and return raw JSON bytes (for --json output).
pub fn run_audit_raw(backend: Backend) -> Result<Vec<u8>, String> {
    backend::backend_audit(backend)
}

/// Run audit and print summary. Returns Ok(()) on success (no error running audit); vulns may still be present.
pub fn run_audit(backend: Backend, quiet: bool) -> Result<(), String> {
    if !Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    utils::log("Running audit...");
    let json_bytes = backend::backend_audit(backend)?;
    let s = String::from_utf8_lossy(&json_bytes);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap_or(serde_json::Value::Null);
    if quiet {
        let total = v.get("vulnerabilities")
            .and_then(|x| x.as_object())
            .map(|o| o.len())
            .unwrap_or_else(|| v.get("metadata").and_then(|m| m.get("vulnerabilities")).and_then(|x| x.as_object()).map(|o| o.len()).unwrap_or(0));
        if total > 0 {
            return Err(format!("{} vulnerability(ies) found.", total));
        }
        return Ok(());
    }
    let vulns = v.get("vulnerabilities").or(v.get("metadata").and_then(|m| m.get("vulnerabilities")));
    if let Some(obj) = vulns.and_then(|x| x.as_object()) {
        if obj.is_empty() {
            println!("No vulnerabilities found.");
            return Ok(());
        }
        println!("Found {} vulnerability(ies):", obj.len());
        for (pkg, info) in obj {
            let severity = info.get("severity").and_then(|s| s.as_str()).unwrap_or("unknown");
            let title = info.get("title").and_then(|s| s.as_str()).unwrap_or("");
            println!("  {} ({}): {}", pkg, severity, title);
        }
    } else {
        println!("{}", s.trim());
    }
    Ok(())
}

/// Run audit fix.
pub fn run_audit_fix(backend: Backend, quiet: bool) -> Result<(), String> {
    if !Path::new("package.json").exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    utils::log("Running audit fix...");
    backend::backend_audit_fix(backend)?;
    if !quiet {
        println!("Audit fix completed.");
    }
    Ok(())
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
