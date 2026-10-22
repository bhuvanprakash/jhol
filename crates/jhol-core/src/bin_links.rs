use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::utils;

#[derive(Clone, Debug, Default)]
pub struct BinLinkReport {
    pub packages_scanned: usize,
    pub links_created: usize,
    pub links_skipped: usize,
}

pub fn rebuild_bin_links(node_modules: &Path) -> Result<BinLinkReport, String> {
    let mut report = BinLinkReport::default();
    if !node_modules.is_dir() {
        return Ok(report);
    }

    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    // Collect the installed package names first (a cheap readdir), then scan them
    // in parallel.
    //
    // Measured: this loop was the entire warm-install tail. It ran *serially* over
    // every installed package, and each iteration calls `link_bins_for_package`,
    // which reads that package's `package.json` in full to test for a `"bin"` key.
    // For 28 packages on a cold page cache that is ~100 ms of scattered reads -
    // and it duplicated work `project_package` had already done per package inside
    // the capsule waveform install. The reads are independent, so fanning them out
    // costs about one read latency instead of 28.
    let mut names: Vec<String> = Vec::new();
    let entries = fs::read_dir(node_modules).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        if name.starts_with('.') || name == ".bin" {
            continue;
        }

        if name.starts_with('@') && path.is_dir() {
            for scoped in fs::read_dir(&path).map_err(|e| e.to_string())?.flatten() {
                if scoped.path().is_dir() {
                    names.push(format!("{}/{}", name, scoped.file_name().to_string_lossy()));
                }
            }
            continue;
        }

        if path.is_dir() {
            names.push(name);
        }
    }

    {
        use rayon::prelude::*;
        let (scanned, created, skipped) = names
            .par_iter()
            .fold(
                || (0usize, 0usize, 0usize),
                |acc, pkg_name| {
                    let ok = link_bins_for_package(node_modules, pkg_name).is_ok();
                    (acc.0 + 1, acc.1 + usize::from(ok), acc.2 + usize::from(!ok))
                },
            )
            .reduce(|| (0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
        report.packages_scanned = scanned;
        report.links_created = created;
        report.links_skipped = skipped;
    }

    // Drop shims whose package is no longer installed.
    prune_dangling_bin_shims(&bin_dir);
    Ok(report)
}

/// Link bin shims for a known slice of package names directly, bypassing node_modules readdir.
pub fn link_bins_for_packages(
    node_modules: &Path,
    names: &[&str],
) -> Result<BinLinkReport, String> {
    let mut report = BinLinkReport::default();
    if !node_modules.is_dir() || names.is_empty() {
        return Ok(report);
    }

    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    {
        use rayon::prelude::*;
        let (scanned, created, skipped) = names
            .par_iter()
            .fold(
                || (0usize, 0usize, 0usize),
                |acc, &pkg_name| {
                    let ok = link_bins_for_package(node_modules, pkg_name).is_ok();
                    (acc.0 + 1, acc.1 + usize::from(ok), acc.2 + usize::from(!ok))
                },
            )
            .reduce(|| (0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
        report.packages_scanned = scanned;
        report.links_created = created;
        report.links_skipped = skipped;
    }

    prune_dangling_bin_shims(&bin_dir);
    Ok(report)
}

/// npm sets the executable bit on every declared `bin` target after unpacking, and
/// tarballs are not required to store it: `typescript` ships `bin/tsc` as mode 644.
/// jhol extracted tar's stored mode, so `node_modules/.bin/tsc` — a symlink to that
/// file — failed with `Permission denied` while npm's identical install ran
/// `Version 5.9.3`. Executing a script needs +x on the real target, so fix the target
/// rather than the link. 0o755 matches npm's chmod.
fn ensure_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o111 != 0o111 {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o755));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Remove `.bin` shims whose target no longer exists.
///
/// `jhol remove <pkg>` deletes the package directory but used to leave its shim
/// behind, so `node_modules/.bin/tsc` remained and running it failed with
/// `cannot execute: No such file or directory` — while `npm uninstall` prunes the
/// entry (verified against npm as a control). Only shims jhol itself wrote (a
/// `/bin/sh` wrapper referencing `$_dir/`) are removed, so a user-added script in
/// `.bin` survives.
fn prune_dangling_bin_shims(bin_dir: &Path) {
    let Ok(entries) = fs::read_dir(bin_dir) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if !content.starts_with("#!/bin/sh") || !content.contains("exec ") {
            continue;
        }
        let Some(rest) = content.split("$_dir/").nth(1) else {
            continue;
        };
        let Some(rel) = rest.split('"').next() else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        if !bin_dir.join(rel).exists() {
            let _ = fs::remove_file(&path);
        }
    }
}

pub fn link_bins_for_package(node_modules: &Path, package_name: &str) -> Result<(), String> {
    let package_name = utils::sanitize_package_name(package_name)
        .ok_or_else(|| format!("Invalid package name: {}", package_name))?;
    let pkg_dir = node_modules.join(package_name.clone());
    if !pkg_dir.is_dir() {
        return Ok(());
    }

    let package_json_path = pkg_dir.join("package.json");
    if !package_json_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&package_json_path).map_err(|e| e.to_string())?;
    if !content.contains("\"bin\"") {
        return Ok(());
    }
    let parsed: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let bins = parse_bin_entries(&parsed)?;
    if bins.is_empty() {
        return Ok(());
    }

    let bin_dir = node_modules.join(".bin");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;

    for (bin_name, rel_target) in bins {
        let target = pkg_dir.join(&rel_target);
        if !target.exists() {
            continue;
        }
        // The target must be runnable, not merely present: `bin` scripts are
        // invoked through `.bin`, and a 644 target fails with "Permission denied".
        ensure_executable(&target);
        create_bin_link(&bin_dir, &bin_name, &package_name, &rel_target)?;
    }

    Ok(())
}

