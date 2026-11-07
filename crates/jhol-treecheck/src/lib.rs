//! Does the installed tree say what the lockfile says?
//!
//! Every fast path in the installer used to answer "is this tree current?" from
//! a *fingerprint of files* — package.json mtime, lockfile mtime, a status
//! file. A fingerprint can prove "nothing changed around the tree". It cannot
//! prove "the tree is what the lockfile authorises", and three things follow
//! from that gap, all reproducible on a plain `express@^4` project:
//!
//!   * a package edited on disk to version 9.9.9 survives `jhol install`
//!     ("Up to date") — a tampered tree is accepted;
//!   * a directory dropped into node_modules that the lockfile never
//!     authorises is kept forever — nothing prunes;
//!   * a deleted package *is* repaired, so the failure is inconsistent:
//!     absence is noticed, presence is not.
//!
//! This module is the single predicate the lanes should share: for every
//! package the lockfile authorises, is it on disk at the locked version, and is
//! everything on disk authorised? It reads one small `package.json` per
//! installed package and no archives — the goal is falsifiability of the fast
//! paths, not content hashing. (SRI verification lives in `integrity.rs` and is
//! the stronger, opt-in form.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Upper bound on directories inspected before we admit we do not know.
const MAX_INSPECT: usize = 40_000;

/// What the tree turned out to be, relative to the lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeState {
    /// Every authorised package is present at its locked version, and nothing
    /// unauthorised is installed.
    Match,
    /// A package the lockfile authorises is absent.
    Missing { path: String },
    /// A package is installed at a version the lockfile does not authorise.
    Wrong { path: String, want: String, got: String },
    /// A package is installed that the lockfile does not authorise.
    Extraneous { path: String },
    /// No readable lockfile, or the tree is too large to inspect cheaply.
    Unverified { reason: String },
}

impl TreeState {
    /// Is the tree safe to skip work for? Only `Match` is.
    pub fn is_match(&self) -> bool {
        matches!(self, TreeState::Match)
    }

    /// One-line explanation, suitable for logs and for telling the user why the
    /// fast path was skipped.
    pub fn describe(&self) -> String {
        match self {
            TreeState::Match => "tree matches lockfile".to_string(),
            TreeState::Missing { path } => format!("{} is missing", path),
            TreeState::Wrong { path, want, got } => {
                format!("{} is {} but the lockfile pins {}", path, got, want)
            }
            TreeState::Extraneous { path } => format!("{} is not in the lockfile", path),
            TreeState::Unverified { reason } => format!("tree not verified: {}", reason),
        }
    }

    /// Repairs needed to converge. `true` means an install must run rather than
    /// being skipped — which is the only thing the lanes need to know.
    pub fn needs_install(&self) -> bool {
        !matches!(self, TreeState::Match | TreeState::Unverified { .. })
    }
}

