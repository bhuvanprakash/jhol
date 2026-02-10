use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Result, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;
use chrono::Local;
use sha2::{Sha256, Digest};

pub const LOG_FILE: &str = "logs.txt";
pub const NPM_SHOW_TIMEOUT_SECS: u64 = 15;
pub const NPM_INSTALL_TIMEOUT_SECS: u64 = 120;

/// Returns the path to the cache directory. Uses JHOL_CACHE_DIR if set;
/// otherwise Windows: %USERPROFILE%\.jhol-cache, Unix: $HOME/.jhol-cache
pub fn get_cache_dir() -> String {
    if let Ok(dir) = env::var("JHOL_CACHE_DIR") {
        return dir;
    }
    let base = if cfg!(target_os = "windows") {
        env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string())
    } else {
        env::var("HOME").unwrap_or_else(|_| ".".to_string())
    };
    let sep = if cfg!(target_os = "windows") { "\\" } else { "/" };
    format!("{}{}.jhol-cache", base, sep)
}

pub fn init_cache() -> Result<()> {
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;

    let log_path = PathBuf::from(format!("{}/{}", cache_dir, LOG_FILE));
    if !log_path.exists() {
        File::create(&log_path)?;
    }

    Ok(())
}

fn is_quiet() -> bool {
    if env::var("JHOL_QUIET").map(|v| v == "1" || v == "true").unwrap_or(false) {
        return true;
    }
    env::var("JHOL_LOG")
        .map(|v| v.to_lowercase() == "quiet" || v.to_lowercase() == "error")
        .unwrap_or(false)
}

pub fn log(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_message = format!("[{}] {}", timestamp, message);

    // When -q/--quiet or JHOL_LOG=quiet, never print logs to stdout (only to file)
    if !is_quiet() {
        println!("{}", log_message);
    }

    let log_path = format!("{}/{}", get_cache_dir(), LOG_FILE);

    let mut should_write = true;
    if let Ok(contents) = fs::read_to_string(&log_path) {
        if let Some(last_line) = contents.lines().last() {
            if last_line == log_message {
                should_write = false;
            }
        }
    }

    if should_write {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(file, "{}", log_message);
        }
    }
}

pub fn log_error(message: &str) {
    eprintln!("{}", message);
    log(message);
}

pub fn format_cache_name(package: &str) -> String {
    package.replace('@', "-")
}

/// Convert stem like "lodash-4.17.21" or "@scope/pkg-1.0.0" to spec "lodash@4.17.21" / "@scope/pkg@1.0.0".
fn stem_to_spec(stem: &str) -> String {
    if let Some(pos) = stem.rfind('-') {
        let suffix = &stem[pos + 1..];
        if suffix.chars().any(|c| c.is_ascii_digit()) {
            return format!("{}@{}", &stem[..pos], suffix);
        }
    }
    stem.replace('-', "@")
}

fn cache_dir_path() -> PathBuf {
    PathBuf::from(get_cache_dir())
}

/// Content-addressable store dir: cache_dir/store/
fn store_dir() -> PathBuf {
    cache_dir_path().join("store")
}

/// Index path: cache_dir/store_index.json (pkg@version -> hash)
fn store_index_path() -> PathBuf {
    cache_dir_path().join("store_index.json")
}

pub fn read_store_index() -> HashMap<String, String> {
    let path = store_index_path();
    if !path.exists() {
        return HashMap::new();
    }
    let s = match fs::read_to_string(&path) {
        Ok(x) => x,
        Err(_) => return HashMap::new(),
    };
    serde_json::from_str(&s).unwrap_or_default()
}

pub fn write_store_index(map: &HashMap<String, String>) -> Result<()> {
    let path = store_index_path();
    let s = serde_json::to_string_pretty(map).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(path, s)?;
    Ok(())
}

