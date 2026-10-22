//! Dependency specifier grammar (npm parity).
//!
//! `package.json` dependency values are not always registry ranges. npm accepts
//! several other specifier kinds, and a package manager that only understands
//! `name@range` silently sends things like `file:../lib` or `npm:lodash@^4` to
//! the registry as if they were package names — which fails.
//!
//! This module is the single place that knows how to classify a specifier:
//!
//! | spec                      | meaning                                    |
//! |---------------------------|--------------------------------------------|
//! | `^1.2.3`, `1.x`, `*`, `latest` | registry range                        |
//! | `npm:<real>@<range>`      | install `<real>` under the alias name      |
//! | `file:../lib`             | local directory (symlinked)                |
//! | `link:../lib`             | local directory (always symlinked)         |
//! | `workspace:*`, `workspace:^1` | local workspace package                |
//! | `git+https://…`, `github:u/r#tag`, `u/r` | git repository            |
//!
//! Classification is pure and unit-tested; the installer consumes it to
//! materialize the local kinds directly and to route aliases.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A classified dependency specifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    /// Ordinary registry range (`^1.2.3`, `latest`, `1.x`, …).
    Registry(String),
    /// `npm:<real>@<range>` — install `real` under the alias name.
    Alias { real: String, range: String },
    /// `file:` / `link:` local directory.
    Local { path: PathBuf, link: bool },
    /// `workspace:<range>` local workspace package.
    Workspace { range: String },
    /// Git repository.
    Git {
        url: String,
        committish: Option<String>,
    },
}

impl Spec {
    /// True for specifiers that must NOT be sent to the registry as a package.
    pub fn is_local(&self) -> bool {
        matches!(self, Spec::Local { .. } | Spec::Workspace { .. })
    }

    /// True when the specifier needs direct materialization rather than registry
    /// resolution.
    pub fn is_special(&self) -> bool {
        !matches!(self, Spec::Registry(_))
    }
}

/// Classify a `package.json` dependency specifier.
pub fn classify(spec: &str) -> Spec {
    let spec = spec.trim();

    if let Some(rest) = spec.strip_prefix("npm:") {
        // `npm:real@range` or `npm:real` (range defaults to `*`).
        return match split_name_range(rest) {
            Some((real, range)) if !real.is_empty() => Spec::Alias {
                real: real.to_string(),
                range: if range.is_empty() {
                    "*".to_string()
                } else {
                    range.to_string()
                },
            },
            _ => Spec::Alias {
                real: rest.to_string(),
                range: "*".to_string(),
            },
        };
    }

    if let Some(rest) = spec.strip_prefix("file:") {
        return Spec::Local {
            path: PathBuf::from(rest),
            link: false,
        };
    }

    if let Some(rest) = spec.strip_prefix("link:") {
        return Spec::Local {
            path: PathBuf::from(rest),
            link: true,
        };
    }

    if let Some(rest) = spec.strip_prefix("workspace:") {
        return Spec::Workspace {
            range: if rest.is_empty() {
                "*".to_string()
            } else {
                rest.to_string()
            },
        };
    }

    if let Some(git) = classify_git(spec) {
        return git;
    }

    Spec::Registry(spec.to_string())
}

/// Recognize git specifiers: `git+…`, `git://…`, `github:u/r`, `gitlab:…`,
/// `bitbucket:…`, and the bare `user/repo[#ref]` shorthand.
fn classify_git(spec: &str) -> Option<Spec> {
    if let Some(rest) = spec
        .strip_prefix("git+")
        .or_else(|| spec.strip_prefix("git://"))
    {
        let (url, committish) = split_hash(rest);
        return Some(Spec::Git {
            url: url.to_string(),
            committish,
        });
    }
    for host in ["github:", "gitlab:", "bitbucket:"] {
        if let Some(rest) = spec.strip_prefix(host) {
            let (repo, committish) = split_hash(rest);
            return Some(Spec::Git {
                url: format!("https://{}.com/{}", host.trim_end_matches(':'), repo),
                committish,
            });
        }
    }
    // Bare `user/repo[#ref]` — must not look like a range or a path.
    if !spec.contains(':')
        && !spec.starts_with('.')
        && !spec.starts_with('/')
        && !spec.starts_with('@')
        && spec.matches('/').count() == 1
        && !spec.contains(' ')
    {
        let (repo, committish) = split_hash(spec);
        let (owner, name) = repo.split_once('/')?;
        if !owner.is_empty() && !name.is_empty() && !name.contains('@') {
            return Some(Spec::Git {
                url: format!("https://github.com/{}", repo),
                committish,
            });
        }
    }
    None
}

