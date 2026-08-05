//! Feature-sheaf selective extract — Soul Path quality plane.
//!
//! Default stalks: legal + types + runtime + bin.
//! Store stays full; project may project a sheaf section.
//! LICENSE / *.d.ts always kept on default path.

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tar::Archive;

use crate::utils;

/// Feature stalks for selective project materialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureSheaf {
    /// legal + types + runtime + bin (default)
    Default,
    /// everything
    Full,
    /// runtime + bin only
    Minimal,
}

impl FeatureSheaf {
    pub fn from_env() -> Self {
        match std::env::var("JHOL_EXTRACT_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "full" | "strict" => Self::Full,
            "minimal" | "min" => Self::Minimal,
            _ => Self::Default,
        }
    }
}

/// Always keep (legal + types + core).
const LEGAL_TYPES_CORE: &[&str] = &[
    "package.json",
    "license",
    "license.md",
    "license.txt",
    "licence",
    "copying",
    "notice",
    "notice.md",
];

/// Runtime / bin directory stalks.
const RUNTIME_DIRS: &[&str] = &["dist/", "lib/", "src/", "bin/", "build/", "esm/", "cjs/"];

/// Never extract on Default/Minimal (not on Full).
const SKIP_NOISE: &[&str] = &[
    "test/",
    "tests/",
    "__tests__/",
    "coverage/",
    ".github/",
    ".gitlab/",
    "docs/",
    "example/",
    "examples/",
    "benchmark/",
    "benchmarks/",
    ".npmignore",
    ".gitignore",
    ".travis.yml",
    ".circleci/",
    "makefile",
];

pub fn extract_selective(
    tarball_path: &Path,
    dest_dir: &Path,
    package_name: &str,
) -> Result<usize, String> {
    let package_name = utils::sanitize_package_name(package_name)
        .ok_or_else(|| format!("Invalid package name: {}", package_name))?;
    extract_selective_to_path(tarball_path, &dest_dir.join(package_name))
}

pub fn extract_selective_to_path(
    tarball_path: &Path,
    package_root: &Path,
) -> Result<usize, String> {
    extract_with_sheaf(tarball_path, package_root, FeatureSheaf::from_env())
}

/// Prefer linking/copying from full store_unpacked with feature filter.
pub fn project_from_store(
    store_unpacked: &Path,
    package_root: &Path,
    sheaf: FeatureSheaf,
) -> Result<usize, String> {
    if !store_unpacked.exists() {
        return Err(format!(
            "store unpacked missing: {}",
            store_unpacked.display()
        ));
    }
    // KPSL default: whole-dir link for Full and Default (warm path speed).
    // Minimal still filters files for size-sensitive installs.
    if sheaf == FeatureSheaf::Full || sheaf == FeatureSheaf::Default {
        return link_or_copy_tree(store_unpacked, package_root);
    }

    fs::create_dir_all(package_root).map_err(|e| e.to_string())?;
    let mut count = 0usize;
    let mut entry_extra: HashSet<String> = HashSet::new();

    // First pass: find package.json for entry-point discovery.
    let pj = store_unpacked.join("package.json");
    if pj.exists() {
        if let Ok(raw) = fs::read_to_string(&pj) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                collect_entry_paths(&v, &mut entry_extra);
            }
        }
    }

    walk_project(
        store_unpacked,
        store_unpacked,
        package_root,
        sheaf,
        &entry_extra,
        &mut count,
    )?;
    Ok(count)
}

fn walk_project(
    root: &Path,
    cur: &Path,
    dest_root: &Path,
    sheaf: FeatureSheaf,
    entry_extra: &HashSet<String>,
    count: &mut usize,
) -> Result<(), String> {
    let entries = fs::read_dir(cur).map_err(|e| e.to_string())?;
    for ent in entries.flatten() {
        let path = ent.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            if sheaf != FeatureSheaf::Full && should_skip_noise(&rel) {
                continue;
            }
            walk_project(root, &path, dest_root, sheaf, entry_extra, count)?;
            continue;
        }
        if !should_keep_file(&rel, sheaf, entry_extra) {
            continue;
        }
        let dest = dest_root.join(path.strip_prefix(root).unwrap());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if crate::cas::link_package(&path, &dest).is_err() {
            fs::copy(&path, &dest).map_err(|e| e.to_string())?;
        }
        *count += 1;
    }
    Ok(())
}

