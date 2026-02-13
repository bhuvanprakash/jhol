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
pub fn query_vulnerabilities(package_name: &str, version: &str) -> Result<Vec<VulnRecord>, String> {
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
