//! Prefetch: fill the store from lockfile without writing node_modules.

use std::path::Path;

use crate::error_handling::JholError;
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
pub fn prefetch_from_lockfile(quiet: bool) -> Result<(), JholError> {
    let specs = crate::install::resolve_install_from_package_json(true)?;
    if specs.is_empty() {
        if !quiet {
            println!("No dependencies to prefetch.");
        }
        return Ok(());
    }
    let (resolved_urls, resolved_integrity) = lockfile::read_resolved_urls_and_integrity_from_dir(Path::new("."))
        .ok_or_else(|| crate::error_handling::utils::config_error("read_resolved_urls", Some("lockfile"), "No package-lock.json or bun.lock found"))?;
    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let store_dir = cache_dir.join("store");
    std::fs::create_dir_all(&store_dir).map_err(|e| crate::error_handling::utils::io_error("create_store_dir", Some("cache/store"), e))?;

    let mut work: Vec<(String, String, Option<String>)> = Vec::new();
    for spec in &specs {
        if utils::get_cached_tarball(spec).is_some() {
            continue;
        }
        let url = resolved_urls.get(spec).cloned().or_else(|| {
            let base = base_name(spec);
            let version = spec.rfind('@').map(|i| &spec[i + 1..]).unwrap_or("latest");
            Some(lockfile::tarball_url_from_registry(base, version))
        });
        if let Some(url) = url {
            let integrity = resolved_integrity.get(spec).cloned();
            work.push((spec.clone(), url, integrity));
        }
    }

    const PREFETCH_CONCURRENCY: usize = 8;
    let mut fetched = 0usize;
    let mut index_batch: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for chunk in work.chunks(PREFETCH_CONCURRENCY) {
        use std::sync::mpsc;
        use std::thread;
        let (tx, rx) = mpsc::channel();
        for (spec, url, integrity) in chunk {
            let spec = spec.clone();
            let url = url.clone();
            let integrity = integrity.clone();
            let cache_dir = cache_dir.clone();
            let tx = tx.clone();
            if !quiet {
                println!("Prefetching {}...", spec);
            }
            thread::spawn(move || {
                let res = registry::download_tarball_to_store_hash_only(
                    &url,
                    &cache_dir,
                    &spec,
                    integrity.as_deref(),
                );
                let _ = tx.send((spec, res));
            });
        }
        drop(tx);
        for (spec, res) in rx {
            let hash = match res {
                Ok(h) => h,
                Err(e) => return Err(crate::error_handling::utils::network_error("download_tarball", Some(&spec), e.to_string())),
            };
            index_batch.insert(spec, hash);
            fetched += 1;
        }
    }

    if !index_batch.is_empty() {
        let mut index = utils::read_store_index();
        index.extend(index_batch);
        utils::write_store_index(&index).map_err(|e| crate::error_handling::utils::io_error("write_store_index", Some("cache/index.json"), std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
    }

    if !quiet && fetched > 0 {
        println!("Prefetched {} package(s).", fetched);
    }
    Ok(())
}
