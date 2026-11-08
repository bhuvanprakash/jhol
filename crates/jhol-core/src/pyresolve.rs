//! Resolving a Python dependency graph against this machine's axes.
//!
//! This is the other half of `axes`: `jhol wheels` says whether *one* release fits;
//! this says what tree fits — following `requires_dist`, evaluating environment
//! markers, and choosing releases that actually publish a wheel for this host.
//!
//! Deliberately not a pip reimplementation: no sdist building, no legacy `setup.py`,
//! no entry-point resolution. It answers "which published artifacts form a working
//! dependency set here", which is the question that decides whether an environment can
//! exist at all. Building is a separate concern, and belongs to uv/pip.
//!
//! Failures name the package, the requirement, and the reason, because a resolver that
//! says "conflict" makes the user do the work the resolver just did.

use crate::axes::HostAxes;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Where release metadata comes from. A trait so the resolver can be tested without a
/// network and so a cached provider can be swapped in later.
pub trait Provider {
    /// Files published for one release, plus its `requires_dist`.
    fn release(&mut self, name: &str, version: Option<&str>) -> Result<ReleaseMeta, String>;
    /// Published versions, newest first.
    fn versions(&mut self, name: &str) -> Result<Vec<String>, String>;
}

/// One downloadable artifact of a release.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactFile {
    pub filename: String,
    pub url: String,
    /// SHA-256 published by the index; empty when the index did not provide one.
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseMeta {
    pub version: String,
    /// PEP 508 strings from `requires_dist`.
    pub requires_dist: Vec<String>,
    /// Wheel filenames published for this release (derived from `artifacts`).
    pub files: Vec<String>,
    /// The same artifacts with URL and digest, for installing what was resolved.
    pub artifacts: Vec<ArtifactFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// name -> (version, chosen artifact when one is known)
    pub packages: BTreeMap<String, (String, Option<String>)>,
    /// requirements deliberately skipped because their marker does not apply here
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub name: String,
    pub extras: Vec<String>,
    pub specifiers: Vec<(String, String)>,
    /// Raw marker text, kept for messages.
    pub marker: Option<String>,
}

impl Requirement {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (body, marker) = match raw.split_once(';') {
            Some((b, m)) => (b.trim(), Some(m.trim().to_string())),
            None => (raw.trim(), None),
        };
        // name[extras] then specifiers
        let (name_part, spec_part) = match body.find(['<', '>', '=', '!', '~']) {
            Some(i) => (&body[..i], &body[i..]),
            None => (body, ""),
        };
        let name_part = name_part.trim();
        let (name, extras) = match name_part.split_once('[') {
            Some((n, rest)) => (
                n.trim().to_string(),
                rest.trim_end_matches(']')
                    .split(',')
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .collect(),
            ),
            None => (name_part.to_string(), Vec::new()),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
            return Err(format!("invalid requirement: {raw}"));
        }
        let mut specifiers = Vec::new();
        for part in spec_part.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let op_len = part
                .char_indices()
                .find(|(_, c)| c.is_ascii_alphanumeric())
                .map(|(i, _)| i)
                .unwrap_or(0);
            if op_len == 0 {
                return Err(format!("invalid specifier in {raw}"));
            }
            let (op, ver) = part.split_at(op_len);
            specifiers.push((op.trim().to_string(), ver.trim().to_string()));
        }
        Ok(Requirement {
            name,
            extras,
            specifiers,
            marker,
        })
    }

    /// Does this requirement's marker hold on `host`?
    ///
    /// Markers are evaluated, not guessed: a `sys_platform == "win32"` dependency is
    /// excluded on Linux instead of being installed and failing later. Extras are
    /// treated as absent, which is what makes `extra == "test"` dependencies drop out.
    pub fn applies(&self, host: &HostAxes) -> Result<bool, String> {
        match &self.marker {
            None => Ok(true),
            Some(m) => eval_marker(m, host),
        }
    }
}