/// SHA256 hash of file (hex)
pub fn content_hash(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// SHA256 hash of lockfile content for CI cache key. Prefers bun.lock then package-lock.json.
pub fn lockfile_content_hash(dir: &Path) -> Option<String> {
    let bun = dir.join("bun.lock");
    let npm = dir.join("package-lock.json");
    let path = if bun.exists() {
        bun
    } else if npm.exists() {
        npm
    } else {
        return None;
    };
    content_hash(&path).ok()
}

/// Returns the path to a cached tarball if present.
/// 1) Content-addressable store (store_index.json + store/<hash>.tgz)
/// 2) Legacy: pkg-version.tgz in cache root
pub fn get_cached_tarball(package: &str) -> Option<PathBuf> {
    let cache_dir = cache_dir_path();
    if !cache_dir.exists() {
        return None;
    }
    let base_name = package.split('@').next().unwrap_or(package);
    let versioned_key = format_cache_name(package);

    // 1) Content-addressable store
    let index = read_store_index();
    let key = if package.contains('@') {
        package.to_string()
    } else {
        // Try to find any version for this package in index
        for (k, hash) in &index {
            if k.starts_with(&format!("{}@", base_name)) {
                let store_file = store_dir().join(format!("{}.tgz", hash));
                if store_file.exists() {
                    return Some(store_file);
                }
            }
        }
        String::new()
    };
    if !key.is_empty() {
        if let Some(hash) = index.get(&key) {
            let store_file = store_dir().join(format!("{}.tgz", hash));
            if store_file.exists() {
                return Some(store_file);
            }
        }
    }

    // 2) Legacy: exact version in cache root
    let exact = cache_dir.join(format!("{}.tgz", versioned_key));
    if exact.exists() {
        return Some(exact);
    }

    // 3) Legacy: no version - any pkg-*.tgz
    if !package.contains('@') {
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{}-", base_name)) && name.ends_with(".tgz") && !name.contains("store") {
                    return Some(e.path());
                }
            }
        }
    }

    None
}

#[allow(dead_code)]
pub fn is_package_cached(package: &str) -> bool {
    get_cached_tarball(package).is_some()
}

/// Store a package in the cache (content-addressable + legacy path).
pub fn cache_package_tarball(base_name: &str, version: &str) -> Result<PathBuf> {
    let cache_dir = cache_dir_path();
    fs::create_dir_all(&cache_dir)?;
    fs::create_dir_all(store_dir())?;

    let output = run_command_timeout(
        "npm",
        &["pack", &format!("{}@{}", base_name, version), "--silent"],
        NPM_SHOW_TIMEOUT_SECS,
    )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("npm pack failed: {}", stderr),
        ));
    }

    let tgz_name = format!("{}-{}.tgz", base_name, version);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let from = cwd.join(&tgz_name);

    if !from.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "npm pack did not create tarball"));
    }

    // Content-addressable: hash and store in store/<hash>.tgz
    let hash = content_hash(&from)?;
    let store_file = store_dir().join(format!("{}.tgz", hash));
    fs::copy(&from, &store_file).map(|_| ()).or_else(|_| fs::rename(&from, &store_file))?;
    let _ = fs::remove_file(&from);

    let pkg_key = format!("{}@{}", base_name, version);
    let mut index = read_store_index();
    index.insert(pkg_key, hash.clone());
    write_store_index(&index)?;

    // Legacy path for backward compatibility
    let key = format!("{}-{}", base_name, version.replace('@', "-"));
    let dest = cache_dir.join(format!("{}.tgz", key));
    let _ = fs::hard_link(&store_file, &dest).or_else(|_| fs::copy(&store_file, &dest).map(|_| ()));

    log(&format!("Cached package: {}@{}", base_name, version));
    Ok(store_file)
}

