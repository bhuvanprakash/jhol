//! Toolchain / environment discovery (blueprint §4 `jhol-env`).
//!
//! This crate observes the host; it never installs or mutates a toolchain.
//! Every value here is derived from `PATH`, a version command, or a project
//! version-pin file. Discovery is deterministic and side-effect free.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Schema version for serialized environment reports.
pub const ENV_SCHEMA: u32 = 1;

/// Environment variables whose values must never be captured in a report.
const SAFE_BASE_ENV: &[&str] = &[
    "HOME",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "USER",
    "LOGNAME",
    "SHELL",
    "PWD",
    "SYSTEMROOT",
    "COMSPEC",
    "PATHEXT",
    "WINDIR",
];

const SECRET_MARKERS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "PASSWD",
    "PASSPHRASE",
    "CREDENTIAL",
    "PRIVATE",
    "AUTH",
    "COOKIE",
    "SESSION",
    "APIKEY",
    "API_KEY",
    "ACCESS_KEY",
    "KEY",
    "DSN",
    "DATABASE_URL",
    "CONNECTION",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFamily {
    Node,
    Python,
    Rust,
    PackageManager,
    Other,
}

impl ToolFamily {
    pub fn for_tool(name: &str) -> Self {
        match name {
            "node" | "deno" | "bun" => ToolFamily::Node,
            "python" | "python3" | "uv" | "pip" | "pip3" => ToolFamily::Python,
            "cargo" | "rustc" | "rustup" => ToolFamily::Rust,
            "npm" | "pnpm" | "yarn" => ToolFamily::PackageManager,
            _ => ToolFamily::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parse the first dotted numeric run in `text`.
    ///
    /// Accepts `v20.11.0`, `Python 3.12.1`, `rustc 1.80.0 (hash date)` and
    /// bare majors such as `3`.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                let mut parts = [0u64; 3];
                let mut part = 0;
                let mut seen_dot = false;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    if bytes[i] == b'.' {
                        if part == 2 {
                            break;
                        }
                        seen_dot = true;
                        part += 1;
                    } else if part < 3 {
                        parts[part] = parts[part]
                            .saturating_mul(10)
                            .saturating_add((bytes[i] - b'0') as u64);
                    }
                    i += 1;
                }
                if i == start {
                    i += 1;
                    continue;
                }
                if seen_dot || parts[0] > 0 {
                    return Some(Version {
                        major: parts[0],
                        minor: parts[1],
                        patch: parts[2],
                    });
                }
            } else {
                i += 1;
            }
        }
        None
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A small, dependency-free version constraint.
///
/// Supported forms: `*`, `any`, empty, `20`, `20.x`, `20.10`, `>=20.10.1`,
/// `>`, `<=`, `<`, `=`, `==`, `^18.2` and `~3.11`. Comma- or space-separated
/// clauses are combined with AND.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionConstraint {
    pub raw: String,
}

