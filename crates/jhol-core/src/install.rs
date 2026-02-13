use std::collections::HashSet;
use std::fs;
use std::path::Path;
use crate::backend::{self, Backend};
use crate::lockfile;
use crate::registry;
use crate::utils::{self, NPM_SHOW_TIMEOUT_SECS};

/// Package name without version: lodash@4 -> lodash, @scope/pkg@1.0 -> @scope/pkg
fn base_name(package: &str) -> &str {
    if let Some(idx) = package.rfind('@') {
        // "@scope/pkg" (no version) or paths containing scoped names have '/' after '@'.
        // A version suffix never contains '/'.
        if idx > 0 && !package[idx + 1..].contains('/') {
            return &package[..idx];
        }
    }
    package
}

/// Read version from node_modules/<base>/package.json (base may be @scope/pkg)
fn read_installed_version(base: &str) -> Option<String> {
    let path = Path::new("node_modules").join(base).join("package.json");
    let s = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("version")?.as_str().map(String::from)
}

pub struct InstallOptions {
    pub no_cache: bool,
    pub quiet: bool,
    pub backend: Backend,
    pub lockfile_only: bool,
    pub offline: bool,
    pub strict_lockfile: bool,
    /// When true, specs came from lockfile; skip npm show and use tarball URLs only (no packument).
    pub from_lockfile: bool,
    /// When true, never call Bun/npm; fail with clear error if native install fails.
    pub native_only: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            no_cache: false,
            quiet: false,
            backend: backend::resolve_backend(None),
            lockfile_only: false,
            offline: false,
            strict_lockfile: false,
            from_lockfile: false,
            native_only: true,
        }
    }
}

/// Only update lockfile (no node_modules). Uses native resolver and lockfile writer.
pub fn install_lockfile_only(_backend: Backend) -> Result<(), String> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let tree = crate::lockfile_write::resolve_full_tree(pj)?;
    let lock_path = Path::new("package-lock.json");
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree)?;
    Ok(())
}