/// List all cached package tarballs (pkg@version or legacy base names)
pub fn list_cached_packages() -> Result<Vec<String>> {
    let mut names: Vec<String> = read_store_index().into_keys().collect();
    let cache_dir = cache_dir_path();
    if cache_dir.exists() {
        for e in fs::read_dir(&cache_dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tgz") && name != "store_index.json" {
                let base = name.trim_end_matches(".tgz");
                if !names.contains(&base.to_string()) && !base.contains("/") {
                    names.push(base.to_string());
                }
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Remove all cached tarballs (store + legacy). Keeps logs.
pub fn cache_clean() -> Result<usize> {
    let cache_dir = cache_dir_path();
    if !cache_dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for e in fs::read_dir(&cache_dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tgz") {
            if fs::remove_file(e.path()).is_ok() {
                removed += 1;
            }
        }
    }
    let store = store_dir();
    if store.exists() {
        for e in fs::read_dir(&store)? {
            let e = e?;
            if e.path().extension().map(|x| x == "tgz").unwrap_or(false) && fs::remove_file(e.path()).is_ok() {
                removed += 1;
            }
        }
    }
    let _ = fs::remove_file(store_index_path());
    Ok(removed)
}

/// Return (total size in bytes, count of tarballs) for the cache (store + legacy .tgz in root).
pub fn cache_size_bytes() -> Result<(u64, usize)> {
    let cache_dir = cache_dir_path();
    let mut total: u64 = 0;
    let mut count = 0;
    if cache_dir.exists() {
        for e in fs::read_dir(&cache_dir)? {
            let e = e?;
            let path = e.path();
            if path.extension().map(|x| x == "tgz").unwrap_or(false) {
                if let Ok(m) = fs::metadata(&path) {
                    total += m.len();
                    count += 1;
                }
            }
        }
    }
    let store = store_dir();
    if store.exists() {
        for e in fs::read_dir(&store)? {
            let e = e?;
            let path = e.path();
            if path.extension().map(|x| x == "tgz").unwrap_or(false) {
                if let Ok(m) = fs::metadata(&path) {
                    total += m.len();
                    count += 1;
                }
            }
        }
    }
    Ok((total, count))
}

/// Prune cache: remove store tarballs not referenced by index. If keep_recent is Some(N), keep only N most recently used (by mtime).
pub fn cache_prune(keep_recent: Option<usize>) -> Result<usize> {
    let mut index = read_store_index();
    let store = store_dir();
    if !store.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    let mut entries: Vec<(PathBuf, std::time::SystemTime, String)> = Vec::new();
    for e in fs::read_dir(&store)? {
        let e = e?;
        let path = e.path();
        if path.extension().map(|x| x == "tgz").unwrap_or(false) {
            let hash = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let in_index = index.values().any(|v| v == &hash);
            if !in_index {
                if fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            } else if let Ok(meta) = fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    entries.push((path, mtime, hash));
                }
            }
        }
    }
    if let Some(n) = keep_recent {
        if entries.len() > n {
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            let to_remove = entries.split_off(n);
            let keep_hashes: std::collections::HashSet<String> = entries.into_iter().map(|(_, _, h)| h).collect();
            index.retain(|_, hash| keep_hashes.contains(hash));
            write_store_index(&index)?;
            for (path, _, _) in to_remove {
                if fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

/// Export cache entries needed for current project (package.json + lockfile) into dir. Writes manifest.json for import. Returns number of files copied.
pub fn cache_export(dir: &Path) -> Result<usize> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No package.json in current directory",
        ));
    }
    let deps = crate::lockfile::read_package_json_deps(pj).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Could not read package.json deps")
    })?;
    if deps.is_empty() {
        return Ok(0);
    }
    let resolved = crate::lockfile::read_resolved_from_dir(Path::new("."));
    let specs = crate::lockfile::resolve_deps_for_install(&deps, resolved.as_ref());
    fs::create_dir_all(dir)?;
    let mut count = 0;
    let mut manifest: Vec<(String, String)> = Vec::new();
    for spec in specs {
        if let Some(path) = get_cached_tarball(&spec) {
            let name = format!("{}.tgz", format_cache_name(&spec));
            let dest = dir.join(&name);
            fs::copy(&path, &dest)?;
            manifest.push((spec, name));
            count += 1;
        }
    }
    let manifest_json: Vec<serde_json::Value> = manifest
        .iter()
        .map(|(spec, file)| {
            serde_json::json!({ "spec": spec, "file": file })
        })
        .collect();
    let manifest_path = dir.join("manifest.json");
    let s = serde_json::to_string_pretty(&manifest_json)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(manifest_path, s)?;
    Ok(count)
}