fn link_or_copy_tree(src: &Path, dest: &Path) -> Result<usize, String> {
    if dest.exists() {
        let _ = fs::remove_dir_all(dest);
        let _ = fs::remove_file(dest);
    }
    // Prefer whole-dir symlink (one syscall).
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(src, dest).is_ok() {
            return Ok(1);
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(src, dest).is_ok() {
            return Ok(1);
        }
    }
    // Fall back to recursive hardlink/copy.
    let mut count = 0usize;
    copy_tree_recursive(src, dest, &mut count)?;
    Ok(count)
}

fn copy_tree_recursive(src: &Path, dest: &Path, count: &mut usize) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for ent in fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let sp = ent.path();
        let dp = dest.join(ent.file_name());
        if sp.is_dir() {
            copy_tree_recursive(&sp, &dp, count)?;
        } else {
            if crate::cas::link_package(&sp, &dp).is_err() {
                fs::copy(&sp, &dp).map_err(|e| e.to_string())?;
            }
            *count += 1;
        }
    }
    Ok(())
}

pub fn extract_with_sheaf(
    tarball_path: &Path,
    package_root: &Path,
    sheaf: FeatureSheaf,
) -> Result<usize, String> {
    // Prefer store_unpacked sibling if this is a store tarball path.
    if let Some(hash) = tarball_path.file_stem().and_then(|s| s.to_str()) {
        if hash.len() >= 8 {
            let cache = crate::utils::get_cache_dir();
            let shard = &hash[..2.min(hash.len())];
            let unpacked = PathBuf::from(&cache)
                .join("store_unpacked")
                .join(shard)
                .join(hash);
            if unpacked.exists() && crate::registry::store_extract_complete(&unpacked) {
                return project_from_store(&unpacked, package_root, sheaf);
            }
        }
    }

    let file = File::open(tarball_path).map_err(|e| format!("Failed to open tarball: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    fs::create_dir_all(package_root).map_err(|e| format!("Failed to create dest dir: {}", e))?;

    // Stream entries: buffer only before package.json is discovered.
    let mut pre_buffer: Vec<(String, Vec<u8>, bool)> = Vec::new();
    let mut entry_extra: HashSet<String> = HashSet::new();
    let mut seen_package_json = false;
    let mut extracted_count = 0usize;

    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read archive: {}", e))?
    {
        let mut entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to get path: {}", e))?
            .to_string_lossy()
            .to_string();
        let Some(rel_path) = normalize_tarball_rel_path(&path) else {
            continue;
        };
        let is_dir = entry.header().entry_type().is_dir();
        let mut data = Vec::new();
        if !is_dir {
            std::io::Read::read_to_end(&mut entry, &mut data)
                .map_err(|e| format!("read entry: {}", e))?;
        }
        if rel_path == "package.json" {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&data) {
                collect_entry_paths(&v, &mut entry_extra);
            }
            seen_package_json = true;
            // Flush pre-buffer now that we know entry_extra
            for (rp, d, id) in pre_buffer.drain(..) {
                if sheaf != FeatureSheaf::Full {
                    if should_skip_noise(&rp) && !should_keep_file(&rp, sheaf, &entry_extra) {
                        continue;
                    }
                    if !should_keep_file(&rp, sheaf, &entry_extra) {
                        continue;
                    }
                }
                let dest_path = package_root.join(&rp);
                if id {
                    fs::create_dir_all(&dest_path)
                        .map_err(|e| format!("Failed to create dir: {}", e))?;
                } else {
                    if let Some(parent) = dest_path.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("Failed to create parent: {}", e))?;
                    }
                    fs::write(&dest_path, &d)
                        .map_err(|e| format!("Failed to unpack file: {}", e))?;
                    extracted_count += 1;
                }
            }
            // Write package.json itself
            let dest_path = package_root.join(&rel_path);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::write(&dest_path, &data).ok();
            extracted_count += 1;
            continue;
        }
        if !seen_package_json {
            pre_buffer.push((rel_path, data, is_dir));
            continue;
        }
        // Stream process: we already know entry_extra
        if sheaf != FeatureSheaf::Full {
            if should_skip_noise(&rel_path) && !should_keep_file(&rel_path, sheaf, &entry_extra) {
                continue;
            }
            if !should_keep_file(&rel_path, sheaf, &entry_extra) {
                continue;
            }
        }
        let dest_path = package_root.join(&rel_path);
        if is_dir {
            fs::create_dir_all(&dest_path).map_err(|e| format!("Failed to create dir: {}", e))?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent: {}", e))?;
            }
            fs::write(&dest_path, &data).map_err(|e| format!("Failed to unpack file: {}", e))?;
            extracted_count += 1;
        }
    }
    // Flush any remaining pre-buffer entries if package.json was never found
    for (rel_path, data, is_dir) in pre_buffer {
        if sheaf != FeatureSheaf::Full {
            if should_skip_noise(&rel_path) && !should_keep_file(&rel_path, sheaf, &entry_extra) {
                continue;
            }
            if !should_keep_file(&rel_path, sheaf, &entry_extra) {
                continue;
            }
        }
        let dest_path = package_root.join(&rel_path);
        if is_dir {
            fs::create_dir_all(&dest_path).ok();
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::write(&dest_path, &data).ok();
            extracted_count += 1;
        }
    }
    Ok(extracted_count)
}