fn parse_bin_entries(pkg: &serde_json::Value) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    let package_name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("");

    let Some(bin) = pkg.get("bin") else {
        return Ok(out);
    };

    if let Some(bin_path) = bin.as_str() {
        let name = package_name
            .rsplit('/')
            .next()
            .unwrap_or(package_name)
            .to_string();
        if !name.is_empty() {
            out.insert(name, bin_path.to_string());
        }
        return Ok(out);
    }

    if let Some(obj) = bin.as_object() {
        for (k, v) in obj {
            if let Some(bin_path) = v.as_str() {
                out.insert(k.clone(), bin_path.to_string());
            }
        }
        return Ok(out);
    }

    Err("invalid package.json bin field".to_string())
}

fn is_node_target(target: &Path) -> bool {
    matches!(
        target.extension().and_then(|e| e.to_str()),
        Some("js") | Some("cjs") | Some("mjs")
    )
}

fn relative_target_from_bin(package_name: &str, rel_target: &str) -> PathBuf {
    let mut rel = PathBuf::from("..");
    for seg in package_name.split('/') {
        rel.push(seg);
    }
    for seg in rel_target.split('/') {
        rel.push(seg);
    }
    rel
}

#[cfg(unix)]
fn create_bin_link(
    bin_dir: &Path,
    bin_name: &str,
    package_name: &str,
    rel_target: &str,
) -> Result<(), String> {
    let link_path = bin_dir.join(bin_name);
    if link_path.exists() {
        fs::remove_file(&link_path).ok();
    }

    let rel = relative_target_from_bin(package_name, rel_target);
    let rel_unix = rel.to_string_lossy().replace('\\', "/");
    let use_node = is_node_target(Path::new(rel_target));

    let script = if use_node {
        format!("#!/bin/sh\nexec node \"$_dir/{rel_unix}\" \"$@\"\n")
    } else {
        format!("#!/bin/sh\nexec \"$_dir/{rel_unix}\" \"$@\"\n")
    };
    let wrapper =
        format!("#!/bin/sh\n_dir=\"$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\"\n{script}");
    fs::write(&link_path, wrapper).map_err(|e| e.to_string())?;

    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&link_path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&link_path, perms).map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(windows)]