/// The lockfile's view: tree path -> locked version.
///
/// Keys are `package-lock.json` `packages` keys (`node_modules/a`,
/// `node_modules/a/node_modules/b`). The root entry (`""`) carries no version
/// and is skipped.
fn locked_paths(project_dir: &Path) -> Option<HashMap<String, String>> {
    let path = ["package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| project_dir.join(n))
        .find(|p| p.exists())?;
    let text = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut out = HashMap::with_capacity(packages.len());
    for (key, entry) in packages {
        if key.is_empty() {
            continue;
        }
        let Some(version) = entry.get("version").and_then(|x| x.as_str()) else {
            // A key without a version is a link/workspace entry, not an
            // installed package.
            continue;
        };
        if entry.get("link").and_then(|x| x.as_bool()) == Some(true) {
            continue;
        }
        // Only real install locations are comparable to disk.
        if !key.contains("node_modules/") {
            continue;
        }
        out.insert(key.clone(), version.to_string());
    }
    Some(out)
}

/// Version recorded in an installed package.json, without parsing the whole
/// document into a struct (these are small, but there are many).
fn disk_version(package_json: &Path) -> Option<String> {
    let text = std::fs::read_to_string(package_json).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

/// Every install location under `node_modules`, as lockfile-style paths.
///
/// The queue holds *node_modules directories*, not package directories: a
/// package's own `lib/` is not an install location, only its nested
/// `node_modules` is. Getting that wrong reports `node_modules/a/lib` as an
/// unauthorised package and prunes real files.
fn installed_paths(project_dir: &Path) -> Option<Vec<String>> {
    let root = project_dir.join("node_modules");
    if !root.is_dir() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut queue: Vec<(PathBuf, String)> = vec![(root, "node_modules".to_string())];
    while let Some((nm_dir, prefix)) = queue.pop() {
        let entries = match std::fs::read_dir(&nm_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == ".bin" || name == ".cache" {
                continue;
            }
            let path = entry.path();
            if name.starts_with('@') {
                // Scoped: the child is the package, not the scope directory.
                if let Ok(children) = std::fs::read_dir(&path) {
                    let mut kids: Vec<_> = children.flatten().collect();
                    kids.sort_by_key(|c| c.file_name());
                    for child in kids {
                        let cname = child.file_name().to_string_lossy().to_string();
                        if cname.starts_with('.') || !child.path().is_dir() {
                            continue;
                        }
                        let cpath = child.path();
                        let rel = format!("{}/{}/{}", prefix, name, cname);
                        out.push(rel.clone());
                        if out.len() > MAX_INSPECT {
                            return None;
                        }
                        let nested = cpath.join("node_modules");
                        if nested.is_dir() {
                            queue.push((nested, format!("{}/node_modules", rel)));
                        }
                    }
                }
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let rel = format!("{}/{}", prefix, name);
            out.push(rel.clone());
            if out.len() > MAX_INSPECT {
                return None;
            }
            let nested = path.join("node_modules");
            if nested.is_dir() {
                queue.push((nested, format!("{}/node_modules", rel)));
            }
        }
    }
    Some(out)
}

/// Compare the installed tree against the lockfile.
///
/// Cheap by design: one `package.json` read per installed package. Returns the
/// *first* discrepancy in a stable order (missing, then wrong, then extraneous)
/// so callers can report something specific and tests can pin it.
pub fn verify_tree(project_dir: &Path) -> TreeState {
    let Some(locked) = locked_paths(project_dir) else {
        return TreeState::Unverified {
            reason: "no readable lockfile".to_string(),
        };
    };
    let Some(installed) = installed_paths(project_dir) else {
        return TreeState::Unverified {
            reason: format!("tree larger than {} entries", MAX_INSPECT),
        };
    };
    let installed_set: std::collections::HashSet<&str> =
        installed.iter().map(|s| s.as_str()).collect();

    // Stable iteration order: want deterministic reports and no reliance on
    // HashMap order (the same discipline the resolver needed).
    let mut keys: Vec<&String> = locked.keys().collect();
    keys.sort();

    for key in &keys {
        let want = &locked[*key];
        if !installed_set.contains(key.as_str()) {
            return TreeState::Missing {
                path: (*key).clone(),
            };
        }
    }
    for key in &keys {
        let want = &locked[*key];
        match disk_version(&project_dir.join(key).join("package.json")) {
            Some(got) if &got == want => {}
            Some(got) => {
                return TreeState::Wrong {
                    path: (*key).clone(),
                    want: want.clone(),
                    got,
                }
            }
            None => {
                return TreeState::Missing {
                    path: (*key).clone(),
                }
            }
        }
    }
    let mut installed_sorted = installed.clone();
    installed_sorted.sort();
    for path in installed_sorted {
        if !locked.contains_key(&path) {
            return TreeState::Extraneous { path };
        }
    }
    TreeState::Match
}

/// Bring the tree into agreement with the lockfile, as far as that needs no
/// resolution: verify, drop unauthorised directories, verify again.
///
/// This is the *one* place fast paths are allowed to call "did the lane finish
/// the job?" — not each lane's own gate. Lane-specific pruning is what made the
/// extraneous-package fix order-dependent: whichever lane happened to win the
/// race decided whether the tree got reconciled.
pub fn reconcile_tree(project_dir: &Path) -> TreeState {
    let state = verify_tree(project_dir);
    if state.is_match() || matches!(state, TreeState::Unverified { .. }) {
        return state;
    }
    let _ = prune_extraneous(project_dir);
    verify_tree(project_dir)
}

/// Remove directories the lockfile does not authorise.
///
/// Deletes deepest-first so a nested extraneous package goes before its parent.
/// Only called after [`verify_tree`] reported `Extraneous`, so the walk is
/// already known to be affordable.
pub fn prune_extraneous(project_dir: &Path) -> std::io::Result<usize> {
    let Some(locked) = locked_paths(project_dir) else {
        return Ok(0);
    };
    let Some(installed) = installed_paths(project_dir) else {
        return Ok(0);
    };
    let mut extraneous: Vec<String> = installed
        .into_iter()
        .filter(|p| !locked.contains_key(p))
        .collect();
    extraneous.sort_by_key(|p| std::cmp::Reverse(p.len()));
    let mut removed = 0;
    for path in extraneous {
        let dir = project_dir.join(&path);
        if std::fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(dir: &Path, lock: &str) {
        fs::create_dir_all(dir.join("node_modules")).unwrap();
        fs::write(dir.join("package-lock.json"), lock).unwrap();
    }

    fn pkg(dir: &Path, path: &str, version: &str) {
        let p = dir.join(path);
        fs::create_dir_all(&p).unwrap();
        fs::write(
            p.join("package.json"),
            format!(r#"{{"name":"x","version":"{}"}}"#, version),
        )
        .unwrap();
    }

    fn lock(entries: &[(&str, &str)]) -> String {
        let mut pkgs = String::from(r#""":{"name":"root"}"#);
        for (path, version) in entries {
            pkgs.push_str(&format!(
                r#","{}":{{"version":"{}"}}"#,
                path, version
            ));
        }
        format!(r#"{{"lockfileVersion":3,"packages":{{{}}}}}"#, pkgs)
    }

    #[test]
    fn matching_tree_is_match() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/a", "1.0.0")]));
        pkg(d, "node_modules/a", "1.0.0");
        assert_eq!(verify_tree(d), TreeState::Match);
        assert!(!verify_tree(d).needs_install());
    }

    #[test]
    fn deleted_package_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/a", "1.0.0")]));
        assert_eq!(
            verify_tree(d),
            TreeState::Missing {
                path: "node_modules/a".into()
            }
        );
    }

    /// The reproduction from the live project: a package edited on disk to
    /// 9.9.9 while the lockfile pins 4.22.3 was reported "Up to date".
    #[test]
    fn tampered_version_is_wrong_not_match() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/express", "4.22.3")]));
        pkg(d, "node_modules/express", "9.9.9");
        let state = verify_tree(d);
        assert_eq!(
            state,
            TreeState::Wrong {
                path: "node_modules/express".into(),
                want: "4.22.3".into(),
                got: "9.9.9".into(),
            }
        );
        assert!(state.needs_install());
        assert!(state.describe().contains("9.9.9"));
    }

    /// The other live reproduction: a package the lockfile never authorised was
    /// kept forever.
    #[test]
    fn unauthorised_package_is_extraneous() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/a", "1.0.0")]));
        pkg(d, "node_modules/a", "1.0.0");
        pkg(d, "node_modules/evil-leftover", "6.6.6");
        assert_eq!(
            verify_tree(d),
            TreeState::Extraneous {
                path: "node_modules/evil-leftover".into()
            }
        );
    }

    #[test]
    fn prune_removes_only_unauthorised() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/a", "1.0.0")]));
        pkg(d, "node_modules/a", "1.0.0");
        pkg(d, "node_modules/evil-leftover", "6.6.6");
        assert_eq!(prune_extraneous(d).unwrap(), 1);
        assert!(!d.join("node_modules/evil-leftover").exists());
        assert!(d.join("node_modules/a").exists());
        assert_eq!(verify_tree(d), TreeState::Match);
    }

    #[test]
    fn nested_and_scoped_entries_compare() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(
            d,
            &lock(&[
                ("node_modules/@scope/pkg", "2.0.0"),
                ("node_modules/a/node_modules/b", "3.0.0"),
                ("node_modules/a", "1.0.0"),
            ]),
        );
        pkg(d, "node_modules/@scope/pkg", "2.0.0");
        pkg(d, "node_modules/a", "1.0.0");
        pkg(d, "node_modules/a/node_modules/b", "3.0.0");
        assert_eq!(verify_tree(d), TreeState::Match);
    }

    #[test]
    fn reconcile_prunes_and_then_reports_match() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/a", "1.0.0")]));
        pkg(d, "node_modules/a", "1.0.0");
        pkg(d, "node_modules/evil-leftover", "6.6.6");
        assert!(!verify_tree(d).is_match());
        assert_eq!(reconcile_tree(d), TreeState::Match);
        assert!(!d.join("node_modules/evil-leftover").exists());
    }

    /// Reconciliation removes what is unauthorised; it must never invent a
    /// package it cannot fetch, so a missing package stays missing.
    #[test]
    fn reconcile_cannot_repair_a_missing_package() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        project(d, &lock(&[("node_modules/a", "1.0.0")]));
        assert_eq!(
            reconcile_tree(d),
            TreeState::Missing {
                path: "node_modules/a".into()
            }
        );
        assert!(reconcile_tree(d).needs_install());
    }

    #[test]
    fn no_lockfile_is_unverified_and_never_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let state = verify_tree(tmp.path());
        assert!(matches!(state, TreeState::Unverified { .. }));
        // An unverified tree must not force an install: a project that has
        // never been locked still installs normally.
        assert!(!state.needs_install());
    }

    #[test]
    fn link_and_workspace_entries_are_not_compared() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("node_modules")).unwrap();
        fs::write(
            d.join("package-lock.json"),
            r#"{"packages":{"":{"name":"root"},
                "node_modules/linked":{"resolved":"../linked","link":true},
                "node_modules/a":{"version":"1.0.0"}}}"#,
        )
        .unwrap();
        pkg(d, "node_modules/a", "1.0.0");
        assert_eq!(verify_tree(d), TreeState::Match);
    }
}