/// A small PEP 508 marker evaluator: comparisons, `and`/`or`, parentheses.
fn eval_marker(raw: &str, host: &HostAxes) -> Result<bool, String> {
    // Split on top-level `or` first (lower precedence), then `and`.
    fn split_top<'a>(s: &'a str, kw: &str) -> Option<Vec<&'a str>> {
        let bytes = s.as_bytes();
        let mut depth = 0i32;
        let mut parts = Vec::new();
        let mut last = 0usize;
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth == 0 && s[i..].starts_with(kw) {
                parts.push(&s[last..i]);
                i += kw.len();
                last = i;
                continue;
            }
            i += 1;
        }
        if parts.is_empty() {
            None
        } else {
            parts.push(&s[last..]);
            Some(parts)
        }
    }

    let t = raw.trim();
    let t = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(t).trim();
    if t.is_empty() {
        return Ok(true);
    }
    if let Some(parts) = split_top(t, " or ") {
        for p in parts {
            if eval_marker(p, host)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(parts) = split_top(t, " and ") {
        for p in parts {
            if !eval_marker(p, host)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    // leaf: <var> <op> "<value>" | in | not in
    let (var, op, value) = {
        let mut found = None;
        for op in [" not in ", " in ", "==", "!=", ">=", "<=", ">", "<"] {
            if let Some(i) = t.find(op) {
                let (l, r) = t.split_at(i);
                found = Some((l.trim(), op.trim(), r[op.len()..].trim()));
                break;
            }
        }
        found.ok_or_else(|| format!("unparsed marker: {t}"))?
    };
    let value = value.trim().trim_matches('"').trim_matches('\'');
    let actual: Option<String> = match var {
        "extra" => Some(String::new()),
        "python_version" => host.python.clone(),
        "python_full_version" => host.python.clone(),
        "sys_platform" => Some(match host.os.as_str() {
            "macos" => "darwin".into(),
            "windows" => "win32".into(),
            other => other.to_string(),
        }),
        "platform_machine" => Some(match host.arch.as_str() {
            "arm64" => "arm64".to_string(),
            other => other.to_string(),
        }),
        "platform_system" => Some(match host.os.as_str() {
            "macos" => "Darwin".into(),
            "linux" => "Linux".into(),
            "windows" => "Windows".into(),
            other => other.to_string(),
        }),
        "os_name" => Some(if host.os == "windows" { "nt".into() } else { "posix".into() }),
        "platform_python_implementation" | "implementation_name" => Some(
            std::env::var("JHOL_PYTHON_IMPL").unwrap_or_else(|_| "cpython".to_string()),
        ),
        "implementation_version" => host.python.clone(),
        // Known PEP 508 variables whose exact value we do not claim to know. A marker
        // built on them is treated as *not applying*: skipping a requirement is
        // recoverable, installing the wrong one is not. The skip is recorded.
        "platform_release" | "platform_version" | "platform_dist" | "platform_uname"
        | "platform_arch" => return Ok(false),
        other => return Err(format!("unsupported marker variable: {other}")),
    };
    let Some(actual) = actual else {
        // Unknown host fact (e.g. python version not detected): a marker that cannot
        // be evaluated is treated as not applying, which keeps the graph smaller
        // rather than larger. Recorded by the caller as a skipped requirement.
        return Ok(false);
    };
    Ok(match op {
        "==" => version_or_text_eq(&actual, value),
        "!=" => !version_or_text_eq(&actual, value),
        ">=" => cmp_loose(&actual, value) >= 0,
        ">" => cmp_loose(&actual, value) > 0,
        "<=" => cmp_loose(&actual, value) <= 0,
        "<" => cmp_loose(&actual, value) < 0,
        "in" => actual.contains(value),
        "not in" => !actual.contains(value),
        _ => return Err(format!("unsupported marker operator: {op}")),
    })
}

fn version_or_text_eq(a: &str, b: &str) -> bool {
    if a.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
        && b.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
    {
        return cmp_loose(a, b) == 0;
    }
    a.eq_ignore_ascii_case(b)
}

/// Compare two versions as dot-separated numbers, padding with zeros. `3.12` vs `3.9`
/// must be 3.12 > 3.9 — string comparison gets that wrong and rejects correct wheels'
/// markers.
fn cmp_loose(a: &str, b: &str) -> i32 {
    fn parts(v: &str) -> Vec<u64> {
        v.split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect()
    }
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return if x < y { -1 } else { 1 };
        }
    }
    0
}

/// Does `version` satisfy the requirement's specifiers?
pub fn satisfies(specifiers: &[(String, String)], version: &str) -> bool {
    specifiers.iter().all(|(op, want)| match op.as_str() {
        "==" => {
            if let Some(prefix) = want.strip_suffix(".*") {
                version_matches_prefix(version, prefix)
            } else {
                cmp_loose(version, want) == 0
            }
        }
        "!=" => {
            if let Some(prefix) = want.strip_suffix(".*") {
                !version_matches_prefix(version, prefix)
            } else {
                cmp_loose(version, want) != 0
            }
        }
        ">=" => cmp_loose(version, want) >= 0,
        ">" => cmp_loose(version, want) > 0,
        "<=" => cmp_loose(version, want) <= 0,
        "<" => cmp_loose(version, want) < 0,
        "~=" => {
            // compatible release: >= want, same prefix as want minus last component
            if cmp_loose(version, want) < 0 {
                return false;
            }
            let comps: Vec<&str> = want.split('.').collect();
            let prefix = comps[..comps.len().saturating_sub(1)].join(".");
            version_matches_prefix(version, &prefix)
        }
        _ => false,
    })
}

/// Is this a pre-release (`1.0.0a1`, `3.7rc0`, `2.0.0.dev3`)?
fn is_prerelease(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    let core_end = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(v.len());
    let tail = &v[core_end..];
    tail.contains("rc") || tail.contains("alpha") || tail.contains("beta") || tail.contains("dev")
        || tail.starts_with('a') || tail.starts_with('b')
}

/// A specifier that explicitly asks for a pre-release opts the whole requirement back in
/// (PEP 440: `==1.0rc1` means the user knows).
fn specifier_names_prerelease(specs: &[(String, String)]) -> bool {
    specs.iter().any(|(_, v)| is_prerelease(v))
}

fn version_matches_prefix(version: &str, prefix: &str) -> bool {
    let v: Vec<&str> = version.split('.').collect();
    let p: Vec<&str> = prefix.split('.').collect();
    p.len() <= v.len() && p.iter().zip(v.iter()).all(|(a, b)| a == b)
}

/// Options for one resolution.
#[derive(Debug, Clone)]
pub struct Options {
    /// Accept the newest version with a **compatible wheel**; otherwise require one.
    pub require_wheel: bool,
    /// How deep to follow transitive dependencies.
    pub max_depth: usize,
    /// Python version to assume (overrides detection).
    pub python: Option<String>,
    /// Allow pre-releases (`rc`, `a`, `b`, `dev`). Off by default: a resolver that
    /// silently prefers `3.7rc0` over `3.6.1` is making a stability decision on the
    /// user's behalf.
    pub allow_prerelease: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            require_wheel: true,
            max_depth: 64,
            python: None,
            allow_prerelease: false,
        }
    }
}