fn collect_entry_paths(pkg: &serde_json::Value, out: &mut HashSet<String>) {
    let push = |s: &str, out: &mut HashSet<String>| {
        let t = s.trim().trim_start_matches("./");
        if !t.is_empty() {
            out.insert(t.replace('\\', "/"));
        }
    };
    for key in ["main", "module", "browser", "types", "typings"] {
        if let Some(s) = pkg.get(key).and_then(|v| v.as_str()) {
            push(s, out);
        }
    }
    if let Some(bin) = pkg.get("bin") {
        if let Some(s) = bin.as_str() {
            push(s, out);
        } else if let Some(obj) = bin.as_object() {
            for v in obj.values() {
                if let Some(s) = v.as_str() {
                    push(s, out);
                }
            }
        }
    }
    if let Some(exports) = pkg.get("exports") {
        walk_exports(exports, out);
    }
}

fn walk_exports(v: &serde_json::Value, out: &mut HashSet<String>) {
    match v {
        serde_json::Value::String(s) => {
            let t = s.trim().trim_start_matches("./");
            if !t.is_empty() {
                out.insert(t.replace('\\', "/"));
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                walk_exports(val, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                walk_exports(val, out);
            }
        }
        _ => {}
    }
}

fn should_skip_noise(path: &str) -> bool {
    let path_lower = path.to_lowercase().replace('\\', "/");
    for pattern in SKIP_NOISE {
        if pattern.ends_with('/') {
            if path_lower.starts_with(pattern) || path_lower.contains(&format!("/{}", pattern)) {
                return true;
            }
        } else if path_lower == *pattern || path_lower.ends_with(&format!("/{}", pattern)) {
            return true;
        }
    }
    // Drop pure markdown except LICENSE* already handled in keep.
    if path_lower.ends_with(".md") || path_lower.ends_with(".markdown") {
        let base = path_lower.rsplit('/').next().unwrap_or(&path_lower);
        if !(base.starts_with("license")
            || base.starts_with("licence")
            || base.starts_with("notice")
            || base.starts_with("copying"))
        {
            return true;
        }
    }
    false
}

fn should_keep_file(path: &str, sheaf: FeatureSheaf, entry_extra: &HashSet<String>) -> bool {
    if sheaf == FeatureSheaf::Full {
        return true;
    }
    let path_norm = path.replace('\\', "/");
    let path_lower = path_norm.to_lowercase();

    // Declared entry points always.
    if entry_extra.iter().any(|e| {
        let e = e.to_lowercase();
        path_lower == e
            || path_lower.ends_with(&format!("/{}", e))
            || path_lower.starts_with(&format!("{}/", e.trim_end_matches('/')))
    }) {
        return true;
    }

    // legal
    let base = path_lower.rsplit('/').next().unwrap_or(&path_lower);
    for p in LEGAL_TYPES_CORE {
        if base == *p
            || base.starts_with("license")
            || base.starts_with("licence")
            || base.starts_with("copying")
            || base.starts_with("notice")
        {
            return true;
        }
    }

    // types stalk (always on Default)
    if sheaf == FeatureSheaf::Default {
        if path_lower.ends_with(".d.ts")
            || path_lower.ends_with(".d.mts")
            || path_lower.ends_with(".d.cts")
            || path_lower.starts_with("types/")
            || path_lower.contains("/types/")
            || path_lower.starts_with("typings/")
            || path_lower.contains("/typings/")
        {
            return true;
        }
        // Keep .ts only under types/ already covered; keep package types field files.
        if path_lower == "tsconfig.json" {
            return true;
        }
    }

    // runtime dirs
    for d in RUNTIME_DIRS {
        if path_lower.starts_with(d) || path_lower.contains(&format!("/{}", d)) {
            return true;
        }
    }

    // index + js modules
    if matches!(
        base,
        "index.js" | "index.mjs" | "index.cjs" | "index.ts" | "index.mts" | "index.cts"
    ) {
        return true;
    }
    if path_lower.ends_with(".js")
        || path_lower.ends_with(".mjs")
        || path_lower.ends_with(".cjs")
        || path_lower.ends_with(".json")
        || path_lower.ends_with(".node")
        || path_lower.ends_with(".wasm")
        || path_lower.ends_with(".css")
        || path_lower.ends_with(".map")
    {
        return true;
    }

    // bin scripts without extension
    if path_lower.starts_with("bin/") {
        return true;
    }

    false
}

fn normalize_tarball_rel_path(path: &str) -> Option<String> {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let first = parts.next()?;
    if first != "package" {
        // Some tarballs use package-name/ prefix
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            return None;
        }
        return Some(rest.join("/"));
    }
    let rel = parts.collect::<Vec<_>>().join("/");
    if rel.is_empty() {
        None
    } else {
        Some(rel)
    }
}

