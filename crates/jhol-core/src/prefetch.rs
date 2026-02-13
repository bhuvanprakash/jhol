//! Prefetch: fill the store from lockfile without writing node_modules.

use std::path::Path;

use crate::lockfile;
use crate::registry;
use crate::utils;

/// Package name without version (for URL construction when missing from map).
fn base_name(package: &str) -> &str {
    if let Some(idx) = package.rfind('@') {
        if idx > 0 && !package[idx + 1..].contains('/') {
            return &package[..idx];
        }
    }
    package
}

/// Prefetch all lockfile dependencies into the store. Requires package.json and lockfile.
/// Does not create node_modules or run backend. Use before `jhol install --offline`.
pub fn prefetch_from_lockfile(quiet: bool) -> Result<(), String> {
    let specs = crate::install::resolve_install_from_package_json(true)?;
    if specs.is_empty() {
        if !quiet {
            println!("No dependencies to prefetch.");
        }
        return Ok(());
    }
    let (resolved_urls, resolved_integrity) = lockfile::read_resolved_urls_and_integrity_from_dir(Path::new("."))
        .ok_or("No package-lock.json or bun.lock found.")?;
    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let store_dir = cache_dir.join("store");
    std::fs::create_dir_all(&store_dir).map_err(|e| e.to_string())?;

    let mut fetched = 0;
    for spec in &specs {
        if utils::get_cached_tarball(spec).is_some() {
            continue;
        }
        let url = resolved_urls.get(spec).cloned().or_else(|| {
            let base = base_name(spec);
            let version = spec.rfind('@').map(|i| &spec[i + 1..]).unwrap_or("latest");
            Some(lockfile::tarball_url_from_registry(base, version))
        });
        let url = match url {
            Some(u) => u,
            None => continue,
        };
        let integrity = resolved_integrity.get(spec).cloned();
        if !quiet {
            println!("Prefetching {}...", spec);
        }
        registry::download_tarball_to_store(&url, &cache_dir, spec, None, integrity.as_deref())?;
        fetched += 1;
    }
    if !quiet && fetched > 0 {
        println!("Prefetched {} package(s).", fetched);
    }
    Ok(())
}
