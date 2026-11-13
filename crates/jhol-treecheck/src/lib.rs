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
use std::path::Path;

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
    /// No lockfile, or the tree is too large to inspect cheaply.
    Unverified { reason: String },
    /// The lockfile exists but cannot be read. Not "unknown" — corrupt, and a failure: a
    /// verifier that answers "unverified" to a truncated authority is lying about the one thing
    /// it exists for.
    Corrupt { path: String, detail: String },
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
            TreeState::Corrupt { path, detail } => {
                format!("{path} is corrupt: {detail}")
            }
            TreeState::Unverified { reason } => format!("tree not verified: {}", reason),
        }
    }

    /// Repairs needed to converge. `true` means an install must run rather than
    /// being skipped — which is the only thing the lanes need to know.
    pub fn needs_install(&self) -> bool {
        !matches!(self, TreeState::Match | TreeState::Unverified { .. })
    }
}

/// Does this lockfile entry apply to the configured install target?
pub fn entry_platform_applies(entry: &serde_json::Value) -> bool {
    fn target(variables: &[&str], default: Option<&str>) -> Option<String> {
        variables
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned())
            .or_else(|| default.map(str::to_owned))
    }

    fn allows(selector: Option<&serde_json::Value>, target: &str) -> bool {
        let values: Vec<&str> = match selector {
            Some(serde_json::Value::Array(values)) => values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect(),
            Some(serde_json::Value::String(value)) => vec![value],
            _ => return true,
        };
        let mut positive = false;
        let mut matched = false;
        for value in values {
            if let Some(excluded) = value.strip_prefix('!') {
                if excluded.eq_ignore_ascii_case(target) {
                    return false;
                }
            } else {
                positive = true;
                matched |= value.eq_ignore_ascii_case(target) || value == "any";
            }
        }
        !positive || matched
    }

    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let cpu = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    let libc = if cfg!(target_env = "musl") {
        Some("musl")
    } else if cfg!(target_os = "linux") {
        Some("glibc")
    } else {
        None
    };
    [
        (
            "os",
            target(
                &["JHOL_OS", "npm_config_os", "npm_config_platform"],
                Some(os),
            ),
        ),
        (
            "cpu",
            target(
                &["JHOL_CPU", "npm_config_cpu", "npm_config_arch"],
                Some(cpu),
            ),
        ),
        ("libc", target(&["JHOL_LIBC", "npm_config_libc"], libc)),
    ]
    .iter()
    .all(|(field, target)| {
        target
            .as_deref()
            .is_none_or(|target| allows(entry.get(field), target))
    })
}

enum LockedPackage<'a> {
    Registry { name: &'a str, version: &'a str },
    Link,
}

type Packages = serde_json::Map<String, serde_json::Value>;

/// Why a lockfile could not be read: it is absent, or it exists and is not usable.
///
/// The distinction is the whole point. A missing lockfile means "nothing to verify against"; a
/// lockfile that **exists but does not parse** means the authority is corrupt, and a verifier that
/// answers "unverified, exit 0" to that is lying about the only thing it is for. Measured: a
/// truncated `package-lock.json` was accepted by `jhol verify` with exit 0.
fn read_lockfile(project_dir: &Path) -> Result<Option<Packages>, (String, String)> {
    for name in ["npm-shrinkwrap.json", "package-lock.json", "jhol-lock.json"] {
        let path = project_dir.join(name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err((name.to_string(), format!("cannot read: {err}"))),
        };
        let mut lock: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| (name.to_string(), format!("not valid JSON: {e}")))?;
        let packages = lock
            .get_mut("packages")
            .ok_or_else(|| (name.to_string(), "no `packages` object".to_string()))?
            .take();
        let serde_json::Value::Object(packages) = packages else {
            return Err((name.to_string(), "`packages` is not an object".to_string()));
        };
        return Ok(Some(packages));
    }
    Ok(None)
}