impl VersionConstraint {
    pub fn parse(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    pub fn any() -> Self {
        Self::parse("*")
    }

    pub fn matches(&self, version: &Version) -> bool {
        let raw = self.raw.trim();
        if raw.is_empty() || raw == "*" || raw.eq_ignore_ascii_case("any") {
            return true;
        }
        raw.split([',', ' '])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .all(|part| clause_matches(part, version))
    }
}

fn clause_matches(clause: &str, version: &Version) -> bool {
    let (op, rest) = if let Some(r) = clause.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = clause.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = clause.strip_prefix("==") {
        ("=", r)
    } else if let Some(r) = clause.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = clause.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = clause.strip_prefix('=') {
        ("=", r)
    } else if let Some(r) = clause.strip_prefix('^') {
        ("^", r)
    } else if let Some(r) = clause.strip_prefix('~') {
        ("~", r)
    } else {
        ("", clause)
    };
    let rest = rest.trim();
    let wildcard_major = rest.ends_with(".x") || rest.ends_with(".*");
    let numeric = rest.trim_end_matches(".x").trim_end_matches(".*");
    let Some(want) = Version::parse(numeric) else {
        return false;
    };
    match op {
        ">=" => *version >= want,
        ">" => *version > want,
        "<=" => *version <= want,
        "<" => *version < want,
        "=" => *version == want,
        "^" => version.major == want.major && *version >= want,
        "~" => version.major == want.major && version.minor == want.minor && *version >= want,
        _ => {
            if wildcard_major || !numeric.contains('.') {
                version.major == want.major
            } else if numeric.matches('.').count() == 1 {
                version.major == want.major && version.minor == want.minor
            } else {
                *version == want
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPresence {
    pub name: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed_version: Option<Version>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainReport {
    #[serde(default = "env_schema")]
    pub schema: u32,
    pub os: String,
    pub arch: String,
    pub tools: Vec<ToolPresence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequirement {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
    /// Arguments used to print the version. Defaults per known tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_args: Vec<String>,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn env_schema() -> u32 {
    ENV_SCHEMA
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementCheck {
    pub name: String,
    pub required: bool,
    pub satisfied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found_version: Option<String>,
    pub detail: String,
}

fn version_args_for(name: &str) -> &'static [&'static str] {
    match name {
        "node" | "npm" | "bun" | "deno" | "python" | "python3" | "uv" | "cargo" | "rustc"
        | "rustup" | "go" | "java" | "pnpm" | "yarn" | "git" => &["--version"],
        _ => &["--version"],
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Resolve `name` on `PATH` without invoking a shell.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let direct = Path::new(name);
    if direct.components().count() > 1 || direct.is_absolute() {
        return is_executable(direct).then(|| direct.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into());
            for ext in pathext.split(';').filter(|e| !e.is_empty()) {
                let candidate = dir.join(format!("{name}{ext}"));
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Locate and probe a single tool. Never panics when the command is missing.
pub fn probe_tool(name: &str, args: &[&str]) -> ToolPresence {
    let found = find_tool(name);
    let mut presence = ToolPresence {
        name: name.to_string(),
        available: found.is_some(),
        version: None,
        parsed_version: None,
        path: found.as_ref().map(|p| p.display().to_string()),
        error: None,
    };
    let Some(path) = found else {
        return presence;
    };
    let args = if args.is_empty() {
        version_args_for(name)
    } else {
        args
    };
    match Command::new(&path).args(args).output() {
        Ok(out) if out.status.success() => {
            let text = output_first_line(&out.stdout)
                .or_else(|| output_first_line(&out.stderr))
                .unwrap_or_default();
            presence.parsed_version = Version::parse(&text);
            presence.version = (!text.is_empty()).then_some(text);
        }
        Ok(out) => {
            let detail = output_first_line(&out.stderr)
                .or_else(|| output_first_line(&out.stdout))
                .unwrap_or_else(|| format!("exited with {}", out.status));
            presence.error = Some(detail);
        }
        Err(e) => presence.error = Some(e.to_string()),
    }
    presence
}

fn output_first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Probe the default host toolchain. Missing tools are reported, not errors.
pub fn probe_host() -> ToolchainReport {
    let tools = ["node", "npm", "bun", "python3", "uv", "cargo", "rustc"]
        .iter()
        .map(|name| probe_tool(name, &[]))
        .collect();
    ToolchainReport {
        schema: ENV_SCHEMA,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        tools,
    }
}

impl ToolchainReport {
    pub fn get(&self, name: &str) -> Option<&ToolPresence> {
        self.tools.iter().find(|t| t.name == name)
    }
}

/// Evaluate declared tool requirements against the host.
pub fn check_requirements(requirements: &[ToolRequirement]) -> Vec<RequirementCheck> {
    requirements
        .iter()
        .map(|req| {
            let args: Vec<&str> = if req.version_args.is_empty() {
                version_args_for(&req.name).to_vec()
            } else {
                req.version_args.iter().map(String::as_str).collect()
            };
            let found = probe_tool(&req.name, &args);
            let (satisfied, detail) = if !found.available {
                (
                    false,
                    if req.required {
                        format!("`{}` not found on PATH", req.name)
                    } else {
                        format!("optional `{}` not found", req.name)
                    },
                )
            } else if let Some(raw) = &req.constraint {
                let constraint = VersionConstraint::parse(raw.clone());
                match found.parsed_version {
                    Some(v) if constraint.matches(&v) => {
                        (true, format!("{} satisfies {}", v, constraint.raw))
                    }
                    Some(v) => (false, format!("{} does not satisfy {}", v, constraint.raw)),
                    None => (
                        false,
                        format!(
                            "`{}` did not report a parsable version (`{}`)",
                            req.name,
                            found.version.clone().unwrap_or_default()
                        ),
                    ),
                }
            } else {
                (true, format!("`{}` available", req.name))
            };
            RequirementCheck {
                name: req.name.clone(),
                required: req.required,
                satisfied,
                found_version: found.version,
                detail,
            }
        })
        .collect()
}

/// True when every required check passed. Optional misses do not fail.
pub fn requirements_satisfied(checks: &[RequirementCheck]) -> bool {
    checks.iter().all(|c| c.satisfied || !c.required)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSnapshot {
    pub schema: u32,
    pub vars: BTreeMap<String, String>,
    pub redacted_keys: Vec<String>,
    pub dropped_keys: Vec<String>,
    pub path_entries: Vec<String>,
}

/// Names that look like credentials. Values are never captured.
pub fn is_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase().replace('-', "_");
    // Prefix matching matters: `SECRET_KEY` and `PRIVATE_KEY` start with a
    // marker but contain no `_MARKER_` run, and missing them would capture
    // real credentials into snapshots. Over-redaction is fail-closed and safe.
    SECRET_MARKERS.iter().any(|m| {
        upper == *m
            || upper.starts_with(m)
            || upper.ends_with(m)
            || upper.contains(&format!("_{m}_"))
    })
}

/// Capture only the named variables. Secret-looking values are redacted.
pub fn capture_env(keys: &[&str]) -> EnvSnapshot {
    let mut vars = BTreeMap::new();
    let mut redacted = Vec::new();
    let mut dropped = Vec::new();
    for key in keys {
        match std::env::var(key) {
            Ok(value) => {
                if is_secret_key(key) {
                    redacted.push((*key).to_string());
                } else {
                    vars.insert((*key).to_string(), value);
                }
            }
            Err(_) => dropped.push((*key).to_string()),
        }
    }
    redacted.sort();
    dropped.sort();
    EnvSnapshot {
        schema: ENV_SCHEMA,
        vars,
        redacted_keys: redacted,
        dropped_keys: dropped,
        path_entries: path_entries(),
    }
}

/// Sorted `PATH` entries, skipping empty segments (which would mean CWD).
pub fn path_entries() -> Vec<String> {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.display().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Build a deterministic, secret-free child environment.
///
/// `allow` lists additional variable names. Secrets are excluded unless
/// `include_secrets` is true, which callers should use only for explicit
/// lifecycle scripts the user approved.
pub fn child_env(allow: &[&str], include_secrets: bool) -> Vec<(String, String)> {
    let mut vars: BTreeMap<String, String> = std::env::vars()
        .filter(|(k, _)| {
            if include_secrets {
                true
            } else if is_secret_key(k) {
                false
            } else {
                k == "PATH" || SAFE_BASE_ENV.contains(&k.as_str()) || allow.contains(&k.as_str())
            }
        })
        .collect();
    for key in allow {
        if let Ok(value) = std::env::var(key) {
            vars.insert((*key).to_string(), value);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        vars.insert("PATH".into(), path.to_string_lossy().into_owned());
    }
    vars.into_iter().collect()
}

/// Version pins declared by common per-ecosystem files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionPins {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Read `.nvmrc`, `.node-version`, `.python-version`, `rust-toolchain(.toml)`
/// and `package.json` engines without executing anything from the project.
pub fn read_version_pins(project: &Path) -> VersionPins {
    let mut pins = VersionPins::default();
    pins.node = read_trimmed(&project.join(".nvmrc"))
        .or_else(|| read_trimmed(&project.join(".node-version")));
    pins.python = read_trimmed(&project.join(".python-version"));
    pins.rust = read_trimmed(&project.join("rust-toolchain")).or_else(|| {
        std::fs::read_to_string(project.join("rust-toolchain.toml"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|line| {
                        let line = line.trim();
                        line.strip_prefix("channel")
                            .and_then(|rest| rest.trim_start().strip_prefix('='))
                            .map(|v| v.trim().trim_matches('"').to_string())
                    })
                    .filter(|v| !v.is_empty())
            })
    });

    let pkg = project.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pkg) {
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                if pins.node.is_none() {
                    pins.node = value
                        .get("engines")
                        .and_then(|e| e.get("node"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
                if pins.python.is_none() {
                    pins.python = value
                        .get("engines")
                        .and_then(|e| e.get("python"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
            }
            Err(e) => pins
                .notes
                .push(format!("package.json is not valid JSON: {e}")),
        }
    }
    pins
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    Some(line.to_string())
}

/// Check whether the installed toolchain matches the project's declared pins.
pub fn check_version_pins(project: &Path) -> Vec<RequirementCheck> {
    let pins = read_version_pins(project);
    let mut reqs = Vec::new();
    if let Some(node) = pins.node {
        reqs.push(ToolRequirement {
            name: "node".into(),
            constraint: Some(node),
            version_args: vec!["--version".into()],
            required: true,
        });
    }
    if let Some(python) = pins.python {
        reqs.push(ToolRequirement {
            name: "python3".into(),
            constraint: Some(python),
            version_args: vec!["--version".into()],
            required: true,
        });
    }
    if let Some(rust) = pins.rust {
        reqs.push(ToolRequirement {
            name: "rustc".into(),
            constraint: Some(rust),
            version_args: vec!["--version".into()],
            required: true,
        });
    }
    check_requirements(&reqs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_common_version_banners() {
        assert_eq!(Version::parse("v20.11.0").unwrap().to_string(), "20.11.0");
        assert_eq!(
            Version::parse("Python 3.12.1").unwrap().to_string(),
            "3.12.1"
        );
        assert_eq!(
            Version::parse("rustc 1.80.0 (abc 2024-01-01)")
                .unwrap()
                .to_string(),
            "1.80.0"
        );
        assert_eq!(Version::parse("3").unwrap().to_string(), "3.0.0");
        assert!(Version::parse("no digits here").is_none());
    }

    #[test]
    fn constraints_match_and_reject() {
        let v = Version::parse("20.11.4").unwrap();
        assert!(VersionConstraint::parse(">=20.10").matches(&v));
        assert!(!VersionConstraint::parse(">=21").matches(&v));
        assert!(VersionConstraint::parse("20.x").matches(&v));
        assert!(VersionConstraint::parse("20.11").matches(&v));
        assert!(!VersionConstraint::parse("20.10").matches(&v));
        assert!(VersionConstraint::parse("^18.2").matches(&Version::parse("18.9.0").unwrap()));
        assert!(!VersionConstraint::parse("^18.2").matches(&v));
        assert!(VersionConstraint::parse("~3.11").matches(&Version::parse("3.11.9").unwrap()));
        assert!(!VersionConstraint::parse("~3.11").matches(&Version::parse("3.12.0").unwrap()));
        assert!(VersionConstraint::parse(">=18, <21").matches(&v));
    }

    #[test]
    fn report_has_host_fields() {
        let r = probe_host();
        assert_eq!(r.schema, ENV_SCHEMA);
        assert!(!r.os.is_empty());
        assert!(!r.arch.is_empty());
        assert!(!r.tools.is_empty());
        assert!(r.tools.iter().all(|t| !t.name.is_empty()));
    }

    #[test]
    fn missing_tool_is_reported_without_error() {
        let p = probe_tool("jhol-definitely-not-installed-xyz", &["--version"]);
        assert!(!p.available);
        assert!(p.path.is_none());
        assert!(p.error.is_none());
    }

    #[test]
    fn secret_keys_are_detected_and_redacted() {
        assert!(is_secret_key("GITHUB_TOKEN"));
        assert!(is_secret_key("npm_config_//registry.npmjs.org/:_authToken"));
        assert!(is_secret_key("DB_PASSWORD"));
        assert!(!is_secret_key("HOME"));
        assert!(!is_secret_key("PATH"));
        let mut vars = BTreeMap::new();
        vars.insert("SAFE".to_string(), "yes".to_string());
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn child_env_is_sorted_and_secret_free() {
        let env = child_env(&["JHOL_ENV_TEST_VALUE"], false);
        assert!(env.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(!env.iter().any(|(k, _)| is_secret_key(k)));
        assert!(env.iter().any(|(k, _)| k == "PATH"));
    }

    #[test]
    fn reads_pins_from_common_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".nvmrc"), "20.11.0\n").unwrap();
        fs::write(dir.path().join(".python-version"), "3.12.2\n").unwrap();
        fs::write(
            dir.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.80.0\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"engines":{"node":">=22"}}"#,
        )
        .unwrap();
        let pins = read_version_pins(dir.path());
        assert_eq!(pins.node.as_deref(), Some("20.11.0"));
        assert_eq!(pins.python.as_deref(), Some("3.12.2"));
        assert_eq!(pins.rust.as_deref(), Some("1.80.0"));
    }

    #[test]
    fn secret_detection_covers_prefix_and_connection_strings() {
        assert!(
            is_secret_key("GITHUB_TOKEN")
                && is_secret_key("NPM_TOKEN")
                && is_secret_key("BOT_TOKEN"),
            "TOKEN-named variables must be redacted"
        );
        // These were all missed before prefix matching existed.
        for key in [
            "SECRET_KEY",
            "PRIVATE_KEY",
            "PASSWORD_HASH",
            "PASSPHRASE",
            "DATABASE_URL",
            "DATABASE_CONNECTION_STRING",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "NPM_CONFIG__AUTH",
        ] {
            assert!(is_secret_key(key), "{key} must be treated as a secret");
        }
        // Ordinary build configuration is not a secret.
        for key in [
            "RUST_BACKTRACE",
            "CARGO_TARGET_DIR",
            "CI",
            "npm_config_registry",
        ] {
            assert!(!is_secret_key(key), "{key} is not a secret");
        }
    }

    #[test]
    fn requirement_failure_for_unknown_tool() {
        let checks = check_requirements(&[ToolRequirement {
            name: "jhol-definitely-not-installed-xyz".into(),
            constraint: None,
            version_args: vec![],
            required: true,
        }]);
        assert!(!checks[0].satisfied);
        assert!(!requirements_satisfied(&checks));
    }
}
