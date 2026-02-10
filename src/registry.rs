//! Native npm registry client: fetch metadata and tarballs via HTTP.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

const REGISTRY_URL: &str = "https://registry.npmjs.org";
const REQUEST_TIMEOUT_MS: u64 = 30_000;

fn registry_get(path: &str) -> Result<Vec<u8>, String> {
    let url = format!("{}/{}", REGISTRY_URL.trim_end_matches('/'), path.trim_start_matches('/'));
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status() != 200 {
        return Err(format!("registry returned {}", resp.status()));
    }
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Fetch package metadata from registry. Scoped: @scope/pkg -> @scope%2Fpkg
pub fn fetch_metadata(package: &str) -> Result<serde_json::Value, String> {
    let path = if package.starts_with('@') {
        format!("{}", package.replace('/', "%2F"))
    } else {
        package.to_string()
    };
    let body = registry_get(&path)?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Resolve version to a concrete semver (e.g. "latest" -> "1.2.3")
pub fn resolve_version(meta: &serde_json::Value, version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() || version == "latest" {
        let dist_tags = meta.get("dist-tags")?.as_object()?;
        return dist_tags.get("latest").and_then(|v| v.as_str()).map(String::from);
    }
    let versions = meta.get("versions")?.as_object()?;
    if versions.contains_key(version) {
        return Some(version.to_string());
    }
    // Try as tag
    let dist_tags = meta.get("dist-tags")?.as_object()?;
    if let Some(tag) = dist_tags.get(version) {
        return tag.as_str().map(String::from);
    }
    // TODO: semver range resolution (e.g. "^1.0" -> "1.2.3")
    None
}

/// Get tarball URL for a specific version from metadata
pub fn get_tarball_url(meta: &serde_json::Value, version: &str) -> Option<String> {
    let versions = meta.get("versions")?.as_object()?;
    let ver_obj = versions.get(version)?.as_object()?;
    let dist = ver_obj.get("dist")?.as_object()?;
    dist.get("tarball")?.as_str().map(String::from)
}

/// Download tarball from URL to a file; returns path
pub fn download_tarball(url: &str, dest: &Path) -> Result<PathBuf, String> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status() != 200 {
        return Err(format!("download returned {}", resp.status()));
    }
    let mut out = File::create(dest).map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
    Ok(dest.to_path_buf())
}

/// Extract .tgz to node_modules/<package_name>. Strips one top-level directory from tarball.
pub fn extract_tarball(tarball_path: &Path, node_modules_dir: &Path, package_name: &str) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let f = File::open(tarball_path).map_err(|e| e.to_string())?;
    let dec = GzDecoder::new(BufReader::new(f));
    let mut archive = Archive::new(dec);

    let dest = node_modules_dir.join(package_name);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path_str = path.to_string_lossy();
        // Tarballs are usually <name>/package.json etc.; strip first component
        let parts: Vec<&str> = path_str.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let rel: String = parts[1..].join(std::path::MAIN_SEPARATOR_STR);
        if rel.is_empty() {
            continue;
        }
        let out_path = dest.join(&rel);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out_path.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            entry.unpack(&out_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Install a single package via native registry (fetch metadata, download tarball, extract).
/// Returns Ok(()) on success, Err on failure (caller can fall back to npm).
pub fn install_package_native(
    package: &str,
    node_modules: &Path,
    cache_dir: &Path,
    options: &crate::install::InstallOptions,
) -> Result<(), String> {
    let meta = fetch_metadata(package)?;
    let (base_name, version_req) = if package.contains('@') && !package.starts_with('@') {
        let mut parts = package.splitn(2, '@');
        let base = parts.next().unwrap_or(package);
        let ver = parts.next().unwrap_or("latest");
        (base, ver)
    } else if package.starts_with('@') {
        let idx = package.rfind('@').unwrap_or(0);
        if idx > 0 {
            (package[..idx].trim_end_matches('@'), package[idx + 1..].trim())
        } else {
            (package, "latest")
        }
    } else {
        (package, "latest")
    };
    let version = resolve_version(&meta, version_req).ok_or_else(|| format!("could not resolve version {}", version_req))?;
    let tarball_url = get_tarball_url(&meta, &version).ok_or("no tarball in metadata")?;

    let pkg_key = format!("{}@{}", base_name, version);
    let store_dir = cache_dir.join("store");
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;

    let tmp = cache_dir.join(format!("tmp-{}.tgz", std::process::id()));
    download_tarball(&tarball_url, &tmp).map_err(|e| format!("download: {}", e))?;
    let hash = crate::utils::content_hash(&tmp).map_err(|e| e.to_string())?;
    let store_file = store_dir.join(format!("{}.tgz", hash));
    std::fs::rename(&tmp, &store_file).or_else(|_| std::fs::copy(&tmp, &store_file).map(|_| ())).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    let mut index = crate::utils::read_store_index();
    index.insert(pkg_key, hash.clone());
    crate::utils::write_store_index(&index).map_err(|e| e.to_string())?;

    let store_file = store_dir.join(format!("{}.tgz", hash));
    extract_tarball(&store_file, node_modules, base_name)?;
    if !options.quiet {
        println!("Installed {}@{} (native)", base_name, version);
    }
    Ok(())
}
