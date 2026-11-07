//! The `packageManager` field (corepack's pin) and duplicate detection.
//!
//! `packageManager: "jhol@2.0.0"` tells every machine which tool owns the
//! install. jhol reads it, reports a mismatch once, and honours an opt-in
//! strict mode (`--pm-strict` / `JHOL_PM_STRICT=1`) that refuses to install
//! with the wrong tool. Silent divergence is the failure mode this prevents:
//! a repo pinned to pnpm installed with jhol produces a different tree.

/// Parsed `packageManager` value: `name@version` (optionally with a hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManager {
    pub name: String,
    pub version: String,
}

/// Parse `"name@version"` / `"name@version+sha..."`.
pub fn parse_field(value: &str) -> Option<PackageManager> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let (name, rest) = v.rsplit_once('@')?;
    if name.is_empty() || rest.is_empty() {
        return None;
    }
    let version = rest.split('+').next().unwrap_or(rest).to_string();
    Some(PackageManager {
        name: name.to_string(),
        version,
    })
}

/// Read the field from a manifest body.
pub fn of_manifest(v: &serde_json::Value) -> Option<PackageManager> {
    v.get("packageManager")
        .and_then(|p| p.as_str())
        .and_then(parse_field)
}

/// Our own name and version (compile-time, so it can never drift).
pub fn this_tool() -> PackageManager {
    PackageManager {
        name: "jhol".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Result of comparing the project's pin with this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinVerdict {
    /// No pin, or the pin names jhol and the major matches.
    Ok,
    /// A different package manager is pinned.
    OtherTool { name: String, version: String },
    /// jhol is pinned at a different major (an incompatible tool version).
    VersionSkew { pinned: String, running: String },
}

impl PinVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, PinVerdict::Ok)
    }

    /// Human-readable message for a non-Ok verdict.
    pub fn message(&self) -> Option<String> {
        match self {
            PinVerdict::Ok => None,
            PinVerdict::OtherTool { name, version } => Some(format!(
                "this project pins `packageManager: {name}@{version}`, but jhol is installing it",
            )),
            PinVerdict::VersionSkew { pinned, running } => Some(format!(
                "this project pins jhol@{pinned}, but this is jhol@{running}",
            )),
        }
    }
}

/// Compare a project's pin with this build. A major-version match is enough:
/// patch releases of the installer must not block an install.
pub fn check(project: &std::path::Path) -> PinVerdict {
    let Ok(text) = std::fs::read_to_string(project.join("package.json")) else {
        return PinVerdict::Ok;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return PinVerdict::Ok;
    };
    let Some(pin) = of_manifest(&v) else {
        return PinVerdict::Ok;
    };
    let me = this_tool();
    if pin.name != me.name {
        return PinVerdict::OtherTool {
            name: pin.name,
            version: pin.version,
        };
    }
    let pin_major = pin.version.split('.').next().unwrap_or("");
    let my_major = me.version.split('.').next().unwrap_or("");
    if !pin_major.is_empty() && pin_major != my_major {
        return PinVerdict::VersionSkew {
            pinned: pin.version,
            running: me.version,
        };
    }
    PinVerdict::Ok
}

/// Whether a mismatch should fail rather than warn.
pub fn strict() -> bool {
    std::env::var("JHOL_PM_STRICT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// duplicate detection
// ---------------------------------------------------------------------------

/// A package present at more than one version in the installed tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub name: String,
    /// (version, how many copies) sorted by version.
    pub versions: Vec<(String, usize)>,
}

/// Scan node_modules (recursively, including nested `node_modules`) for
/// packages that appear at more than one version.
pub fn find_duplicates(project: &std::path::Path) -> Vec<Duplicate> {
    use std::collections::HashMap;
    let mut seen: HashMap<String, HashMap<String, usize>> = HashMap::new();

    fn walk(
        nm: &std::path::Path,
        seen: &mut HashMap<String, HashMap<String, usize>>,
        depth: usize,
    ) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(nm) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == ".bin" {
                continue;
            }
            let path = entry.path();
            if name.starts_with('@') && path.is_dir() {
                if let Ok(scoped) = std::fs::read_dir(&path) {
                    for s in scoped.flatten() {
                        let full = format!("{}/{}", name, s.file_name().to_string_lossy());
                        record(&s.path(), &full, seen);
                        walk(&s.path().join("node_modules"), seen, depth + 1);
                    }
                }
            } else if path.is_dir() {
                record(&path, &name, seen);
                walk(&path.join("node_modules"), seen, depth + 1);
            }
        }
    }

    fn record(
        dir: &std::path::Path,
        name: &str,
        seen: &mut HashMap<String, HashMap<String, usize>>,
    ) {
        let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let Some(ver) = v.get("version").and_then(|x| x.as_str()) else {
            return;
        };
        *seen
            .entry(name.to_string())
            .or_default()
            .entry(ver.to_string())
            .or_insert(0) += 1;
    }

    walk(&project.join("node_modules"), &mut seen, 0);

    let mut out: Vec<Duplicate> = seen
        .into_iter()
        .filter(|(_, versions)| versions.len() > 1)
        .map(|(name, versions)| {
            let mut v: Vec<(String, usize)> = versions.into_iter().collect();
            v.sort();
            Duplicate { name, versions: v }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_field() {
        let pm = parse_field("jhol@2.0.0").unwrap();
        assert_eq!(pm.name, "jhol");
        assert_eq!(pm.version, "2.0.0");
        let pm = parse_field("pnpm@9.1.0+sha512.abc").unwrap();
        assert_eq!(pm.name, "pnpm");
        assert_eq!(pm.version, "9.1.0");
        assert!(parse_field("nonsense").is_none());
        assert!(parse_field("@scope/name@1.0.0").is_some());
    }

    #[test]
    fn verdicts() {
        assert!(this_tool().name == "jhol");
        let other = PinVerdict::OtherTool {
            name: "pnpm".into(),
            version: "9.0.0".into(),
        };
        assert!(!other.is_ok());
        assert!(other.message().unwrap().contains("pnpm@9.0.0"));
    }

    #[test]
    fn finds_duplicate_versions() {
        let root = std::env::temp_dir().join(format!("jhol-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mk = |p: &std::path::Path, name: &str, ver: &str| {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(
                p.join("package.json"),
                format!(r#"{{"name":"{}","version":"{}"}}"#, name, ver),
            )
            .unwrap();
        };
        mk(&root.join("node_modules/lodash"), "lodash", "4.17.21");
        mk(
            &root.join("node_modules/a/node_modules/lodash"),
            "lodash",
            "3.10.1",
        );
        let dups = find_duplicates(&root);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].name, "lodash");
        assert_eq!(dups[0].versions.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