/// Resolve `roots` into a set of releases whose artifacts can run here.
pub fn resolve<P: Provider>(
    provider: &mut P,
    roots: &[(String, String)],
    host: &HostAxes,
    options: &Options,
) -> Result<Resolved, String> {
    let mut packages: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut queue: Vec<(Requirement, usize, String)> = Vec::new();

    for (name, spec) in roots {
        let raw = if spec.trim().is_empty() || spec.trim() == "*" {
            name.clone()
        } else {
            format!("{name}{spec}")
        };
        queue.push((Requirement::parse(&raw)?, 0, "<root>".to_string()));
    }

    let mut constraints: HashMap<String, Vec<(String, String)>> = HashMap::new();
    while let Some((req, depth, from)) = queue.pop() {
        if depth > options.max_depth {
            return Err(format!(
                "dependency graph deeper than {} levels at {}",
                options.max_depth, req.name
            ));
        }
        if !req.applies(host)? {
            skipped.push(format!("{} (from {from})", req.name));
            continue;
        }
        constraints
            .entry(req.name.clone())
            .or_default()
            .extend(req.specifiers.iter().cloned());

        // Already chosen: check the new constraint against it.
        if let Some((chosen, _)) = packages.get(&req.name) {
            let all: Vec<(String, String)> = constraints[&req.name].clone();
            if satisfies(&all, chosen) {
                continue;
            }
            return Err(format!(
                "{} {} is required by {}, but {} is already selected",
                req.name,
                render_specs(&req.specifiers),
                from,
                chosen
            ));
        }

        let versions = provider.versions(&req.name)?;
        let all: Vec<(String, String)> = constraints[&req.name].clone();
        let mut chosen: Option<(String, Option<String>, ReleaseMeta)> = None;
        let mut rejections: Vec<String> = Vec::new();
        for v in &versions {
            if !satisfies(&all, v) {
                continue;
            }
            if !options.allow_prerelease && is_prerelease(v) && !specifier_names_prerelease(&all) {
                continue;
            }
            let meta = match provider.release(&req.name, Some(v)) {
                Ok(m) => m,
                Err(e) => {
                    rejections.push(format!("{v}: {e}"));
                    continue;
                }
            };
            let artifact = best_artifact(meta.files.as_slice(), host);
            if options.require_wheel && artifact.is_none() {
                rejections.push(format!(
                    "{v}: no artifact runs here ({})",
                    meta.files.first().cloned().unwrap_or_else(|| "no files".into())
                ));
                continue;
            }
            chosen = Some((v.clone(), artifact, meta));
            break;
        }
        let Some((version, artifact, meta)) = chosen else {
            let mut msg = format!(
                "no version of {} satisfies {}{}",
                req.name,
                render_specs(&all),
                if from == "<root>" { String::new() } else { format!(" (required by {from})") }
            );
            if !rejections.is_empty() {
                msg.push_str(&format!("; {}", rejections.iter().take(3).cloned().collect::<Vec<_>>().join("; ")));
            }
            return Err(msg);
        };
        packages.insert(req.name.clone(), (version.clone(), artifact));
        for dep in &meta.requires_dist {
            match Requirement::parse(dep) {
                Ok(r) => queue.push((r, depth + 1, format!("{} {}", req.name, version))),
                Err(e) => skipped.push(format!("{}: {e}", dep)),
            }
        }
    }
    Ok(Resolved { packages, skipped })
}