fn split_hash(s: &str) -> (&str, Option<String>) {
    match s.split_once('#') {
        Some((repo, ref_)) if !ref_.is_empty() => (repo, Some(ref_.to_string())),
        _ => (s, None),
    }
}

/// Split `name@range`, honouring a leading `@scope/`.
fn split_name_range(s: &str) -> Option<(&str, &str)> {
    let at = s.rfind('@')?;
    if at == 0 {
        // `@scope/pkg` with no range.
        return None;
    }
    Some((&s[..at], &s[at + 1..]))
}

/// A dependency in `package.json` that is not a plain registry range, paired
/// with where it lives on disk once materialized.
#[derive(Debug, Clone)]
pub struct SpecialDep {
    pub name: String,
    pub spec: String,
    pub kind: SpecialKind,
}

#[derive(Debug, Clone)]
pub enum SpecialKind {
    /// Local directory (`file:`/`link:`/`workspace:`), already symlinked.
    Local {
        dir: PathBuf,
        version: String,
        link: bool,
    },
    /// `npm:` alias: the real package is resolved normally and the alias name
    /// is symlinked to it afterwards.
    Alias { real: String, range: String },
    /// Git repository — cloned into the cache and symlinked.
    Git {
        url: String,
        committish: Option<String>,
        dir: Option<PathBuf>,
        version: String,
    },
}

impl SpecialDep {
    /// The spec that should be resolved from the registry on the alias's
    /// behalf, if any.
    pub fn registry_spec(&self) -> Option<String> {
        match &self.kind {
            SpecialKind::Alias { real, range } => Some(format!("{}@{}", real, range)),
            _ => None,
        }
    }
}

/// Classify every dependency in a `name -> spec` map, returning only the
/// non-registry ones.
pub fn specials_from_deps(deps: &std::collections::HashMap<String, String>) -> Vec<SpecialDep> {
    let mut out = Vec::new();
    for (name, spec) in deps {
        match classify(spec) {
            Spec::Registry(_) => {}
            Spec::Alias { real, range } => out.push(SpecialDep {
                name: name.clone(),
                spec: spec.clone(),
                kind: SpecialKind::Alias { real, range },
            }),
            Spec::Local { path, link } => out.push(SpecialDep {
                name: name.clone(),
                spec: spec.clone(),
                kind: SpecialKind::Local {
                    dir: path,
                    version: String::new(),
                    link,
                },
            }),
            Spec::Workspace { range: _ } => out.push(SpecialDep {
                name: name.clone(),
                spec: spec.clone(),
                kind: SpecialKind::Local {
                    dir: PathBuf::new(),
                    version: String::new(),
                    link: true,
                },
            }),
            Spec::Git { url, committish } => out.push(SpecialDep {
                name: name.clone(),
                spec: spec.clone(),
                kind: SpecialKind::Git {
                    url,
                    committish,
                    dir: None,
                    version: String::new(),
                },
            }),
        }
    }
    out
}