/// Install dependencies from package.json (and optional package-lock.json or bun.lock). Returns list of specs to install.
/// If strict_lockfile is true, requires lockfile to exist and all deps to be in lockfile.
pub fn resolve_install_from_package_json(strict_lockfile: bool) -> Result<Vec<String>, String> {
    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Err("No package.json found in current directory.".to_string());
    }
    let deps = lockfile::read_package_json_deps(pj_path)
        .ok_or("Could not read package.json dependencies.")?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    if strict_lockfile {
        if resolved.is_none() {
            return Err("Strict lockfile required but no package-lock.json or bun.lock found. Run install without --frozen first.".to_string());
        }
        if !lockfile::lockfile_integrity_complete(Path::new(".")) {
            return Err("Strict lockfile: integrity entries missing. Run install without --frozen to regenerate lockfile with integrity.".to_string());
        }
        let r = resolved.as_ref().unwrap();
        for name in deps.keys() {
            if !r.contains_key(name) {
                return Err(format!("Strict lockfile: dependency {} not in lockfile. Run install without --frozen to update lockfile.", name));
            }
        }
    }

    // When lockfile URLs are available, prefer full resolved spec list (top-level + transitive)
    // so native lockfile/offline installs can be deterministic and complete.
    if let Some(mut specs) = lockfile::read_all_resolved_specs_from_dir(Path::new(".")) {
        if !specs.is_empty() {
            specs.sort();
            specs.dedup();
            return Ok(specs);
        }
    }

    Ok(lockfile::resolve_deps_for_install(&deps, resolved.as_ref()))
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with backend fallback.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), String> {
    let mut seen_packages = HashSet::new();
    let mut to_install_from_cache = Vec::new();
    let mut to_fetch = Vec::new();
    let mut missing_for_offline = Vec::new();

    for package in packages {
        let base = base_name(package);
        if seen_packages.contains(base) {
            if !options.quiet {
                println!("Warning: Multiple versions of {} requested.", base);
            }
        }
        seen_packages.insert(base.to_string());
        utils::log(&format!("Installing package: {}", package));

        if !options.no_cache {
            if let Some(tarball) = utils::get_cached_tarball(package) {
                if !options.quiet {
                    println!("Installing {} from cache...", package);
                }
                to_install_from_cache.push((package.to_string(), tarball));
                continue;
            }
        }
        if options.offline {
            missing_for_offline.push(package.to_string());
            continue;
        }
        to_fetch.push(package.to_string());
    }

    if !missing_for_offline.is_empty() {
        return Err(format!(
            "Offline mode: package(s) not in cache: {}. Run without --offline to fetch.",
            missing_for_offline.join(", ")
        ));
    }

    // Skip npm show when we trust the lockfile or frozen (zero packument)
    if !to_fetch.is_empty() && !options.from_lockfile && !options.strict_lockfile {
        let results = registry::parallel_validate_packages(&to_fetch, NPM_SHOW_TIMEOUT_SECS);
        let invalid: Vec<String> = results.iter().filter(|(_, ok)| !*ok).map(|(p, _)| p.clone()).collect();
        if !invalid.is_empty() {
            return Err(format!("Package(s) not found or invalid: {}", invalid.join(", ")));
        }
    }

    // Install from cache: link from unpacked store, or fall back to backend/copy
    if !to_install_from_cache.is_empty() {
        let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
        let node_modules = Path::new("node_modules");
        std::fs::create_dir_all(node_modules).map_err(|e| e.to_string())?;
        let mut fallback_tarballs = Vec::new();
        for (pkg, tarball_path) in &to_install_from_cache {
            let base = base_name(pkg);
            match registry::ensure_unpacked_in_store(tarball_path, &cache_dir) {
                Ok(unpacked) => {
                    if utils::link_package_from_store(&unpacked, node_modules, base).is_ok() {
                        utils::log(&format!("Installed {} from cache (link).", pkg));
                    } else if registry::extract_tarball(tarball_path, node_modules, base).is_ok() {
                        utils::log(&format!("Installed {} from cache (copy).", pkg));
                    } else {
                        fallback_tarballs.push((pkg.clone(), tarball_path.clone()));
                    }
                }
                Err(_) => fallback_tarballs.push((pkg.clone(), tarball_path.clone())),
            }
        }
        if !fallback_tarballs.is_empty() {
            if options.native_only {
                let pkgs: Vec<String> = fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
                return Err(format!(
                    "Native-only: could not link or extract from cache for: {}. Try JHOL_LINK=0 or run without --native-only.",
                    pkgs.join(", ")
                ));
            }
            let paths: Vec<std::path::PathBuf> = fallback_tarballs.iter().map(|(_, p)| p.clone()).collect();
            match backend::backend_install_tarballs(&paths, options.backend) {
                Ok(()) => {
                    for (pkg, _) in &fallback_tarballs {
                        utils::log(&format!("Installed {} from cache (backend).", pkg));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    if to_fetch.is_empty() {
        return Ok(());
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| e.to_string())?;

    let mut npm_fallback = Vec::new();
    let mut index_batch: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if options.from_lockfile {
        // Zero packument: use lockfile URLs and integrity when present, parallel download, then extract
        let (resolved_urls, resolved_integrity) = match lockfile::read_resolved_urls_and_integrity_from_dir(Path::new(".")) {
            Some((u, i)) => (u, i),
            None => (std::collections::HashMap::new(), std::collections::HashMap::new()),
        };
        let mut work: Vec<(String, String, Option<String>)> = Vec::new();
        for pkg in &to_fetch {
            if options.no_cache {
                npm_fallback.push(pkg.clone());
                continue;
            }
            let url = resolved_urls
                .get(pkg)
                .cloned()
                .or_else(|| {
                    let base = base_name(pkg);
                    let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("latest");
                    Some(lockfile::tarball_url_from_registry(base, version))
                });
            match url {
                Some(u) => {
                    let integrity = resolved_integrity.get(pkg).cloned();
                    work.push((pkg.clone(), u, integrity));
                }
                None => npm_fallback.push(pkg.clone()),
            }
        }
        const DL_CONCURRENCY: usize = 8;
        let mut download_results: Vec<(String, Result<String, String>)> = Vec::with_capacity(work.len());
        for chunk in work.chunks(DL_CONCURRENCY) {
            use std::sync::mpsc;
            use std::thread;
            let (tx, rx) = mpsc::channel();
            for (pkg, url, integrity) in chunk {
                let pkg = pkg.clone();
                let url = url.clone();
                let integrity = integrity.clone();
                let cache_dir = cache_dir.clone();
                let tx = tx.clone();
                thread::spawn(move || {
                    let res = registry::download_tarball_to_store_hash_only(
                        &url,
                        &cache_dir,
                        &pkg,
                        integrity.as_deref(),
                    );
                    let _ = tx.send((pkg, res));
                });
            }
            drop(tx);
            for (pkg, res) in rx {
                download_results.push((pkg, res));
            }
        }
        for (pkg, res) in download_results {
            match res {
                Ok(hash) => {
                    index_batch.insert(pkg.clone(), hash.clone());
                    let store_path = cache_dir.join("store").join(format!("{}.tgz", hash));
                    let base = base_name(&pkg);
                    if let Err(e) = registry::extract_tarball(&store_path, node_modules, base) {
                        let msg = format!("Extract failed for {}: {}", pkg, e);
                        utils::log(&msg);
                        npm_fallback.push(pkg);
                        continue;
                    }
                    if !options.quiet {
                        let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("");
                        println!("Installed {}@{} (native)", base, version);
                    }
                }
                Err(_) => npm_fallback.push(pkg),
            }
        }
        if !index_batch.is_empty() {
            let mut index = utils::read_store_index();
            index.extend(index_batch);
            utils::write_store_index(&index).map_err(|e| e.to_string())?;
        }
    } else {
        for pkg in &to_fetch {
            if options.no_cache {
                npm_fallback.push(pkg.clone());
                continue;
            }
            match registry::install_package_native(pkg, node_modules, &cache_dir, options) {
                Ok(()) => {}
                Err(_) => {
                    npm_fallback.push(pkg.clone());
                }
            }
        }
    }

    if npm_fallback.is_empty() {
        return Ok(());
    }

    if options.native_only {
        return Err(format!(
            "Native-only: install failed for: {}. Run without --native-only to use Bun/npm fallback.",
            npm_fallback.join(", ")
        ));
    }

    // Fallback: backend install for any that native failed
    let fetch_refs: Vec<&str> = npm_fallback.iter().map(|s| s.as_str()).collect();
    let mut attempts = 3;
    loop {
        match backend::backend_install(&fetch_refs, options.backend, options.lockfile_only) {
            Ok(()) => {
                let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
                for pkg in &npm_fallback {
                    let base = base_name(pkg);
                    if let Some(version) = read_installed_version(base) {
                        let _ = registry::fill_store_from_registry(base, &version, &cache_dir);
                    }
                    utils::log(&format!("Installed {} via backend.", pkg));
                }
                return Ok(());
            }
            Err(e) => {
                if attempts <= 1 {
                    return Err(e);
                }
                if !options.quiet {
                    eprintln!("Install failed, retrying in 2s...");
                }
            }
        }
        attempts -= 1;
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}