/// Validate the complete lock path and return its final package name.
fn installed_name(mut key: &str) -> Option<&str> {
    fn valid(segment: &str) -> bool {
        !segment.is_empty() && !segment.starts_with('.') && !segment.contains(['\\', ':'])
    }

    loop {
        key = key.strip_prefix("node_modules/")?;
        let first = key.split('/').next()?;
        if !valid(first) {
            return None;
        }
        let mut length = first.len();
        if first.starts_with('@') {
            let second = key.get(length + 1..)?.split('/').next()?;
            if first.len() == 1 || !valid(second) || second.starts_with('@') {
                return None;
            }
            length += second.len() + 1;
        }
        if length == key.len() {
            return Some(key);
        }
        key = key.get(length + 1..)?;
    }
}

fn locked_paths(packages: &Packages, detached: bool) -> Option<HashMap<&str, LockedPackage<'_>>> {
    let mut locked = HashMap::with_capacity(packages.len());
    for (key, entry) in packages {
        if key.is_empty() {
            continue;
        }
        if !detached && !key.starts_with("node_modules/") && !key.contains("/node_modules/") {
            continue; // Workspace source record, outside the live tree.
        }
        let name = installed_name(key)?;
        let package = if entry.get("link").and_then(|value| value.as_bool()) == Some(true) {
            if detached {
                return None; // Links can depend on their original location.
            }
            LockedPackage::Link
        } else {
            let version = entry.get("version")?.as_str()?;
            let name = match entry.get("name") {
                Some(value) => value.as_str()?,
                None => name,
            };
            if name.is_empty() || version.is_empty() {
                return None;
            }
            if entry.get("optional").and_then(|value| value.as_bool()) == Some(true)
                && !entry_platform_applies(entry)
            {
                continue;
            }
            LockedPackage::Registry { name, version }
        };
        locked.insert(key.as_str(), package);
    }
    Some(locked)
}

/// Walk package locations, scopes and nested node_modules. A failed directory
/// read cannot establish absence. Never descend through a package symlink.
fn installed_paths(root: &Path, detached: bool) -> std::io::Result<Vec<String>> {
    use std::io::{Error, ErrorKind};

    let mut installed = Vec::new();
    let mut inspected = 0;
    let mut queue = vec![(root.to_path_buf(), "node_modules".to_owned(), false)];
    while let Some((directory, prefix, scoped)) = queue.pop() {
        let metadata = match std::fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(err) if !scoped && err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        if !metadata.is_dir() {
            return Err(Error::other(format!(
                "{} is not a directory",
                directory.display()
            )));
        }
        for entry in std::fs::read_dir(&directory)? {
            inspected += 1;
            if inspected > MAX_INSPECT {
                return Err(Error::other(format!(
                    "tree larger than {MAX_INSPECT} entries"
                )));
            }
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::other("node_modules contains a non-Unicode entry"))?;
            if name.starts_with('.') {
                continue;
            }
            let kind = entry.file_type()?;
            let relative = format!("{prefix}/{name}");
            if !scoped && name.starts_with('@') {
                if kind.is_dir() || kind.is_symlink() {
                    queue.push((entry.path(), relative, true));
                }
                continue;
            }
            if !kind.is_dir() && !kind.is_symlink() {
                continue;
            }
            if kind.is_symlink() && detached {
                return Err(Error::other(format!("{relative} is a symlink")));
            }
            if kind.is_dir() {
                queue.push((
                    entry.path().join("node_modules"),
                    format!("{relative}/node_modules"),
                    false,
                ));
            }
            installed.push(relative);
        }
    }
    Ok(installed)
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
fn lockfile_is_complete(packages: &Packages) -> bool {
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
    inspect_tree(project_dir).state
}

/// A tree verdict together with the counts that decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeReport {
    pub state: TreeState,
    /// Directories found under `node_modules`: the installed package count.
    pub installed: usize,
}

/// Verdict *and* installed count from **one** walk.
///
/// A dashboard that shows both used to walk the tree twice: once here and once in a
/// counter that also called `std::env::var` per entry. The walk is the expensive part of
/// a status refresh, so it happens once and reports both facts.
pub fn inspect_tree(project_dir: &Path) -> TreeReport {
    inspect_tree_at(project_dir, &project_dir.join("node_modules"), false)
}

