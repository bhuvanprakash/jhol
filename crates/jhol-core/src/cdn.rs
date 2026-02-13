//! Optional CDN / ESM-style fetch: produce esm.sh URLs for packages (no full install).
//! Does not replace the main install path; no native addons or full npm semantics.

/// Build esm.sh URL for a package (e.g. lodash@4 or lodash -> https://esm.sh/lodash@4 or latest).
pub fn esm_sh_url(package: &str, version: Option<&str>) -> String {
    const ESM_SH: &str = "https://esm.sh";
    let spec = match version {
        Some(v) => format!("{}@{}", package.trim(), v.trim()),
        None => package.trim().to_string(),
    };
    format!("{}/{}", ESM_SH.trim_end_matches('/'), spec)
}

/// Fetch ESM bundle from URL to a file (e.g. for one-off scripts). Uses http_client.
pub fn fetch_esm_to_file(url: &str, dest: &std::path::Path) -> Result<(), String> {
    crate::http_client::get_to_file(url, dest).map(|_| ())
}
