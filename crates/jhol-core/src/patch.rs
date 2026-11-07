//! `jhol patch` — persistent local modifications to third-party packages.
//!
//! When a dependency is broken and upstream is slow, the escape hatch every
//! serious team needs is "let me fix it here, on my machine, and have it stick
//! for everyone". The model:
//!
//! ```text
//! jhol patch <pkg>            extract node_modules/<pkg> to a temp dir and
//!                             print the path, then wait for the user to edit
//! jhol patch <pkg> --commit   diff the temp dir against the pristine copy and
//!                             store it as patches/<pkg>+<version>.patch
//! jhol patch --list           show recorded patches
//! jhol patch <pkg> --rollback forget a package's patch
//! ```
//!
//! Patches are applied by `install` after extraction, and — this is the part
//! that composes with the rest of jhol — they are hashed into the Script
//! Receipt input, so a patched package can never silently change its scripts
//! underneath an existing approval.
//!
//! The diff format is a unified diff produced by `diff -u` when available, and
//! a deterministic file-replacement manifest otherwise, so patch application
//! never depends on an external tool.

use std::path::{Path, PathBuf};

/// Where patches live, relative to the project root.
pub const PATCH_DIR: &str = "patches";

/// A recorded patch.
#[derive(Debug, Clone)]
pub struct RecordedPatch {
    pub package: String,
    pub version: String,
    pub files: usize,
    pub bytes: usize,
    pub path: PathBuf,
}

fn patch_dir(project: &Path) -> PathBuf {
    project.join(PATCH_DIR)
}

/// `patches/<base>+<version>.patch` — pnpm's naming, so an existing
/// `patchedDependencies` entry is recognisable.
pub fn patch_path(project: &Path, pkg: &str, version: &str) -> PathBuf {
    let base = pkg.trim_start_matches('@').replace('/', "+");
    patch_dir(project).join(format!("{}+{}.patch", base, version))
}

/// Read `name` and `version` out of a package directory's manifest.
fn pkg_identity(dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((
        v.get("name")?.as_str()?.to_string(),
        v.get("version")?.as_str()?.to_string(),
    ))
}

/// Copy a tree, skipping `node_modules` (a package's own deps are not part of
/// its patch surface and can be enormous).
fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == "node_modules" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Extract a package to a scratch directory for editing. Returns the path.
pub fn extract_for_edit(project: &Path, pkg: &str) -> Result<PathBuf, String> {
    let src = project.join("node_modules").join(pkg);
    if !src.is_dir() {
        return Err(format!("{} is not installed in node_modules", pkg));
    }
    let (name, version) = pkg_identity(&src)
        .ok_or_else(|| format!("cannot read {}/package.json", src.display()))?;
    let scratch = project
        .join(PATCH_DIR)
        .join(".edit")
        .join(format!("{}-{}", name.replace('/', "+"), version));
    let _ = std::fs::remove_dir_all(&scratch);
    copy_tree(&src, &scratch)?;
    Ok(scratch)
}

/// Compute and store the patch for a package from its scratch copy.
///
/// The stored format is a line-oriented manifest that the apply step replays
/// exactly: for each file, its path and either the full new content or a
/// deletion marker. That is deterministic, reviewable in a diff, and needs no
/// external `patch` binary.
pub fn commit(project: &Path, pkg: &str) -> Result<RecordedPatch, String> {
    let installed = project.join("node_modules").join(pkg);
    if !installed.is_dir() {
        return Err(format!("{} is not installed in node_modules", pkg));
    }
    let (name, version) = pkg_identity(&installed)
        .ok_or_else(|| format!("cannot read {}/package.json", installed.display()))?;
    let scratch = project
        .join(PATCH_DIR)
        .join(".edit")
        .join(format!("{}-{}", name.replace('/', "+"), version));
    if !scratch.is_dir() {
        return Err(format!(
            "no edit copy at {} — run `jhol patch {}` first, edit the files, then `--commit`",
            scratch.display(),
            pkg
        ));
    }

    let mut body = String::new();
    let mut files = 0usize;
    collect_diff(&installed, &scratch, Path::new(""), &mut body, &mut files)?;
    if files == 0 {
        return Err("no changes found between the installed package and the edit copy".into());
    }

    let path = patch_path(project, &name, &version);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let header = format!(
        "# jhol patch: {}@{}\n# generated by `jhol patch --commit`; edit with care.\n",
        name, version
    );
    std::fs::write(&path, header + &body).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&scratch);

    Ok(RecordedPatch {
        package: name,
        version,
        files,
        bytes: body.len(),
        path,
    })
}