/// Import tarballs from dir into cache. Expects manifest.json (from cache export) or .tgz files named pkg-version.tgz.
/// Returns number of packages imported.
pub fn cache_import(dir: &Path) -> Result<usize> {
    if !dir.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Export directory does not exist",
        ));
    }
    fs::create_dir_all(store_dir())?;
    let mut index = read_store_index();
    let mut count = 0;
    let manifest_path = dir.join("manifest.json");
    if manifest_path.exists() {
        let s = fs::read_to_string(&manifest_path)?;
        let entries: Vec<serde_json::Value> = serde_json::from_str(&s).unwrap_or_default();
        for entry in entries {
            let spec = entry.get("spec").and_then(|v| v.as_str()).unwrap_or("");
            let file = entry.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if spec.is_empty() || file.is_empty() {
                continue;
            }
            let path = dir.join(file);
            if !path.exists() {
                continue;
            }
            let hash = content_hash(&path)?;
            let store_file = store_dir().join(format!("{}.tgz", hash));
            if !store_file.exists() {
                fs::copy(&path, &store_file)?;
            }
            index.insert(spec.to_string(), hash);
            count += 1;
        }
    } else {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            if path.extension().map(|x| x == "tgz").unwrap_or(false) {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.is_empty() {
                    continue;
                }
                let hash = content_hash(&path)?;
                let store_file = store_dir().join(format!("{}.tgz", hash));
                if !store_file.exists() {
                    fs::copy(&path, &store_file)?;
                }
                let pkg_key = stem_to_spec(stem);
                index.insert(pkg_key, hash);
                count += 1;
            }
        }
    }
    write_store_index(&index)?;
    Ok(count)
}

/// Run a command with a timeout. On timeout the process is killed and an error is returned.
pub fn run_command_timeout(program: &str, args: &[&str], timeout_secs: u64) -> Result<Output> {
    let child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let pid = child.id();
    let kill_handle = thread::spawn(move || {
        thread::sleep(Duration::from_secs(timeout_secs));
        #[cfg(unix)]
        {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output();
        }
    });

    let out = child.wait_with_output();
    let _ = kill_handle.join();
    out
}

/// Run npm show with timeout (for package validation)
pub fn npm_show_timeout(package: &str, timeout_secs: u64) -> Result<Output> {
    run_command_timeout("npm", &["show", package, "name"], timeout_secs)
}

/// Max concurrency for parallel npm show / cache checks
const PARALLEL_CONCURRENCY: usize = 8;

/// Run npm show for multiple packages in parallel. Returns vec of (package, is_valid).
pub fn parallel_validate_packages(packages: &[String], timeout_secs: u64) -> Vec<(String, bool)> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::channel();
    for chunk in packages.chunks(PARALLEL_CONCURRENCY) {
        let chunk: Vec<String> = chunk.to_vec();
        let tx = tx.clone();
        thread::spawn(move || {
            for pkg in chunk {
                let ok = npm_show_timeout(&pkg, timeout_secs)
                    .map(|out| out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty())
                    .unwrap_or(false);
                let _ = tx.send((pkg, ok));
            }
        });
    }
    drop(tx);
    rx.into_iter().collect()
}

/// Run npm install with timeout
pub fn npm_install_timeout(args: &[&str], timeout_secs: u64) -> Result<Output> {
    let mut a = vec!["install"];
    a.extend(args);
    run_command_timeout("npm", &a, timeout_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_cache_name() {
        assert_eq!(format_cache_name("lodash"), "lodash");
        assert_eq!(format_cache_name("lodash@4.17.21"), "lodash-4.17.21");
        assert_eq!(format_cache_name("@scope/pkg@1.0.0"), "-scope/pkg-1.0.0");
    }

    #[test]
    fn test_get_cache_dir_non_empty() {
        let dir = get_cache_dir();
        assert!(!dir.is_empty());
        assert!(dir.contains("jhol-cache") || dir.contains(".jhol-cache"));
    }

    #[test]
    fn test_is_package_cached_no_dir() {
        // With a non-existent path in cache dir, should be false
        assert!(!is_package_cached("nonexistent-package-xyz-123"));
    }
}