fn inspect_tree_at(project_dir: &Path, tree: &Path, detached: bool) -> TreeReport {
    let unverified = |reason| TreeReport {
        state: TreeState::Unverified { reason },
        installed: 0,
    };
    let packages = match read_lockfile(project_dir) {
        Ok(Some(packages)) => packages,
        Ok(None) => return unverified("no lockfile".to_owned()),
        Err((path, detail)) => {
            return TreeReport {
                state: TreeState::Corrupt { path, detail },
                installed: 0,
            }
        }
    };
    let Some(locked) = locked_paths(&packages, detached) else {
        return unverified("lockfile layout cannot be verified".to_owned());
    };
    let installed = match installed_paths(tree, detached) {
        Ok(installed) => installed,
        Err(err) => return unverified(err.to_string()),
    };
    TreeReport {
        state: verify_against(tree, &packages, &locked, &installed),
        installed: installed.len(),
    }
}

fn verify_against(
    tree: &Path,
    packages: &Packages,
    locked: &HashMap<&str, LockedPackage<'_>>,
    installed: &[String],
) -> TreeState {
    let installed_set: std::collections::HashSet<&str> =
        installed.iter().map(String::as_str).collect();
    let mut keys: Vec<_> = locked.keys().copied().collect();
    keys.sort_unstable();

    for &key in &keys {
        if !installed_set.contains(key) {
            return TreeState::Missing {
                path: key.to_owned(),
            };
        }
    }

    let read_one = |key: &str| {
        let package_dir = tree.join(&key["node_modules/".len()..]);
        let missing = || TreeState::Missing {
            path: key.to_owned(),
        };
        let LockedPackage::Registry { name, version } = &locked[key] else {
            return (!package_dir.is_dir()).then(missing);
        };
        let Some(manifest) = std::fs::read(package_dir.join("package.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            return Some(missing());
        };
        let got_version = manifest.get("version").and_then(|value| value.as_str());
        let got_name = manifest.get("name").and_then(|value| value.as_str());
        match (got_name, got_version) {
            (_, Some(got)) if got != *version => Some(TreeState::Wrong {
                path: key.to_owned(),
                want: (*version).to_owned(),
                got: got.to_owned(),
            }),
            (Some(got), Some(_)) if got != *name => Some(TreeState::Wrong {
                path: key.to_owned(),
                want: format!("{name}@{version}"),
                got: format!("{got}@{version}"),
            }),
            (Some(_), Some(_)) => None,
            _ => Some(missing()),
        }
    };

    // Parallel manifest reads matter when the restored metadata is cold.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(keys.len().max(1));
    let chunk = keys.len().div_ceil(threads).max(1);
    let mismatch = std::thread::scope(|scope| {
        let handles: Vec<_> = keys
            .chunks(chunk)
            .map(|part| {
                let read_one = &read_one;
                std::thread::Builder::new()
                    .spawn_scoped(scope, move || part.iter().find_map(|key| read_one(key)))
            })
            .collect();
        let mut mismatch = None;
        for (handle, part) in handles.into_iter().zip(keys.chunks(chunk)) {
            let result = match handle {
                Ok(handle) => handle.join().unwrap_or_else(|_| {
                    Some(TreeState::Unverified {
                        reason: "manifest inspection failed".to_owned(),
                    })
                }),
                Err(_) => part.iter().find_map(|key| read_one(key)),
            };
            mismatch = mismatch.or(result);
        }
        mismatch
    });
    if let Some(mismatch) = mismatch {
        return mismatch;
    }

    if let Some(path) = installed
        .iter()
        .filter(|p| !locked.contains_key(p.as_str()))
        .min()
    {
        if !lockfile_is_complete(packages) {
            return TreeState::Unverified {
                reason: "lockfile does not account for every package it installed".to_owned(),
            };
        }
        return TreeState::Extraneous { path: path.clone() };
    }
    TreeState::Match
}

/// Verify a detached node_modules tree against the project's lockfile.
pub fn subtree_matches(project_dir: &Path, tree: &Path) -> bool {
    inspect_tree_at(project_dir, tree, true).state.is_match()
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
    // A corrupt lockfile authorises nothing, so pruning would be arbitrary: refuse instead of
    // deleting against an authority we cannot read.
    let Ok(Some(packages)) = read_lockfile(project_dir) else {
        return Ok(0);
    };
    if !lockfile_is_complete(&packages) {
        return Ok(0);
    }
    let Some(locked) = locked_paths(&packages, false) else {
        return Ok(0);
    };
    let mut extraneous: Vec<String> = installed_paths(&project_dir.join("node_modules"), false)?
        .into_iter()
        .filter(|p| !locked.contains_key(p.as_str()))
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
            serde_json::json!({
                "name": installed_name(path).unwrap(),
                "version": version,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn lock(entries: &[(&str, &str)]) -> String {
        let mut pkgs = String::from(r#""":{"name":"root"}"#);
        for (path, version) in entries {
            pkgs.push_str(&format!(r#","{}":{{"version":"{}"}}"#, path, version));
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
    fn detached_layout_rejects_extras_and_wrong_package_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let slot = root.join("snapshot");
        project(root, &lock(&[("node_modules/a", "1.0.0")]));
        pkg(&slot, "node_modules/a", "1.0.0");
        let tree = slot.join("node_modules");
        assert!(subtree_matches(root, &tree));
        assert!(!verify_tree(root).is_match());

        pkg(&slot, "node_modules/a/node_modules/@scope/extra", "1.0.0");
        assert!(!subtree_matches(root, &tree));
        fs::remove_dir_all(tree.join("a/node_modules")).unwrap();
        fs::write(
            tree.join("a/package.json"),
            r#"{"name":"wrong","version":"1.0.0"}"#,
        )
        .unwrap();
        assert!(!subtree_matches(root, &tree));
    }

    #[test]
    fn unsafe_lock_paths_are_unverified() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for path in [
            "node_modules/../outside",
            "node_modules/a/extra",
            "node_modules//a",
        ] {
            project(root, &lock(&[(path, "1.0.0")]));
            assert!(matches!(verify_tree(root), TreeState::Unverified { .. }));
            assert!(!subtree_matches(root, &root.join("node_modules")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_scopes_are_unverified_and_are_not_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        project(root, &lock(&[]));
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("node_modules/@scope")).unwrap();
        assert!(matches!(verify_tree(root), TreeState::Unverified { .. }));
        assert!(prune_extraneous(root).is_err());
        assert!(outside.is_dir());
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

#[cfg(test)]
mod corrupt_lockfile_tests {
    use super::*;

    /// A lockfile that exists and does not parse is a **failure**, not "unverified". The audit
    /// harness caught this: `jhol verify` exited 0 on `{"packages":` — a verifier that passes a
    /// corrupt authority is worse than no verifier.
    #[test]
    fn a_truncated_lockfile_is_corrupt_not_unverified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/a")).unwrap();
        std::fs::write(dir.path().join("node_modules/a/package.json"), r#"{"name":"a","version":"1.0.0"}"#).unwrap();
        std::fs::write(dir.path().join("package-lock.json"), r#"{"packages":"#).unwrap();
        match verify_tree(dir.path()) {
            TreeState::Corrupt { path, detail } => {
                assert_eq!(path, "package-lock.json");
                assert!(detail.contains("JSON"), "{detail}");
            }
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    /// Absent stays "unverified": there is nothing to disagree with, and that distinction is the
    /// reason the two states exist separately.
    #[test]
    fn an_absent_lockfile_stays_unverified() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(verify_tree(dir.path()), TreeState::Unverified { .. }));
    }

    /// A lockfile without a `packages` object is also corrupt: npm's schema requires it.
    #[test]
    fn a_lockfile_without_packages_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package-lock.json"), r#"{"lockfileVersion":3}"#).unwrap();
        assert!(matches!(verify_tree(dir.path()), TreeState::Corrupt { .. }));
    }
}