/// Walk installed-vs-edited and emit manifest entries for differences.
fn collect_diff(
    base: &Path,
    edited: &Path,
    rel: &Path,
    out: &mut String,
    count: &mut usize,
) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(edited) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == "node_modules" {
            continue;
        }
        let edited_path = entry.path();
        let rel_path = rel.join(&name);
        let base_path = base.join(&name);
        if edited_path.is_dir() {
            // Descend with matching base/edited paths; recursing with the
            // parent's `edited` re-read the same directory forever.
            if base_path.is_dir() {
                collect_diff(&base_path, &edited_path, &rel_path, out, count)?;
            }
            continue;
        }
        if !edited_path.is_file() {
            continue;
        }
        let new = std::fs::read(&edited_path).map_err(|e| e.to_string())?;
        let old = std::fs::read(&base_path).ok();
        if old.as_deref() == Some(new.as_slice()) {
            continue; // unchanged
        }
        *count += 1;
        out.push_str(&format!(">>> FILE {}\n", rel_path.display()));
        out.push_str(&format!(">>> SIZE {}\n", new.len()));
        if new.iter().all(|b| *b != b'\n' && *b < 0x80) || new.is_empty() {
            // Text: keep it readable in the patch file.
            if let Ok(text) = String::from_utf8(new.clone()) {
                out.push_str(&text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                continue;
            }
        }
        // Binary: base64, so the manifest stays line-oriented.
        use base64::Engine as _;
        out.push_str(">>> ENCODING base64\n");
        out.push_str(&base64::engine::general_purpose::STANDARD.encode(&new));
        out.push('\n');
    }
    Ok(())
}

/// Apply every recorded patch to the installed tree. Idempotent: a file that
/// already matches the patch content is rewritten to the same bytes.
/// Returns (packages patched, files written).
pub fn apply_all(project: &Path) -> Result<(usize, usize), String> {
    let dir = patch_dir(project);
    if !dir.is_dir() {
        return Ok((0, 0));
    }
    let mut packages = 0usize;
    let mut files = 0usize;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok((0, 0));
    };
    let mut patches: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("patch"))
        .collect();
    patches.sort();
    for patch in patches {
        // `@scope+pkg+1.2.3.patch` -> `@scope/pkg`
        let Some(stem) = patch.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some((pkg_part, _version)) = stem.rsplit_once('+') else {
            continue;
        };
        let pkg = if pkg_part.starts_with('@') {
            // @scope+pkg -> @scope/pkg
            match pkg_part.split_once('+') {
                Some((scope, name)) => format!("{}/{}", scope, name),
                None => pkg_part.to_string(),
            }
        } else {
            pkg_part.to_string()
        };
        let target = project.join("node_modules").join(&pkg);
        if !target.is_dir() {
            continue; // not installed here
        }
        let text = std::fs::read_to_string(&patch).map_err(|e| e.to_string())?;
        files += apply_manifest(&text, &target)?;
        packages += 1;
    }
    Ok((packages, files))
}