fn render_specs(specs: &[(String, String)]) -> String {
    if specs.is_empty() {
        return "(any version)".to_string();
    }
    specs
        .iter()
        .map(|(o, v)| format!("{o}{v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Newest artifact that fits this host, preferring wheels over sdists.
fn best_artifact(files: &[String], host: &HostAxes) -> Option<String> {
    let mut compatible: Vec<(bool, String)> = files
        .iter()
        .filter_map(|f| match crate::axes::WheelTag::parse(f) {
            Some(tag) => crate::axes::wheel_fits(&tag, host).ok().map(|_| (true, f.clone())),
            None => None,
        })
        .collect();
    compatible.sort_by(|a, b| b.1.cmp(&a.1));
    compatible.into_iter().next().map(|(_, f)| f)
}

/// A provider backed by the PyPI JSON API.
pub struct PyPIProvider {
    cache: HashMap<(String, Option<String>), ReleaseMeta>,
    versions: HashMap<String, Vec<String>>,
}

impl PyPIProvider {
    pub fn new() -> Self {
        PyPIProvider {
            cache: HashMap::new(),
            versions: HashMap::new(),
        }
    }
}

impl Default for PyPIProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for PyPIProvider {
    fn release(&mut self, name: &str, version: Option<&str>) -> Result<ReleaseMeta, String> {
        let key = (name.to_string(), version.map(|v| v.to_string()));
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit.clone());
        }
        let url = match version {
            Some(v) => format!("https://pypi.org/pypi/{name}/{v}/json"),
            None => format!("https://pypi.org/pypi/{name}/json"),
        };
        let body = crate::http_client::get(&url)?;
        let doc: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let info = doc.get("info").cloned().unwrap_or_default();
        let meta = ReleaseMeta {
            version: info
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            requires_dist: info
                .get("requires_dist")
                .and_then(|r| r.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default(),
            files: Vec::new(),
            artifacts: doc
                .get("urls")
                .and_then(|u| u.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|f| {
                            Some(ArtifactFile {
                                filename: f.get("filename")?.as_str()?.to_string(),
                                url: f.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
                                sha256: f
                                    .get("digests")
                                    .and_then(|d| d.get("sha256"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                size: f.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        let mut meta = meta;
        meta.files = meta.artifacts.iter().map(|a| a.filename.clone()).collect();
        self.cache.insert(key, meta.clone());
        Ok(meta)
    }

    fn versions(&mut self, name: &str) -> Result<Vec<String>, String> {
        if let Some(hit) = self.versions.get(name) {
            return Ok(hit.clone());
        }
        let body = crate::http_client::get(&format!("https://pypi.org/pypi/{name}/json"))?;
        let doc: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let mut list: Vec<String> = doc
            .get("releases")
            .and_then(|r| r.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        // Newest first; pre-releases sorted below releases of the same number.
        list.sort_by(|a, b| cmp_loose(b, a).cmp(&0).then_with(|| b.cmp(a)));
        self.versions.insert(name.to_string(), list.clone());
        Ok(list)
    }
}

/// Where a resolved tree would be written (not implemented yet — resolution only).
pub fn plan_root(_dir: &std::path::Path) -> PathBuf {
    PathBuf::from(".jhol/py")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::Libc;

    struct Fake {
        releases: HashMap<String, Vec<ReleaseMeta>>,
    }

    impl Provider for Fake {
        fn release(&mut self, name: &str, version: Option<&str>) -> Result<ReleaseMeta, String> {
            let list = self.releases.get(name).ok_or_else(|| format!("unknown {name}"))?;
            match version {
                Some(v) => list.iter().find(|m| m.version == v).cloned().ok_or_else(|| format!("no {name} {v}")),
                None => list.first().cloned().ok_or_else(|| format!("no {name}")),
            }
        }
        fn versions(&mut self, name: &str) -> Result<Vec<String>, String> {
            let list = self.releases.get(name).ok_or_else(|| format!("unknown {name}"))?;
            Ok(list.iter().map(|m| m.version.clone()).collect())
        }
    }

    fn host(py: &str) -> HostAxes {
        HostAxes {
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: Libc::Glibc { major: 2, minor: 35 },
            python: Some(py.into()),
            cuda_driver: None,
            sm: Vec::new(),
            gpu_mem_mb: None,
        }
    }

    fn wheel(name: &str, ver: &str, py: &str) -> String {
        format!("{name}-{ver}-cp{}-cp{}-manylinux_2_17_x86_64.whl", py.replace('.', ""), py.replace('.', ""))
    }

    fn meta(name: &str, ver: &str, py: &str, deps: &[&str]) -> ReleaseMeta {
        let filename = wheel(name, ver, py);
        ReleaseMeta {
            version: ver.into(),
            requires_dist: deps.iter().map(|d| d.to_string()).collect(),
            files: vec![filename.clone()],
            artifacts: vec![ArtifactFile {
                filename,
                url: String::new(),
                sha256: String::new(),
                size: 0,
            }],
        }
    }

    fn provider(releases: Vec<(&str, Vec<ReleaseMeta>)>) -> Fake {
        Fake {
            releases: releases.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    #[test]
    fn parses_requirements_and_specifiers() {
        let r = Requirement::parse("numpy[extra1,extra2] >=1.21,<3 ; python_version >= \"3.9\"").unwrap();
        assert_eq!(r.name, "numpy");
        assert_eq!(r.extras, vec!["extra1", "extra2"]);
        assert_eq!(r.specifiers.len(), 2);
        assert!(r.marker.is_some());
        assert!(Requirement::parse("").is_err());
        assert!(satisfies(&[(">=".into(), "1.21".into())], "2.0.0"));
        assert!(!satisfies(&[(">=".into(), "1.21".into())], "1.20.9"));
        assert!(satisfies(&[("~=".into(), "2.2".into())], "2.2.9"));
        assert!(!satisfies(&[("~=".into(), "2.2".into())], "3.0"));
        assert!(satisfies(&[("==".into(), "1.*".into())], "1.9.0"));
    }

    #[test]
    fn markers_are_evaluated_not_guessed() {
        let h = host("3.12");
        let win = Requirement::parse("pywin32; sys_platform == \"win32\"").unwrap();
        assert!(!win.applies(&h).unwrap());
        let linux = Requirement::parse("uvloop; sys_platform == \"linux\"").unwrap();
        assert!(linux.applies(&h).unwrap());
        let extra = Requirement::parse("pytest; extra == \"test\"").unwrap();
        assert!(!extra.applies(&h).unwrap(), "extras are not being installed");
        let old_py = Requirement::parse("x; python_version < \"3.10\"").unwrap();
        assert!(!old_py.applies(&h).unwrap());
        // 3.12 vs 3.9 must compare numerically, not as strings.
        let py39 = Requirement::parse("x; python_version > \"3.9\"").unwrap();
        assert!(py39.applies(&h).unwrap());
    }

    #[test]
    fn resolves_transitively_and_picks_newest_compatible() {
        let mut p = provider(vec![
            (
                "requests",
                vec![
                    meta("requests", "2.32.0", "3.12", &["urllib3>=1.21"]),
                    meta("requests", "2.31.0", "3.9", &[]),
                ],
            ),
            (
                "urllib3",
                vec![
                    meta("urllib3", "2.5.0", "3.12", &[]),
                    meta("urllib3", "1.26.0", "3.9", &[]),
                ],
            ),
        ]);
        let out = resolve(&mut p, &[("requests".into(), "*".into())], &host("3.12"), &Options::default()).unwrap();
        assert_eq!(out.packages["requests"].0, "2.32.0");
        assert_eq!(out.packages["urllib3"].0, "2.5.0");
    }

    #[test]
    fn skips_versions_without_a_wheel_for_this_python() {
        // Newest only publishes a cp311 wheel; on 3.12 it must fall back.
        let mut p = provider(vec![(
            "numpy",
            vec![meta("numpy", "2.1.0", "3.11", &[]), meta("numpy", "2.0.0", "3.12", &[])],
        )]);
        let out = resolve(&mut p, &[("numpy".into(), ">=1".into())], &host("3.12"), &Options::default()).unwrap();
        assert_eq!(out.packages["numpy"].0, "2.0.0", "the cp311 wheel cannot run on 3.12");
    }

    /// Pre-releases must not win by being "newest": 3.7rc0 is not newer than 3.6.1 for
    /// anyone who did not ask for it.
    #[test]
    fn prereleases_are_excluded_by_default() {
        let mut p = provider(vec![(
            "net",
            vec![meta("net", "3.7rc0", "3.12", &[]), meta("net", "3.6.1", "3.12", &[])],
        )]);
        let out = resolve(&mut p, &[("net".into(), "*".into())], &host("3.12"), &Options::default()).unwrap();
        assert_eq!(out.packages["net"].0, "3.6.1");
        // And opting in gets the rc.
        let opts = Options {
            allow_prerelease: true,
            ..Default::default()
        };
        let out = resolve(&mut p, &[("net".into(), "*".into())], &host("3.12"), &opts).unwrap();
        assert_eq!(out.packages["net"].0, "3.7rc0");
        assert!(is_prerelease("1.0.0a1") && is_prerelease("2.0.0.dev3"));
        assert!(!is_prerelease("1.0.0") && !is_prerelease("2024.10.22"));
    }

    #[test]
    fn explicit_prerelease_specifier_opts_in() {
        let mut p = provider(vec![(
            "x",
            vec![meta("x", "1.0rc1", "3.12", &[]), meta("x", "0.9", "3.12", &[])],
        )]);
        let out = resolve(&mut p, &[("x".into(), "==1.0rc1".into())], &host("3.12"), &Options::default()).unwrap();
        assert_eq!(out.packages["x"].0, "1.0rc1");
    }

    #[test]
    fn conflicts_name_both_sides() {
        let mut p = provider(vec![
            ("a", vec![meta("a", "1.0.0", "3.12", &["c==1.0.0"])]),
            ("b", vec![meta("b", "1.0.0", "3.12", &["c==2.0.0"])]),
            ("c", vec![meta("c", "1.0.0", "3.12", &[]), meta("c", "2.0.0", "3.12", &[])]),
        ]);
        let err = resolve(
            &mut p,
            &[("a".into(), "*".into()), ("b".into(), "*".into())],
            &host("3.12"),
            &Options::default(),
        )
        .unwrap_err();
        assert!(err.contains("c") && err.contains("already selected"), "{err}");
    }

    #[test]
    fn unsatisfiable_requirement_lists_rejections() {
        let mut p = provider(vec![("x", vec![meta("x", "1.0.0", "3.11", &[])])]);
        let err = resolve(&mut p, &[("x".into(), ">=1".into())], &host("3.12"), &Options::default())
            .unwrap_err();
        assert!(err.contains("no version of x"), "{err}");
        assert!(err.contains("no artifact runs here"), "{err}");
    }
}
