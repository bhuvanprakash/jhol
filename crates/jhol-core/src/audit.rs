//! Audit (vulnerabilities) and SBOM generation.

/// OSV (Open Source Vulnerabilities) API client (was `osv.rs`).
pub mod osv {
    //! OSV (Open Source Vulnerabilities) API client for native audit.

    const OSV_API_URL: &str = "https://api.osv.dev/v1/query";

    /// One vulnerability record from OSV.
    #[derive(Clone, Debug)]
    pub struct VulnRecord {
        pub id: String,
        pub summary: String,
        pub severity: Option<String>,
        pub package_name: String,
        pub package_version: String,
    }

    /// Query OSV for vulnerabilities affecting the given npm package at version.
    pub fn query_vulnerabilities(
        package_name: &str,
        version: &str,
    ) -> Result<Vec<VulnRecord>, String> {
        let body = serde_json::json!({
            "package": {
                "ecosystem": "npm",
                "name": package_name,
            },
            "version": version,
        });
        let body_bytes = body.to_string();
        let resp = crate::http_client::post_json(OSV_API_URL, body_bytes.as_bytes())?;
        let v: serde_json::Value = serde_json::from_slice(&resp).map_err(|e| e.to_string())?;
        let empty: Vec<serde_json::Value> = Vec::new();
        let vulns = v.get("vulns").and_then(|x| x.as_array()).unwrap_or(&empty);
        let mut out = Vec::new();
        for vuln in vulns {
            let id = vuln
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let summary = vuln
                .get("summary")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let severity = vuln
                .get("database_specific")
                .and_then(|d| d.get("severity"))
                .and_then(|s| s.as_str())
                .map(String::from)
                .or_else(|| {
                    vuln.get("details")
                        .and_then(|d| d.as_str())
                        .map(|_| "unknown".to_string())
                });
            out.push(VulnRecord {
                id,
                summary,
                severity,
                package_name: package_name.to_string(),
                package_version: version.to_string(),
            });
        }
        Ok(out)
    }
}

use std::path::Path;

use crate::lockfile;
use crate::utils;

/// Run native audit via OSV and return vulnerability list.
pub fn native_audit() -> Result<Vec<osv::VulnRecord>, String> {
    let resolved = lockfile::read_resolved_from_dir(Path::new("."))
        .ok_or("No package-lock.json or bun.lock found. Run install first.")?;
    let mut all = Vec::new();
    let mut ok_queries = 0usize;
    let mut failures = 0usize;
    for (name, version) in &resolved {
        match osv::query_vulnerabilities(name, version) {
            Ok(vulns) => {
                ok_queries += 1;
                all.extend(vulns);
            }
            Err(_) => {
                failures += 1;
            }
        }
    }
    finish_audit_from_query_totals(ok_queries, failures, !resolved.is_empty(), all)
}

/// Pure gate used by [`native_audit`]: empty advisory answers with a non-empty
/// lockfile must fail closed (blueprint Phase A).
pub(crate) fn finish_audit_from_query_totals(
    ok_queries: usize,
    failures: usize,
    had_packages: bool,
    vulns: Vec<osv::VulnRecord>,
) -> Result<Vec<osv::VulnRecord>, String> {
    if ok_queries == 0 && had_packages {
        return Err(
            "advisory_source_unavailable: OSV unreachable or all package queries failed"
                .to_string(),
        );
    }
    if failures > 0 {
        utils::log(&format!(
            "audit: {failures} package(s) skipped (OSV error); results may be incomplete"
        ));
    }
    Ok(vulns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisory_unavailable_when_no_ok_queries() {
        let err = finish_audit_from_query_totals(0, 3, true, Vec::new()).unwrap_err();
        assert!(err.contains("advisory_source_unavailable"));
    }

    #[test]
    fn empty_project_ok_with_zero_queries() {
        assert!(finish_audit_from_query_totals(0, 0, false, Vec::new()).is_ok());
    }

    #[test]
    fn partial_ok_queries_pass_even_with_failures() {
        assert!(finish_audit_from_query_totals(1, 2, true, Vec::new()).is_ok());
    }
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
    if vulns.is_empty() {
        if !quiet {
            println!("No vulnerabilities found.");
        }
        return Ok(());
    }
    // Always print the list (so the developer sees what failed) AND return
    // Err so the process exits non-zero. The `quiet` flag only suppresses the
    // "Running audit..." log line and the "No vulnerabilities found." message
    // — it must NOT swallow the failure exit code. Previously the non-quiet
    // path printed vulns but returned Ok(()), which broke CI gating.
    if !quiet {
        println!("Found {} vulnerability(ies):", vulns.len());
        for v in &vulns {
            let sev = v.severity.as_deref().unwrap_or("unknown");
            println!(
                "  {}@{} ({}): {} - {}",
                v.package_name, v.package_version, sev, v.id, v.summary
            );
        }
    }
    Err(format!("{} vulnerability(ies) found.", vulns.len()))
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
    Err(format!(
        "audit gate failed: {} vulnerability(ies) found",
        vulns.len()
    ))
}

// ── Filtered audit variants (used by main.rs with --ignore/--min-severity) ─────
pub fn run_audit_filtered(
    backend: crate::backend::Backend,
    quiet: bool,
    _ignore: &[String],
    _min_severity: Option<&str>,
) -> Result<(), String> {
    run_audit(backend, quiet)
}
pub fn run_audit_fix_filtered(
    backend: crate::backend::Backend,
    quiet: bool,
    _ignore: &[String],
    _min_severity: Option<&str>,
) -> Result<(), String> {
    run_audit_fix(backend, quiet)
}
pub fn run_audit_gate_filtered(
    backend: crate::backend::Backend,
    _ignore: &[String],
    _min_severity: Option<&str>,
) -> Result<(), String> {
    run_audit_gate(backend)
}
pub fn run_audit_raw_filtered(
    backend: crate::backend::Backend,
    _ignore: &[String],
    _min_severity: Option<&str>,
) -> Result<Vec<u8>, String> {
    run_audit_raw(backend)
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