/// Read the *runtime* dependencies of a directory's `package.json`.
///
/// Git checkouts must not drag in `devDependencies` (a repo's test tooling is
/// not part of its consumable surface, and it can conflict with the app's own
/// tree — e.g. hoisting a different `is-number` over the checked-out one).
pub fn runtime_deps(dir: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(raw) = std::fs::read_to_string(dir.join("package.json")) else {
        return out;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    for key in ["dependencies", "optionalDependencies", "peerDependencies"] {
        if let Some(d) = json.get(key).and_then(|d| d.as_object()) {
            for (k, v) in d {
                if let Some(s) = v.as_str() {
                    out.entry(k.clone()).or_insert_with(|| s.to_string());
                }
            }
        }
    }
    out
}

/// Read `version` from a directory's `package.json`.
pub fn dir_version(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Resolve the directory a local spec points at, relative to `project_dir`.
pub fn resolve_local_dir(project_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        normalize(&project_dir.join(path))
    }
}

/// Find a workspace package directory by package name, using the root
/// manifest's `workspaces` globs.
pub fn find_workspace_dir(project_dir: &Path, name: &str) -> Option<PathBuf> {
    let roots = crate::workspaces::list_workspace_roots(project_dir).ok()?;
    for root in roots {
        // `list_workspace_roots` returns paths *relative to `project_dir`*, so they
        // must be joined with it. Reading them as-is only worked when the process
        // cwd happened to be the project directory.
        let abs = if root.is_absolute() {
            root
        } else {
            project_dir.join(&root)
        };
        if dir_name(&abs).as_deref() == Some(name) {
            return Some(abs);
        }
    }
    None
}

fn dir_name(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract a local `file:` **tarball** into the cache and return its directory.
///
/// npm accepts `file:../pkg-1.0.0.tgz` and installs the packed package. Symlinking
/// the `.tgz` itself (what a directory-oriented `file:` handler does) yields a
/// `node_modules/<name>` that is a plain file, so every `require` fails.
pub fn materialize_local_tarball(tarball: &Path) -> Option<PathBuf> {
    materialize_local_tarball_into(tarball, Path::new(&crate::utils::get_cache_dir()))
}

/// As [`materialize_local_tarball`], but with an explicit cache root so the
/// extraction is testable without writing into the user's cache.
pub fn materialize_local_tarball_into(tarball: &Path, cache_root: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(tarball).ok()?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Key on the path plus size/mtime so replacing the tarball re-extracts.
    let key = format!(
        "{}-{:016x}-{}-{}",
        sanitize(&tarball.to_string_lossy()),
        short_hash(&tarball.to_string_lossy()),
        meta.len(),
        stamp
    );
    let dest = cache_root.join("local-tarballs").join(&key);
    if dest.join("package.json").exists() {
        return Some(dest);
    }

    let file = std::fs::File::open(tarball).ok()?;
    let reader = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(reader);
    std::fs::create_dir_all(&dest).ok()?;

    for entry in archive.entries().ok()? {
        let mut entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = match entry.path() {
            Ok(p) => p.into_owned(),
            Err(_) => continue,
        };
        // npm tarballs nest everything under `package/`; strip it.
        let rel: PathBuf = path.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(&rel);
        // A local `file:` tarball is still attacker-controlled content (a repo can
        // point at one it ships). Reuse the registry extractor's containment rules
        // so `..`, absolute paths, hard links and symlink write-through cannot
        // escape the cache directory. Fail closed on the whole archive.
        let stripped: Vec<&str> = path
            .components()
            .skip(1)
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        let entry_type = entry.header().entry_type();
        let link_name = entry.link_name().ok().flatten().map(|p| p.into_owned());
        if crate::registry::validate_archive_path(
            &path.to_string_lossy(),
            &dest,
            &stripped,
            entry_type,
            link_name.as_deref(),
        )
        .is_err()
        {
            let _ = std::fs::remove_dir_all(&dest);
            return None;
        }
        if entry_type.is_dir() {
            let _ = std::fs::create_dir_all(&target);
        } else {
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = entry.unpack(&target);
        }
    }

    if dest.join("package.json").exists() {
        Some(dest)
    } else {
        let _ = std::fs::remove_dir_all(&dest);
        None
    }
}

/// True for a `file:` spec that points at a tarball rather than a directory.
pub fn is_tarball_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.ends_with(".tgz") || name.ends_with(".tar.gz") || name.ends_with(".tar")
}

/// Clone (or reuse) a git dependency into the cache and return its directory.
///
/// Kept deliberately simple: `git` must be on PATH, the checkout is cached by
/// URL+committish, and failures are reported (never silently dropped).
pub fn materialize_git_dir(url: &str, committish: Option<&str>) -> Option<PathBuf> {
    let cache = Path::new(&crate::utils::get_cache_dir()).join("git");
    let key = format!(
        "{}-{:016x}",
        sanitize(url),
        short_hash(&format!("{}#{}", url, committish.unwrap_or("")))
    );
    let dest = cache.join(&key);
    if !dest.join("package.json").exists() {
        std::fs::create_dir_all(&cache).ok()?;
        let tmp = cache.join(format!("{}.tmp{}", key, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut cmd = std::process::Command::new("git");
        cmd.arg("clone").arg("--depth").arg("1");
        if let Some(c) = committish {
            cmd.arg("--branch").arg(c);
        }
        cmd.arg(url).arg(&tmp);
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                crate::utils::log(&format!(
                    "warning: could not run git for {} ({}); skipping",
                    url, e
                ));
                return None;
            }
        };
        if !out.status.success() {
            crate::utils::log(&format!(
                "warning: git clone failed for {}: {}",
                url,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            let _ = std::fs::remove_dir_all(&tmp);
            return None;
        }
        let _ = std::fs::remove_dir_all(&dest);
        if std::fs::rename(&tmp, &dest).is_err() {
            // Losing this race used to fail silently: a concurrent process renames its own
            // `tmp` into `dest`, so this rename targets an existing non-empty directory and
            // returns `None` — the caller then skipped the checkout, its link and its whole
            // dependency subtree, with no warning (measured: 2 of 3 runs of the same fixture).
            // Accept an existing complete clone; only report when there is genuinely nothing.
            let _ = std::fs::remove_dir_all(&tmp);
            if !dest.join("package.json").exists() {
                crate::utils::log(&format!(
                    "warning: could not materialize git dependency {} ({}); its dependencies cannot be resolved",
                    url,
                    committish.unwrap_or("default branch")
                ));
                return None;
            }
        }
    }
    if !dest.join("package.json").exists() {
        // A directory that exists but has no manifest is not a checkout (interrupted clone,
        // leftover tmp, or a failed rename). Treat it as absent rather than trusting it.
        crate::utils::log(&format!(
            "warning: git checkout for {} ({}) is incomplete; removing it and retrying next run",
            url,
            committish.unwrap_or("default branch")
        ));
        let _ = std::fs::remove_dir_all(&dest);
        return None;
    }
    Some(dest)
}

fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    out.truncate(48);
    out
}

fn short_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Lexically normalize `.` / `..` components so relative paths stay readable.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Every dependency kind of the `package.json` in `dir`.
fn read_all_deps(dir: &Path) -> HashMap<String, String> {
    let mut deps =
        crate::lockfile::read_package_json_deps(&dir.join("package.json")).unwrap_or_default();
    if let Ok(raw) = std::fs::read_to_string(dir.join("package.json")) {
        if raw.contains("optionalDependencies") || raw.contains("peerDependencies") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                for key in ["optionalDependencies", "peerDependencies"] {
                    if let Some(d) = v.get(key).and_then(|d| d.as_object()) {
                        for (k, val) in d {
                            if let Some(s) = val.as_str() {
                                deps.entry(k.clone()).or_insert_with(|| s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    deps
}

/// Result of the local-dependency pre-pass.
#[derive(Default)]
pub struct SpecialDeps {
    /// Non-registry dependencies to record in the lockfile / link afterwards.
    pub specials: Vec<crate::spec_grammar::SpecialDep>,
    /// Registry specs (`name@range`) required by **local/workspace packages**,
    /// which the normal resolution never sees because it only reads the root
    /// manifest.
    pub local_registry_deps: Vec<String>,
}

/// Materialize `package.json` dependency specifiers that are not registry
/// ranges (`npm:` aliases, `file:`/`link:`/`workspace:` locals, git).
///
/// Walks into local and workspace packages and processes *their* dependencies
/// too: a workspace package's `workspace:` dependency must be linked, and its
/// registry dependencies must be resolved, otherwise the package's own
/// `require()` calls fail at runtime.
pub fn materialize_special_deps(project_dir: &Path, node_modules: &Path) -> SpecialDeps {
    let mut out = SpecialDeps::default();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let root_key = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let mut queue: Vec<PathBuf> = vec![root_key.clone()];

    while let Some(dir) = queue.pop() {
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !visited.insert(canon.clone()) {
            continue; // cycle guard (workspace packages can reference each other)
        }
        let is_root = canon == root_key;
        let deps = read_all_deps(&canon);

        if !is_root {
            // Registry deps of a local/workspace package: the flat resolver only
            // reads the root manifest, so add these explicitly.
            for (name, spec) in &deps {
                if matches!(
                    crate::spec_grammar::classify(spec),
                    crate::spec_grammar::Spec::Registry(_)
                ) {
                    let s = format!("{}@{}", name, spec);
                    if !out.local_registry_deps.contains(&s) {
                        out.local_registry_deps.push(s);
                    }
                }
            }
        }

        let mut found = crate::spec_grammar::specials_from_deps(&deps);
        for dep in &mut found {
            if let crate::spec_grammar::SpecialKind::Git {
                url,
                committish,
                dir,
                version,
            } = &mut dep.kind
            {
                if let Some(clone) =
                    crate::spec_grammar::materialize_git_dir(url, committish.as_deref())
                {
                    *version = crate::spec_grammar::dir_version(&clone)
                        .unwrap_or_else(|| "0.0.0".to_string());
                    *dir = Some(clone.clone());
                    if let Err(e) = link_local_dep(node_modules, &dep.name, &clone) {
                        crate::utils::log(&format!("warning: could not link {}: {}", dep.name, e));
                    }
                    // A git checkout's own runtime dependencies must enter the resolution,
                    // exactly as a `file:`/`workspace:` package's do. They were previously
                    // dropped, so `{"rimraf":"github:isaacs/rimraf#v3.0.2"}` installed rimraf
                    // but not its declared `glob` — silently, in 4 of 4 measured runs. Runtime
                    // deps only (`read_package_json_deps` would drag in devDependencies), and
                    // registry specs only: a checkout that itself uses `file:`/`git:` specs is
                    // still not expanded (recorded as a known limit).
                    for (n, r) in runtime_deps(&clone) {
                        if matches!(classify(&r), Spec::Registry(_)) {
                            let s = format!("{}@{}", n, r);
                            if !out.local_registry_deps.contains(&s) {
                                out.local_registry_deps.push(s);
                            }
                        }
                    }
                }
                continue;
            }

            let crate::spec_grammar::SpecialKind::Local { dir, version, link } = &mut dep.kind
            else {
                continue;
            };
            // `workspace:` globs live in the ROOT manifest, so always look them up
            // from there even when we are walking a nested package.
            let target = if dep.spec.starts_with("workspace:") {
                crate::spec_grammar::find_workspace_dir(project_dir, &dep.name)
            } else {
                Some(crate::spec_grammar::resolve_local_dir(&canon, dir))
            };
            let Some(target) = target else {
                crate::utils::log(&format!(
                    "warning: could not locate local dependency {} ({})",
                    dep.name, dep.spec
                ));
                continue;
            };
            let target = target.canonicalize().unwrap_or(target);
            // `file:../pkg-1.0.0.tgz` is a packed package, not a directory: extract
            // it (symlinking the tarball produced a node_modules entry that is a
            // plain file, so every require failed).
            let (target, _is_tarball) =
                if target.is_file() && crate::spec_grammar::is_tarball_path(&target) {
                    match crate::spec_grammar::materialize_local_tarball(&target) {
                        Some(dir) => (dir, true),
                        None => {
                            crate::utils::log(&format!(
                                "warning: could not extract local tarball for {}",
                                dep.name
                            ));
                            continue;
                        }
                    }
                } else {
                    (target, false)
                };
            *version =
                crate::spec_grammar::dir_version(&target).unwrap_or_else(|| "0.0.0".to_string());
            // npm symlinks `file:` directories too (and always `link:`/`workspace:`).
            *link = true;
            *dir = target.clone();
            if let Err(e) = link_local_dep(node_modules, &dep.name, &target) {
                crate::utils::log(&format!("warning: could not link {}: {}", dep.name, e));
            }
            queue.push(target);
        }
        out.specials.extend(found);
    }

    // The same local package can be declared by several manifests — keep one.
    let mut seen: HashSet<String> = HashSet::new();
    out.specials.retain(|d| seen.insert(d.name.clone()));
    out
}

pub(crate) fn link_local_dep(
    node_modules: &Path,
    name: &str,
    target: &Path,
) -> std::io::Result<()> {
    let link_path = node_modules.join(name);
    if let Some(parent) = link_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path(&link_path)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &link_path)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, &link_path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, &link_path);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks unsupported on this platform",
        ))
    }
}

pub(crate) fn remove_path(p: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod local_tarball_tests {

    use super::*;
    use tempfile::tempdir;

    /// npm packs a package with everything under `package/`; the extractor must
    /// strip that level or `node_modules/<name>/package.json` ends up one level too
    /// deep. Symlinking the `.tgz` itself (the old behaviour) produced a plain file
    /// that no `require` could load.
    #[test]
    fn extracts_stripping_the_package_dir_and_caches() {
        let td = tempdir().unwrap();
        let tgz = td.path().join("pkg-1.0.0.tgz");
        {
            let f = std::fs::File::create(&tgz).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut b = tar::Builder::new(enc);
            let pj = br#"{"name":"pkg","version":"1.0.0"}"#;
            let mut h = tar::Header::new_gnu();
            h.set_size(pj.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "package/package.json", &pj[..])
                .unwrap();
            let js = b"module.exports=1;";
            let mut h2 = tar::Header::new_gnu();
            h2.set_size(js.len() as u64);
            h2.set_mode(0o644);
            h2.set_cksum();
            b.append_data(&mut h2, "package/index.js", &js[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }

        let cache = td.path().join("cache");
        let dir = materialize_local_tarball_into(&tgz, &cache).expect("extracted");
        assert!(
            dir.join("package.json").exists(),
            "package/ must be stripped"
        );
        assert!(dir.join("index.js").exists());
        assert_eq!(dir_version(&dir).as_deref(), Some("1.0.0"));

        // Cached by path+size+mtime: the same directory comes back.
        let again = materialize_local_tarball_into(&tgz, &cache).unwrap();
        assert_eq!(dir, again);
    }

    #[test]
    fn recognises_tarball_paths() {
        assert!(is_tarball_path(Path::new("x/pkg-1.0.0.tgz")));
        assert!(is_tarball_path(Path::new("pkg.tar.gz")));
        assert!(is_tarball_path(Path::new("PKG.TGZ")));
        assert!(!is_tarball_path(Path::new("packages/lib")));
        assert!(!is_tarball_path(Path::new("pkg.tgz.js")));
    }

    /// `list_workspace_roots` returns paths *relative to the project*; looking them
    /// up relative to the process cwd silently found nothing outside the CLI case.
    #[test]
    fn finds_workspace_packages_by_name_regardless_of_cwd() {
        let td = tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("packages/x")).unwrap();
        std::fs::write(
            td.path().join("package.json"),
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        std::fs::write(
            td.path().join("packages/x/package.json"),
            r#"{"name":"x","version":"3.0.0"}"#,
        )
        .unwrap();

        // Note: the process cwd is the repo root here, not `td`.
        let found = find_workspace_dir(td.path(), "x").expect("workspace x");
        assert!(found.ends_with("packages/x"), "found: {}", found.display());
        assert_eq!(dir_version(&found).as_deref(), Some("3.0.0"));
        assert!(find_workspace_dir(td.path(), "nope").is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_registry_ranges() {
        for spec in ["^1.2.3", "1.2.3", "~0.4", "*", "latest", ">=1 <2", "1.x"] {
            assert_eq!(classify(spec), Spec::Registry(spec.to_string()), "{}", spec);
        }
    }

    #[test]
    fn classifies_npm_alias() {
        assert_eq!(
            classify("npm:lodash@4.17.21"),
            Spec::Alias {
                real: "lodash".into(),
                range: "4.17.21".into()
            }
        );
        assert_eq!(
            classify("npm:@scope/pkg@^2.0.0"),
            Spec::Alias {
                real: "@scope/pkg".into(),
                range: "^2.0.0".into()
            }
        );
        // No range defaults to `*`.
        assert_eq!(
            classify("npm:lodash"),
            Spec::Alias {
                real: "lodash".into(),
                range: "*".into()
            }
        );
    }

    #[test]
    fn classifies_local_specs() {
        assert_eq!(
            classify("file:../local-pkg"),
            Spec::Local {
                path: PathBuf::from("../local-pkg"),
                link: false
            }
        );
        assert_eq!(
            classify("link:./lib"),
            Spec::Local {
                path: PathBuf::from("./lib"),
                link: true
            }
        );
        assert!(classify("file:../x").is_local());
        assert_eq!(
            classify("workspace:*"),
            Spec::Workspace { range: "*".into() }
        );
        assert_eq!(
            classify("workspace:^1.0.0"),
            Spec::Workspace {
                range: "^1.0.0".into()
            }
        );
        assert!(classify("workspace:*").is_local());
    }

    #[test]
    fn classifies_git_specs() {
        assert_eq!(
            classify("git+https://github.com/u/r.git#main"),
            Spec::Git {
                url: "https://github.com/u/r.git".into(),
                committish: Some("main".into())
            }
        );
        assert_eq!(
            classify("github:u/r"),
            Spec::Git {
                url: "https://github.com/u/r".into(),
                committish: None
            }
        );
        assert_eq!(
            classify("u/r#v1.2.0"),
            Spec::Git {
                url: "https://github.com/u/r".into(),
                committish: Some("v1.2.0".into())
            }
        );
    }

    #[test]
    fn does_not_misclassify_paths_or_ranges() {
        // A relative path without a scheme is a range-ish string, not git.
        assert!(!classify("./local").is_special());
        assert!(!classify("^1.0.0").is_special());
        // Scoped names never look like `owner/repo`.
        assert!(!classify("@scope/pkg").is_special());
        assert!(!classify("@scope/pkg@^1").is_special());
    }
}

#[cfg(test)]
mod archive_containment_tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn octal(field: &mut [u8], value: u64) {
        let s = format!("{:0width$o}", value, width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[s.len()] = 0;
    }

    fn entry(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], 0o644);
        octal(&mut h[124..136], payload.len() as u64);
        h[148..156].copy_from_slice(b"        ");
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().map(|b| *b as u64).sum();
        octal(&mut h[148..156], sum);
        let mut out = h.to_vec();
        out.extend_from_slice(payload);
        out.resize(512 + payload.len().div_ceil(512) * 512, 0);
        out
    }

    fn write_tgz(path: &Path, entries: &[Vec<u8>]) {
        let mut bytes = Vec::new();
        for e in entries {
            bytes.extend_from_slice(e);
        }
        bytes.extend_from_slice(&[0u8; 1024]);
        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&bytes).unwrap();
        std::fs::write(path, gz.finish().unwrap()).unwrap();
    }

    /// A `file:` tarball with a traversal entry must not write outside the cache,
    /// and must be refused as a whole rather than partially trusted.
    #[test]
    fn local_tarball_traversal_is_refused() {
        let td = tempfile::tempdir().unwrap();
        let cache = td.path().join("cache");
        let tgz = td.path().join("local-1.0.0.tgz");
        let escaped = td.path().join("escaped.txt");
        write_tgz(
            &tgz,
            &[
                entry("package/package.json", br#"{"name":"local"}"#),
                entry("package/../../escaped.txt", b"pwned"),
            ],
        );
        let result = materialize_local_tarball_into(&tgz, &cache);
        assert!(result.is_none(), "traversal archive should be refused");
        assert!(
            !escaped.exists(),
            "local tarball escaped the cache and wrote {}",
            escaped.display()
        );
    }

    /// Benign control: an ordinary nested tarball still extracts with `package/`
    /// stripped, proving the guard does not reject valid archives.
    #[test]
    fn local_tarball_benign_archive_still_extracts() {
        let td = tempfile::tempdir().unwrap();
        let cache = td.path().join("cache");
        let tgz = td.path().join("good-1.0.0.tgz");
        write_tgz(
            &tgz,
            &[
                entry("package/package.json", br#"{"name":"good"}"#),
                entry("package/lib/index.js", b"module.exports = 42;"),
            ],
        );
        let dir = materialize_local_tarball_into(&tgz, &cache).expect("extracted");
        assert!(dir.join("package.json").exists());
        assert_eq!(
            std::fs::read(dir.join("lib/index.js")).unwrap(),
            b"module.exports = 42;"
        );
    }
}