fn create_bin_link(
    bin_dir: &Path,
    bin_name: &str,
    package_name: &str,
    rel_target: &str,
) -> Result<(), String> {
    let cmd_path = bin_dir.join(format!("{}.cmd", bin_name));
    let ps1_path = bin_dir.join(format!("{}.ps1", bin_name));

    let rel = relative_target_from_bin(package_name, rel_target);
    let rel_win = rel.to_string_lossy().replace('/', "\\");
    let use_node = is_node_target(Path::new(rel_target));

    let cmd = if use_node {
        format!("@echo off\r\nset \"_prog=%~dp0\\{rel_win}\"\r\nnode \"%_prog%\" %*\r\n")
    } else {
        format!("@echo off\r\nset \"_prog=%~dp0\\{rel_win}\"\r\n\"%_prog%\" %*\r\n")
    };
    fs::write(&cmd_path, cmd).map_err(|e| e.to_string())?;

    let ps1 = if use_node {
        format!("$prog = Join-Path $PSScriptRoot '{rel_win}'\nnode $prog $args\n")
    } else {
        format!("$prog = Join-Path $PSScriptRoot '{rel_win}'\n& $prog $args\n")
    };
    fs::write(&ps1_path, ps1).map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bin_string_field() {
        let v: serde_json::Value = serde_json::json!({
            "name": "foo",
            "bin": "bin/cli.js"
        });
        let parsed = parse_bin_entries(&v).expect("parse");
        assert_eq!(parsed.get("foo"), Some(&"bin/cli.js".to_string()));
    }

    #[test]
    fn parse_bin_object_field() {
        let v: serde_json::Value = serde_json::json!({
            "name": "@scope/foo",
            "bin": {
                "foo": "dist/foo.js",
                "bar": "dist/bar.js"
            }
        });
        let parsed = parse_bin_entries(&v).expect("parse");
        assert_eq!(parsed.get("foo"), Some(&"dist/foo.js".to_string()));
        assert_eq!(parsed.get("bar"), Some(&"dist/bar.js".to_string()));
    }

    #[test]
    fn scoped_relative_target_is_correct() {
        let rel = relative_target_from_bin("@scope/foo", "dist/cli.js");
        assert_eq!(
            rel.to_string_lossy().replace('\\', "/"),
            "../@scope/foo/dist/cli.js"
        );
    }

    /// A `bin` target that the tarball stored as 644 must still be runnable:
    /// `typescript` publishes `bin/tsc` that way, and `.bin/tsc` is a symlink to
    /// it, so the executable bit belongs on the target.
    #[test]
    #[cfg(unix)]
    fn link_bins_makes_target_executable() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::tempdir().expect("tmp");
        let node_modules = td.path().join("node_modules");
        let pkg_dir = node_modules.join("demo-cli");
        std::fs::create_dir_all(pkg_dir.join("bin")).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"demo-cli","version":"1.0.0","bin":{"demo":"bin/cli.js"}}"#,
        )
        .unwrap();
        let script = pkg_dir.join("bin/cli.js");
        std::fs::write(&script, "#!/usr/bin/env node\nconsole.log('ok');\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            std::fs::metadata(&script).unwrap().permissions().mode() & 0o111,
            0,
            "precondition: target starts non-executable"
        );

        link_bins_for_package(&node_modules, "demo-cli").expect("link bins");

        assert_ne!(
            std::fs::metadata(&script).unwrap().permissions().mode() & 0o111,
            0,
            "declared bin target must be executable, else .bin/<tool> fails"
        );
    }

    #[test]
    fn link_bins_for_package_creates_entry() {
        let td = tempfile::tempdir().expect("tmp");
        let node_modules = td.path().join("node_modules");
        let pkg_dir = node_modules.join("demo-cli");
        std::fs::create_dir_all(pkg_dir.join("bin")).expect("mkdir");
        std::fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"demo-cli","version":"1.0.0","bin":"bin/cli.js"}"#,
        )
        .expect("package json");
        std::fs::write(pkg_dir.join("bin/cli.js"), "console.log('ok')").expect("bin");

        link_bins_for_package(&node_modules, "demo-cli").expect("link");

        #[cfg(unix)]
        assert!(node_modules.join(".bin").join("demo-cli").exists());
        #[cfg(windows)]
        assert!(node_modules.join(".bin").join("demo-cli.cmd").exists());
    }
}