/// Get estimated size savings from selective extraction
pub fn estimate_savings(tarball_path: &Path) -> Result<(u64, u64, f64), String> {
    let file = File::open(tarball_path).map_err(|e| format!("Failed to open tarball: {}", e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut total_size = 0u64;
    let mut essential_size = 0u64;
    let empty = HashSet::new();
    for entry in archive
        .entries()
        .map_err(|e| format!("Failed to read archive: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let size = entry
            .header()
            .size()
            .map_err(|e| format!("Failed to get size: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to get path: {}", e))?
            .to_string_lossy()
            .to_string();
        total_size += size;
        if let Some(rel_path) = normalize_tarball_rel_path(&path) {
            if should_keep_file(&rel_path, FeatureSheaf::Default, &empty) {
                essential_size += size;
            }
        }
    }
    let savings = if total_size > 0 {
        ((total_size.saturating_sub(essential_size)) as f64 / total_size as f64) * 100.0
    } else {
        0.0
    };
    Ok((total_size, essential_size, savings))
}

/// Extract tarball from a reader (streaming — no temp file needed).
pub fn extract_tarball_from_reader<R: std::io::Read>(
    reader: R,
    dest_dir: &Path,
) -> Result<(usize, String), String> {
    let mut hasher = Sha256::new();
    let mut tee = TeeReader::new(reader, &mut hasher);
    let decoder = GzDecoder::new(&mut tee);
    let mut archive = Archive::new(decoder);
    fs::create_dir_all(dest_dir).map_err(|e| format!("Failed to create dest dir: {}", e))?;
    let mut count = 0usize;
    for entry in archive.entries().map_err(|e| format!("Tar error: {}", e))? {
        let mut entry = entry.map_err(|e| format!("Tar entry error: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("Tar path error: {}", e))?
            .to_path_buf();
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let dest = dest_dir.join(&path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        }
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&dest).ok();
        } else {
            let mut out = fs::File::create(&dest).map_err(|e| format!("create: {}", e))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("write: {}", e))?;
        }
        count += 1;
    }
    let hash = format!("{:x}", hasher.finalize());
    Ok((count, hash))
}

struct TeeReader<'a, R: std::io::Read> {
    inner: R,
    hasher: &'a mut Sha256,
}

impl<'a, R: std::io::Read> TeeReader<'a, R> {
    fn new(inner: R, hasher: &'a mut Sha256) -> Self {
        Self { inner, hasher }
    }
}

impl<'a, R: std::io::Read> std::io::Read for TeeReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keep_license_and_dts() {
        let empty = HashSet::new();
        assert!(should_keep_file("LICENSE", FeatureSheaf::Default, &empty));
        assert!(should_keep_file(
            "index.d.ts",
            FeatureSheaf::Default,
            &empty
        ));
        assert!(should_keep_file(
            "dist/index.js",
            FeatureSheaf::Default,
            &empty
        ));
        assert!(
            !should_keep_file("test/foo.js", FeatureSheaf::Default, &empty)
                || should_skip_noise("test/foo.js")
        );
        assert!(should_skip_noise("test/foo.js"));
        assert!(should_keep_file("README.md", FeatureSheaf::Full, &empty));
    }

    #[test]
    fn test_entry_extra() {
        let mut extra = HashSet::new();
        extra.insert("build/main.js".into());
        assert!(should_keep_file(
            "build/main.js",
            FeatureSheaf::Minimal,
            &extra
        ));
    }

    #[test]
    fn test_normalize() {
        assert_eq!(
            normalize_tarball_rel_path("package/package.json"),
            Some("package.json".into())
        );
    }
}
