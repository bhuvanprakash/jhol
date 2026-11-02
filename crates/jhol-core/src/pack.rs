//! `jhol pack` — build a publishable tarball from a package directory.
//!
//! Deterministic by construction: entries sorted, mtimes fixed to the epoch,
//! gzip written without an OS/timestamp field. Packing the same directory
//! twice yields byte-identical output — verifiable, cacheable, and a
//! differentiator (pack twice with npm and compare the hashes).
//!
//! Inclusion rules (own implementation, no npm-packlist dependency):
//! always `package.json` + `README*` + `LICENSE*` + `CHANGELOG*`;
//! `files` list when present, otherwise everything except the default
//! excludes (node_modules, .git, VCS and editor droppings, .jhol state).

use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::{Path, PathBuf};

const ALWAYS_EXCLUDE: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    ".jhol",
    ".DS_Store",
    ".npmrc",
    ".npmignore",
];

const ALWAYS_INCLUDE: &[&str] = &["package.json", "README", "LICENSE", "LICENCE", "CHANGELOG"];

fn crc32(data: &[u8]) -> u32 {
    // Standard CRC-32 (IEEE 802.3), table-free bitwise implementation —
    // clarity over speed; the input is already in memory.
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn is_excluded(rel: &str) -> bool {
    let p = Path::new(rel);
    for comp in p.components() {
        let c = comp.as_os_str().to_string_lossy();
        if ALWAYS_EXCLUDE.iter().any(|e| c == *e) || c.starts_with('.') && c.len() > 1 && c.ends_with("~") {
            return true;
        }
    }
    false
}

fn is_always_included(rel: &str) -> bool {
    Path::new(rel)
        .file_name()
        .map(|f| {
            let f = f.to_string_lossy();
            ALWAYS_INCLUDE
                .iter()
                .any(|a| f == *a || f.starts_with(&format!("{}.", a)))
        })
        .unwrap_or(false)
}

/// Collect the files to pack, as relative paths (sorted).
pub fn collect_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let pj_path = dir.join("package.json");
    let pj: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&pj_path)
            .map_err(|e| format!("read package.json: {}", e))?,
    )
    .map_err(|e| format!("parse package.json: {}", e))?;

    let files_field: Vec<String> = pj
        .get("files")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut out: Vec<PathBuf> = Vec::new();
    let use_files = !files_field.is_empty();

    fn walk(
        dir: &Path,
        rel: &str,
        out: &mut Vec<PathBuf>,
        use_files: bool,
        files_field: &[String],
    ) -> Result<(), String> {
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("read {}: {}", dir.display(), e))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", rel, name)
            };
            if is_excluded(&child_rel) {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                if use_files && !files_field.iter().any(|f| child_rel == *f || f.starts_with(&format!("{}/", child_rel))) {
                    // directory not listed in files — still walk its top level
                    // because a nested path may be listed exactly
                    walk(&path, &child_rel, out, use_files, files_field)?;
                    continue;
                }
                walk(&path, &child_rel, out, use_files, files_field)?;
            } else {
                let included = !use_files
                    || files_field.iter().any(|f| {
                        child_rel == *f
                            || f.starts_with(&format!("{}/", child_rel))
                            || child_rel.starts_with(&format!("{}/", f.trim_end_matches('/')))
                    })
                    || is_always_included(&child_rel);
                if included {
                    out.push(PathBuf::from(child_rel));
                }
            }
        }
        Ok(())
    }

    walk(dir, "", &mut out, use_files, &files_field)?;
    out.sort();
    out.dedup();
    Ok(out)
}

/// Build the deterministic tarball bytes for `dir`.
pub fn pack_bytes(dir: &Path) -> Result<(Vec<u8>, String, usize), String> {
    let pj: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("package.json"))
            .map_err(|e| format!("read package.json: {}", e))?,
    )
    .map_err(|e| format!("parse package.json: {}", e))?;
    let name = pj
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or("package.json missing name")?
        .trim_start_matches('@')
        .replace('/', "-");
    let version = pj
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or("package.json missing version")?
        .to_string();

    let files = collect_files(dir)?;
    let mut tar = tar::Builder::new(Vec::new());
    for rel in &files {
        let abs = dir.join(rel);
        let meta = abs.metadata().map_err(|e| format!("{}: {}", rel.display(), e))?;
        if meta.is_dir() {
            let mut h = tar::Header::new_gnu();
            h.set_size(0);
            h.set_mode(0o755);
            h.set_entry_type(tar::EntryType::Directory);
            h.set_mtime(0);
            h.set_cksum();
            tar.append_data(&mut h, &format!("package/{}", rel.display()), std::io::empty())
                .map_err(|e| e.to_string())?;
        } else {
            let data = std::fs::read(&abs).map_err(|e| format!("{}: {}", rel.display(), e))?;
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(0);
            h.set_cksum();
            tar.append_data(&mut h, &format!("package/{}", rel.display()), data.as_slice())
                .map_err(|e| e.to_string())?;
        }
    }
    let tar_bytes = tar.into_inner().map_err(|e| e.to_string())?;

    // Deterministic gzip via a raw deflate stream wrapped in a hand-built
    // header/trailer with mtime=0 and OS=unknown. flate2's GzEncoder emits a
    // header whose mtime is zero already, but building the container by hand
    // makes the byte layout fully explicit (and testable).
    let mut defl = flate2::write::DeflateEncoder::new(Vec::new(), Compression::default());
    defl.write_all(&tar_bytes).map_err(|e| e.to_string())?;
    let deflate_body = defl.finish().map_err(|e| e.to_string())?;

    let mut h = Vec::with_capacity(10);
    h.push(0x1f);
    h.push(0x8b);
    h.push(8); // deflate
    h.push(0); // no flags (no name/comment/mtime extras)
    h.extend_from_slice(&[0, 0, 0, 0]); // mtime = 0
    h.push(0); // XFL
    h.push(255); // OS = unknown
    let crc = crc32(&tar_bytes);
    let len = tar_bytes.len() as u32;
    let gz = h
        .into_iter()
        .chain(deflate_body)
        .chain(crc.to_le_bytes())
        .chain(len.to_le_bytes())
        .collect::<Vec<u8>>();

    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(&gz);
    let digest = h.finalize();
    let sha = digest.iter().map(|b| format!("{:02x}", b)).collect::<String>();
    let filename = format!("{}-{}.tgz", name, version);
    Ok((gz, format!("{}:{}", filename, sha), files.len()))
}