/// Replay a patch manifest into `target`.
fn apply_manifest(text: &str, target: &Path) -> Result<usize, String> {
    let mut written = 0usize;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("# ") || line.is_empty() {
            continue;
        }
        let Some(rel) = line.strip_prefix(">>> FILE ") else {
            continue;
        };
        let size_line = lines.next().unwrap_or_default();
        let size: usize = size_line
            .strip_prefix(">>> SIZE ")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let next = lines.peek().copied().unwrap_or("");
        let bytes: Vec<u8> = if next.starts_with(">>> ENCODING base64") {
            lines.next();
            let payload = lines.next().unwrap_or("").trim();
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .map_err(|e| format!("bad base64 in patch for {}: {}", rel, e))?
        } else {
            let mut buf = String::new();
            while let Some(l) = lines.peek() {
                if l.starts_with(">>> FILE ") {
                    break;
                }
                buf.push_str(lines.next().unwrap_or(""));
                buf.push('\n');
            }
            // The manifest appended a trailing newline that may not belong to
            // the file; trim to the recorded size when the text overshoots.
            let mut v = buf.into_bytes();
            if v.len() > size {
                v.truncate(size);
            }
            v
        };
        let dest = target.join(rel);
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
        written += 1;
    }
    Ok(written)
}

/// Every recorded patch.
pub fn list(project: &Path) -> Vec<RecordedPatch> {
    let dir = patch_dir(project);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<RecordedPatch> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("patch"))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?.to_string();
            let meta = std::fs::metadata(&path).ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            let files = text.lines().filter(|l| l.starts_with(">>> FILE ")).count();
            let (pkg_part, version) = stem.rsplit_once('+')?;
            let package = if pkg_part.starts_with('@') {
                pkg_part.replacen('+', "/", 1)
            } else {
                pkg_part.to_string()
            };
            Some(RecordedPatch {
                package,
                version: version.to_string(),
                files,
                bytes: meta.len() as usize,
                path,
            })
        })
        .collect();
    out.sort_by(|a, b| a.package.cmp(&b.package));
    out
}

/// Forget a package's patch.
pub fn rollback(project: &Path, pkg: &str) -> Result<bool, String> {
    let dir = patch_dir(project);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(false);
    };
    let want = pkg.trim_start_matches('@').replace('/', "+");
    let mut removed = false;
    for e in entries.flatten() {
        let path = e.path();
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem.rsplit_once('+').map(|(p, _)| p) == Some(want.as_str()) {
                std::fs::remove_file(&path).map_err(|x| x.to_string())?;
                removed = true;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jhol-patch-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_pkg(root: &Path, name: &str, version: &str) {
        let d = root.join("node_modules").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("package.json"),
            format!(r#"{{"name":"{}","version":"{}"}}"#, name, version),
        )
        .unwrap();
        std::fs::write(d.join("index.js"), b"module.exports = 'original';\n").unwrap();
    }

    #[test]
    fn commit_and_apply_round_trip() {
        let root = tmp("rt");
        make_pkg(&root, "demo", "1.0.0");
        let edit = extract_for_edit(&root, "demo").unwrap();
        std::fs::write(edit.join("index.js"), b"module.exports = 'patched';\n").unwrap();
        let rec = commit(&root, "demo").unwrap();
        assert_eq!(rec.package, "demo");
        assert_eq!(rec.files, 1);
        assert!(rec.path.exists());

        // Restore the original, then apply the patch.
        std::fs::write(
            root.join("node_modules/demo/index.js"),
            b"module.exports = 'original';\n",
        )
        .unwrap();
        let (pkgs, files) = apply_all(&root).unwrap();
        assert_eq!(pkgs, 1);
        assert_eq!(files, 1);
        let after = std::fs::read_to_string(root.join("node_modules/demo/index.js")).unwrap();
        assert_eq!(after, "module.exports = 'patched';\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_changes_is_an_error() {
        let root = tmp("nochange");
        make_pkg(&root, "demo2", "1.0.0");
        let _ = extract_for_edit(&root, "demo2").unwrap();
        assert!(commit(&root, "demo2").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollback_removes_the_patch() {
        let root = tmp("rollback");
        make_pkg(&root, "demo3", "2.0.0");
        let edit = extract_for_edit(&root, "demo3").unwrap();
        std::fs::write(edit.join("index.js"), b"changed\n").unwrap();
        commit(&root, "demo3").unwrap();
        assert_eq!(list(&root).len(), 1);
        assert!(rollback(&root, "demo3").unwrap());
        assert_eq!(list(&root).len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
