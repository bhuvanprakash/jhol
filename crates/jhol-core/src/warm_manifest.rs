//! Persistent warm-install verification manifest.
//!
//! Survives `rm -rf node_modules` because it lives in the cache directory.
//! Records the spec list of a successful install keyed by lockfile content
//! hash, so the first warm run after `rm -rf node_modules` can materialize
//! every package straight from the already-unpacked store (`store_unpacked`)
//! via a single O(1) `clonefile(2)` per package on macOS APFS — instead of
//! re-reading, decompressing, and re-extracting each tarball.
//!
//! Manifest is keyed by `lockfile_content_hash` so it invalidates when the
//! lockfile changes.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct WarmManifestEntry {
    pub spec: String,
}

fn manifest_path(lockfile_hash: &str) -> PathBuf {
    PathBuf::from(crate::utils::get_cache_dir())
        .join("warm-manifest")
        .join(format!("{}.json", lockfile_hash))
}

static CACHE: OnceLock<Mutex<HashMap<String, Vec<WarmManifestEntry>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, Vec<WarmManifestEntry>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load the manifest for a given lockfile hash. Returns `None` if the
/// manifest file doesn't exist or is corrupt.
pub(crate) fn load(lockfile_hash: &str) -> Option<Vec<WarmManifestEntry>> {
    {
        let guard = cache().lock();
        if let Some(entries) = guard.get(lockfile_hash) {
            return Some(entries.clone());
        }
    }
    let path = manifest_path(lockfile_hash);
    let bytes = std::fs::read(&path).ok()?;
    let entries: Vec<WarmManifestEntry> = serde_json::from_slice(&bytes).ok()?;
    let mut guard = cache().lock();
    guard.insert(lockfile_hash.to_string(), entries.clone());
    Some(entries)
}

/// Save the manifest for a given lockfile hash.
pub(crate) fn save(lockfile_hash: &str, entries: &[WarmManifestEntry]) {
    let path = manifest_path(lockfile_hash);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(entries) {
        let _ = std::fs::write(&path, bytes);
    }
    let mut guard = cache().lock();
    guard.insert(lockfile_hash.to_string(), entries.to_vec());
}

/// Compute the lockfile content hash used as the manifest key.
/// This is the same hash used by `lockfile_content_hash` in utils.
pub(crate) fn lockfile_hash() -> Option<String> {
    let lockfile = std::path::Path::new("package-lock.json");
    if !lockfile.exists() {
        return None;
    }
    let bytes = std::fs::read(lockfile).ok()?;
    Some(format!("{:x}", xxhash_rust::xxh3::xxh3_64(&bytes)))
}

/// Materialize a verified entry from `store_unpacked` to its placement path.
/// Tries a single O(1) clonefile directory clone first (macOS APFS), then
/// falls back to hardlink → reflink → copy per file. Store markers are
/// removed from the placement path. Returns Ok(()) on success.
pub(crate) fn materialize_from_store(
    entry: &crate::lockfile::LockfileInstallEntry,
    store_index: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let hash = store_index
        .get(&entry.spec)
        .ok_or_else(|| format!("No store hash for {}", entry.spec))?;
    let unpacked = crate::utils::sharded_unpacked_path(hash);
    if !crate::registry::store_extract_complete(&unpacked) {
        return Err(format!("store_unpacked incomplete for {}", entry.spec));
    }
    let install_path = std::path::Path::new(&entry.install_path);
    if install_path.join("package.json").is_file() {
        return Ok(());
    }
    if install_path.exists() {
        std::fs::remove_dir_all(install_path).map_err(|e| e.to_string())?;
    }
    if let Some(parent) = install_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Try clonefile first (O(1) directory clone on macOS APFS).
    // Falls back to hardlink-per-file, then copy-per-file.
    if clone_dir(&unpacked, install_path).is_err() {
        copy_tree_hardlink(&unpacked, install_path)?;
    }
    Ok(())
}

/// Clone a directory tree using clonefile(2) on macOS (O(1) copy-on-write).
/// Falls back to reflink on Linux, then hardlink-per-file.
#[cfg(target_os = "macos")]
pub(crate) fn clone_dir(src: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_canon = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    let dest_canon = if let Some(parent) = dest.parent() {
        let parent_canon = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
        if let Some(file_name) = dest.file_name() {
            parent_canon.join(file_name)
        } else {
            dest.to_path_buf()
        }
    } else {
        dest.to_path_buf()
    };

    let src_c = CString::new(src_canon.as_os_str().as_bytes())
        .map_err(|_| "invalid src path".to_string())?;
    let dest_c = CString::new(dest_canon.as_os_str().as_bytes())
        .map_err(|_| "invalid dest path".to_string())?;
    // clonefile(src, dest, flags) — CLONE_NOOWNERCOPY=0x02
    let ret = unsafe { libc::clonefile(src_c.as_ptr(), dest_c.as_ptr(), 0x02) };
    if ret == 0 {
        Ok(())
    } else {
        Err(format!(
            "clonefile failed: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn clone_dir(src: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    if reflink_copy::reflink(src, dest).is_ok() {
        return Ok(());
    }
    Err("clonefile/reflink not available".to_string())
}

/// Copy a directory tree using hardlink → reflink → copy fallback per file.
/// Skips `.jhol-extract-complete` and `.jhol-fingerprint` markers.
pub(crate) fn copy_tree_hardlink(src: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let read = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for ent in read.flatten() {
        let sp = ent.path();
        let name = ent.file_name();
        if name == ".jhol-extract-complete" || name == ".jhol-fingerprint" {
            continue;
        }
        let dp = dest.join(&name);
        if sp.is_dir() {
            copy_tree_hardlink(&sp, &dp)?;
        } else {
            // Try hardlink first (O(1) metadata, no data copy)
            if std::fs::hard_link(&sp, &dp).is_err() {
                // Try reflink (macOS APFS clonefile, Linux Btrfs)
                if reflink_copy::reflink(&sp, &dp).is_err() {
                    // Fall back to copy
                    std::fs::copy(&sp, &dp).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let hash = format!("test-hash-{}", std::process::id());
        let entries = vec![
            WarmManifestEntry {
                spec: "a@1.0.0".to_string(),
            },
            WarmManifestEntry {
                spec: "b@2.0.0".to_string(),
            },
        ];
        save(&hash, &entries);
        let loaded = load(&hash).expect("should load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].spec, "a@1.0.0");
        assert_eq!(loaded[1].spec, "b@2.0.0");
    }
}
