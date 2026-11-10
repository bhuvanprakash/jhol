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

/// Does a lockfile entry's platform restriction apply to this machine?
///
/// npm records `os`/`cpu`/`libc` lists on optional platform-specific packages
/// (esbuild publishes one per target: `@esbuild/aix-ppc64`, `@esbuild/darwin-arm64`,
/// …). Exactly one of them installs here; the rest are *legitimately absent*, and a
/// verifier that demands them reports a missing package on a perfectly good tree.
/// Found on `vite`, which ships 21 of them.
///
/// Semantics follow npm: a plain list is an allow-list (any match passes), a
/// leading `!` makes it a deny-list (no match may appear).
pub fn entry_platform_applies(entry: &serde_json::Value) -> bool {
    fn list(entry: &serde_json::Value, key: &str) -> Vec<String> {
        match entry.get(key) {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    // Current machine in npm's vocabulary.
    let here: &[&str] = &match std::env::consts::OS {
        "macos" => &["darwin"][..],
        "windows" => &["win32"][..],
        other => &[other][..], // linux is "linux" in npm too
    };
    let arch: &[&str] = &match std::env::consts::ARCH {
        "x86_64" => &["x64"][..],
        "aarch64" => &["arm64"][..],
        "x86" => &["ia32"][..],
        other => &[other][..],
    };

    let mut check = |values: Vec<String>, candidates: &[&str]| -> bool {
        if values.is_empty() {
            return true;
        }
        let (negated, positive): (Vec<String>, Vec<String>) =
            values.into_iter().partition(|v| v.starts_with('!'));
        if !negated.is_empty() {
            // A deny-list: nothing listed may be present.
            return !negated
                .iter()
                .map(|n| n.trim_start_matches('!'))
                .any(|n| candidates.iter().any(|c| c.eq_ignore_ascii_case(n)));
        }
        positive
            .iter()
            .any(|p| candidates.iter().any(|c| c.eq_ignore_ascii_case(p)))
    };

    if !check(list(entry, "os"), here) {
        return false;
    }
    if !check(list(entry, "cpu"), arch) {
        return false;
    }
    // `libc` only distinguishes Linux distributions; elsewhere it never applies.
    let libc = list(entry, "libc");
    if !libc.is_empty() {
        let current: &[&str] = if cfg!(target_env = "musl") {
            &["musl"]
        } else if cfg!(target_os = "linux") {
            &["glibc"]
        } else {
            &[]
        };
        if !check(libc, current) {
            return false;
        }
    }
    true
}

/// The lockfile's view: tree path -> locked version.
///
/// Keys are `package-lock.json` `packages` keys (`node_modules/a`,
/// `node_modules/a/node_modules/b`). The root entry (`""`) carries no version
/// and is skipped.
/// Install locations that the lockfile records as **links** (`link:`, `file:`,
/// workspace). They carry no registry version, so they were skipped entirely — which
/// made a link-only project verify as `Match` with `node_modules` deleted, and `install`
/// then answer "Up to date (0 packages)" and create nothing.
fn locked_links(project_dir: &Path) -> Vec<String> {
    let Some(path) = ["package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| project_dir.join(n))
        .find(|p| p.exists())
    else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut out: Vec<String> = v
        .get("packages")
        .and_then(|p| p.as_object())
        .map(|packages| {
            packages
                .iter()
                .filter(|(_, entry)| entry.get("link").and_then(|x| x.as_bool()) == Some(true))
                .map(|(key, _)| key.clone())
                .filter(|key| key.contains("node_modules/"))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

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
        // A package for another OS/CPU/libc cannot be on this disk. Demanding it
        // reported `@esbuild/aix-ppc64` missing on darwin/arm64 — a false alarm on
        // a correct tree (21 such packages in `vite` alone).
        if !entry_platform_applies(entry) {
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

/// Does the lockfile account for everything its own entries declare?
///
/// A lockfile can be *incomplete*: the installer materialises a tree and the
/// lockfile writer records a subset. Real case found while wiring this up —
/// `express@latest` installed 68 packages and wrote 56 entries, and none of the
/// 12 missing ones appeared in any entry's `dependencies` either. Such a
/// lockfile is not an authority on what is authorised, so pruning against it
/// deletes packages a previous install legitimately created.
///
/// Conservative on purpose: we prune only when every dependency declared by a
/// locked package is itself locked. Otherwise the tree is reported but never
/// modified.
fn lockfile_is_complete(project_dir: &Path) -> bool {
    let Some(path) = ["package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| project_dir.join(n))
        .find(|p| p.exists())
    else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(packages) = v.get("packages").and_then(|p| p.as_object()) else {
        return false;
    };
    // Names that are locked, keyed the way dependency maps refer to them.
    let mut locked_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for key in packages.keys() {
        if let Some(idx) = key.rfind("node_modules/") {
            locked_names.insert(&key[idx + "node_modules/".len()..]);
        }
    }
    for entry in packages.values() {
        for section in ["dependencies", "optionalDependencies"] {
            let Some(deps) = entry.get(section).and_then(|d| d.as_object()) else {
                continue;
            };
            for name in deps.keys() {
                // A nested duplicate is legal; the name must exist somewhere.
                if !locked_names.contains(name.as_str()) {
                    return false;
                }
            }
        }
    }
    true
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

    // Links must exist too. They are not version-compared (a link has no registry
    // version), but a lockfile that records one is asking for a path on disk.
    let links = locked_links(project_dir);
    for path in &links {
        if !project_dir.join(path).exists() {
            return TreeState::Missing { path: path.clone() };
        }
    }

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
    // Read the manifests **in parallel**. After a snapshot restore the tree's metadata is
    // cold, and 71 sequential opens cost ~10 ms of a 17 ms warm install. Chunked with
    // `std::thread::scope` rather than a new dependency: this crate deliberately carries
    // only `serde_json`.
    let read_one = |key: &String| {
        (
            key.clone(),
            disk_version(&project_dir.join(key).join("package.json")),
        )
    };
    let versions: Vec<(String, Option<String>)> = {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(keys.len().max(1));
        let chunk = keys.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            let handles: Vec<_> = keys
                .chunks(chunk)
                .map(|part| {
                    let read_one = &read_one;
                    scope.spawn(move || part.iter().map(|k| read_one(k)).collect::<Vec<_>>())
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };
    for (key, got) in &versions {
        let key = key;
        let want = &locked[key];
        match got.clone() {
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
    let has_extras = installed_sorted
        .iter()
        .any(|p| !locked.contains_key(p) && !links.iter().any(|l| l == p));
    if has_extras && !lockfile_is_complete(project_dir) {
        // Installed packages the lockfile does not mention, *and* a lockfile
        // that does not account for its own dependencies: this is a lockfile
        // defect, not an unauthorised package. Report it; never delete for it.
        return TreeState::Unverified {
            reason: "lockfile does not account for every package it installed".to_string(),
        };
    }
    for path in installed_sorted {
        if !locked.contains_key(&path) && !links.iter().any(|l| l == &path) {
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
    // Pruning is opt-in.
    //
    // It is only sound when the lockfile is a complete record of what belongs on
    // disk, and today it sometimes is not: `express@latest` installs 68 packages
    // and writes 56 entries, and the 12 omissions appear in no dependency map
    // either — so the lockfile looks internally consistent while being wrong.
    // Deleting on that basis removed real packages from a working tree. Until
    // the lockfile writer records everything it materialises, the safe default
    // is to *report* an unauthorised directory and leave it alone;
    // `jhol prune` (JHOL_PRUNE_EXTRANEOUS=1) opts in.
    let prune = std::env::var_os("JHOL_PRUNE_EXTRANEOUS").is_some();
    if prune && matches!(state, TreeState::Extraneous { .. }) {
        let _ = prune_extraneous(project_dir);
        return verify_tree(project_dir);
    }
    state
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
/// Tests that mutate process environment variables must not run concurrently:
/// `JHOL_PRUNE_EXTRANEOUS` gates pruning, and two tests setting and clearing it in
/// parallel made the suite flaky (each occasionally saw the other's value).
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
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
        // Pruning is opt-in: a lockfile can be an incomplete record.
        let _guard = env_lock();
        std::env::set_var("JHOL_PRUNE_EXTRANEOUS", "1");
        let reconciled = reconcile_tree(d);
        std::env::remove_var("JHOL_PRUNE_EXTRANEOUS");
        assert_eq!(reconciled, TreeState::Match);
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

    /// A link entry has no registry version, so it is never *version*-compared — but a
    /// lockfile that records one is asking for a path on disk. Skipping them entirely
    /// made a link-only project verify as `Match` after `node_modules` was deleted, and
    /// `jhol install` then answered "Up to date (0 packages)" and created nothing.
    #[test]
    fn link_entries_must_exist_but_are_not_version_compared() {
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

        // Present, with any version: whatever the link points at is not compared.
        fs::create_dir_all(d.join("node_modules/linked")).unwrap();
        fs::write(
            d.join("node_modules/linked/package.json"),
            r#"{"name":"linked","version":"9.9.9"}"#,
        )
        .unwrap();
        assert_eq!(verify_tree(d), TreeState::Match);

        // Absent: that is the defect this test used to bless.
        fs::remove_dir_all(d.join("node_modules/linked")).unwrap();
        assert_eq!(
            verify_tree(d),
            TreeState::Missing {
                path: "node_modules/linked".into()
            }
        );
    }
}

#[cfg(test)]
mod completeness_tests {
    use super::*;
    use std::fs;

    /// A lockfile that omits packages it itself declares must not authorise
    /// deletions: the reproduction is `express@latest`, where the installer
    /// wrote 56 entries for 68 installed packages and none of the 12 appeared in
    /// any dependency map either. Pruning against it removed real code.
    #[test]
    fn incomplete_lockfile_is_unverified_not_extraneous() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("node_modules/a")).unwrap();
        // The lockfile declares a dependency on `ghost`, which it never locks.
        fs::write(
            d.join("package-lock.json"),
            r#"{"packages":{"":{"name":"root"},"node_modules/a":{"version":"1.0.0","dependencies":{"ghost":"^1"}}}}"#,
        )
        .unwrap();
        fs::write(
            d.join("node_modules/a/package.json"),
            r#"{"name":"a","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::create_dir_all(d.join("node_modules/ghost")).unwrap();
        fs::write(
            d.join("node_modules/ghost/package.json"),
            r#"{"name":"ghost","version":"1.0.0"}"#,
        )
        .unwrap();

        let state = verify_tree(d);
        assert!(
            matches!(state, TreeState::Unverified { .. }),
            "expected Unverified, got {:?}",
            state
        );
        // Reconciliation must leave the tree alone.
        assert_eq!(reconcile_tree(d), state);
        assert!(d.join("node_modules/ghost").exists());
    }

    /// A complete lockfile still prunes real intruders — the guard must not
    /// disable the feature it exists to make safe.
    #[test]
    fn complete_lockfile_still_authorises_pruning() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("node_modules/a")).unwrap();
        fs::write(
            d.join("package-lock.json"),
            r#"{"packages":{"":{"name":"root"},"node_modules/a":{"version":"1.0.0"}}}"#,
        )
        .unwrap();
        fs::write(
            d.join("node_modules/a/package.json"),
            r#"{"name":"a","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::create_dir_all(d.join("node_modules/intruder")).unwrap();
        fs::write(
            d.join("node_modules/intruder/package.json"),
            r#"{"name":"intruder","version":"9.9.9"}"#,
        )
        .unwrap();

        assert_eq!(
            verify_tree(d),
            TreeState::Extraneous {
                path: "node_modules/intruder".into()
            }
        );
        // Pruning is opt-in (see reconcile_tree): a lockfile can be wrong.
        let _guard = env_lock();
        std::env::set_var("JHOL_PRUNE_EXTRANEOUS", "1");
        let reconciled = reconcile_tree(d);
        std::env::remove_var("JHOL_PRUNE_EXTRANEOUS");
        assert_eq!(reconciled, TreeState::Match);
        assert!(!d.join("node_modules/intruder").exists());
        assert!(d.join("node_modules/a").exists());
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn other_platform_entries_are_not_required_here() {
        // Whichever machine runs the test, these cannot apply everywhere.
        let aix = json!({ "version": "0.21.5", "optional": true, "os": ["aix"], "cpu": ["ppc64"] });
        assert!(!entry_platform_applies(&aix));

        let linux_musl = json!({ "version": "1.0.0", "optional": true, "os": ["linux"], "libc": ["musl"] });
        assert!(!entry_platform_applies(&linux_musl));
    }

    #[test]
    fn this_platform_is_required() {
        let here = json!({ "version": "1.0.0", "os": [std::env::consts::OS] });
        // `macos` vs npm's `darwin` differ; a wrong-vocabulary list is simply a
        // non-matching allow-list, which is the safe reading.
        let expects = std::env::consts::OS != "macos";
        assert_eq!(entry_platform_applies(&here), expects);

        let any = json!({ "version": "1.0.0" });
        assert!(entry_platform_applies(&any), "no restriction means every platform");
    }

    #[test]
    fn negated_lists_exclude() {
        let not_linux = json!({ "version": "1.0.0", "os": ["!linux"] });
        let is_linux = std::env::consts::OS == "linux";
        assert_eq!(entry_platform_applies(&not_linux), !is_linux);
    }
}
