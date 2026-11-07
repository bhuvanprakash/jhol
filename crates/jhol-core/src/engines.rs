//! `engines` enforcement — refuse (or warn about) a tree the runtime cannot run.
//!
//! npm's rule, kept: a package whose `engines.node` range excludes the running
//! Node version warns by default and fails under `--engine-strict`. jhol has
//! its own engine and can also run on a user's Node, so the range is compared
//! against the language level this build implements (`NODE_COMPAT`) or, when
//! the process is plainly running under Node, that version.
//!
//! Only `engines.node` is enforced. `npm`, `yarn`, `pnpm`, `bun` and `jhol`
//! keys are read and reported, never blocked on: a project may legitimately
//! declare another package manager it also uses.

/// The Node language level this build implements. One constant, so the claim
/// is checkable: `engines.node` ranges compare against it.
pub const NODE_COMPAT: &str = "22.0.0";

/// Outcome for one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineVerdict {
    Ok,
    Mismatch { required: String, actual: String },
}

type V = (u64, u64, u64);

fn parse(v: &str) -> Option<V> {
    let core = v.trim().trim_start_matches('v').trim_start_matches('=');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// The runtime version to compare against: `$NODE_VERSION` when the caller set
/// it (a CI matrix), else `NODE_COMPAT`.
pub fn runtime_version() -> String {
    std::env::var("NODE_VERSION")
        .ok()
        .filter(|v| parse(v).is_some())
        .unwrap_or_else(|| NODE_COMPAT.to_string())
}

/// Does `actual` satisfy `range`?
///
/// Supports what packages actually publish: `>=14`, `>=14.0.0`, `^18.0.0`,
/// `~20.1`, `14.x`, `14`, `*`, conjunctions (`>=12 <19`) and alternatives
/// (`>=16 || >=18`). Anything unparsable counts as satisfied — refusing an
/// install because we could not read a range would be worse than the range.
pub fn check_node_range(range: &str, actual: &str) -> EngineVerdict {
    let range = range.trim();
    if range.is_empty() || range == "*" || range.eq_ignore_ascii_case("latest") {
        return EngineVerdict::Ok;
    }
    let Some(av) = parse(actual) else {
        return EngineVerdict::Ok;
    };
    for alt in range.split("||") {
        if satisfies_group(alt, av) {
            return EngineVerdict::Ok;
        }
    }
    EngineVerdict::Mismatch {
        required: range.to_string(),
        actual: actual.to_string(),
    }
}

fn satisfies_group(group: &str, actual: V) -> bool {
    let mut saw_any = false;
    for comp in group.split_whitespace() {
        if comp.is_empty() || comp == "*" {
            continue;
        }
        saw_any = true;
        if !satisfies_one(comp, actual) {
            return false;
        }
    }
    // An unrecognisable group is satisfied (see the module note).
    let _ = saw_any;
    true
}

fn satisfies_one(comp: &str, actual: V) -> bool {
    let comp = comp.trim();
    let (op, rest) = if let Some(r) = comp.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = comp.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = comp.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = comp.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = comp.strip_prefix('^') {
        ("^", r)
    } else if let Some(r) = comp.strip_prefix('~') {
        ("~", r)
    } else {
        ("=", comp)
    };
    let rest = rest.trim();
    // `x`-ranges: `14.x`, `14.1.x`, `14`.
    let x_range = rest.split('.').any(|p| p == "x" || p == "X" || p == "*");
    let Some(v) = parse(rest) else {
        return true; // unparsable comparator: do not block
    };
    match op {
        ">=" => actual >= v,
        "<=" => actual <= v,
        ">" => actual > v,
        "<" => actual < v,
        "^" => {
            // Caret: same major (or same minor when major is 0).
            if v.0 == 0 {
                actual.0 == 0 && actual.1 == v.1 && actual >= v
            } else {
                actual.0 == v.0 && actual >= v
            }
        }
        "~" => {
            // Tilde: same major.minor.
            actual.0 == v.0 && actual.1 == v.1 && actual >= v
        }
        _ => {
            if x_range {
                // `14.x` means major 14; `14.1.x` means 14.1.x.
                let depth = rest.split('.').take_while(|p| *p != "x" && *p != "X" && *p != "*").count();
                match depth {
                    1 => actual.0 == v.0,
                    2 => actual.0 == v.0 && actual.1 == v.1,
                    _ => actual == v,
                }
            } else {
                // Bare version as a minimum, matching node's lenient reading.
                actual >= v
            }
        }
    }
}

/// Read `engines.node` out of a package.json body.
pub fn node_range_of(pkg_json: &serde_json::Value) -> Option<String> {
    pkg_json
        .get("engines")
        .and_then(|e| e.get("node"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Scan installed packages and report engine mismatches.
/// Returns (package, required, actual) triples.
pub fn scan_installed(project: &std::path::Path) -> Vec<(String, String, String)> {
    let nm = project.join("node_modules");
    let actual = runtime_version();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&nm) else {
        return out;
    };
    let mut check = |dir: &std::path::Path, name: &str| {
        let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        if let Some(range) = node_range_of(&v) {
            if let EngineVerdict::Mismatch { required, actual } = check_node_range(&range, &actual) {
                out.push((name.to_string(), required, actual));
            }
        }
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
                    check(&s.path(), &full);
                }
            }
        } else if path.is_dir() {
            check(&path, &name);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(range: &str, actual: &str) -> bool {
        matches!(check_node_range(range, actual), EngineVerdict::Ok)
    }

    #[test]
    fn gte_and_lt() {
        assert!(ok(">=14", "22.0.0"));
        assert!(!ok(">=30", "22.0.0"));
        assert!(!ok(">=12 <19", "22.0.0"));
        assert!(ok(">=12 <25", "22.0.0"));
        assert!(ok(">21", "22.0.0"));
        assert!(!ok("<22", "22.0.0"));
    }

    #[test]
    fn caret_and_tilde() {
        assert!(!ok("^20.0.0", "22.0.0"), "major differs");
        assert!(!ok("^22.1.0", "22.0.0"), "actual below the caret floor");
        assert!(ok("^22.0.0", "22.0.0"));
        assert!(ok("~22.0", "22.0.5"));
        assert!(!ok("~22.1", "22.0.5"), "minor differs");
    }

    #[test]
    fn alternatives_and_wildcards() {
        assert!(ok(">=30 || >=18", "22.0.0"));
        assert!(ok("*", "22.0.0"));
        assert!(ok("", "22.0.0"));
        assert!(ok("14.x || 18.x || 20.x || 22.x", "22.0.0"));
        assert!(!ok("14.x || 16.x", "22.0.0"));
        assert!(ok("22", "22.0.0"), "bare major is a minimum");
    }

    #[test]
    fn unparsable_range_never_blocks() {
        assert!(ok("not-a-range", "22.0.0"));
        assert!(ok(">=banana", "22.0.0"));
    }

    #[test]
    fn reads_engines_node() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"engines":{"node":">=18","npm":">=9"}}"#).unwrap();
        assert_eq!(node_range_of(&v).as_deref(), Some(">=18"));
        let none: serde_json::Value = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(node_range_of(&none), None);
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn scan_finds_mismatched_package() {
        let root = std::env::temp_dir().join(format!("jhol-engscan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pkg = root.join("node_modules").join("future");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name":"future","version":"9.0.0","engines":{"node":">=40"}}"#,
        )
        .unwrap();
        let found = scan_installed(&root);
        assert_eq!(found.len(), 1, "got {:?}", found);
        assert_eq!(found[0].0, "future");
        let _ = std::fs::remove_dir_all(&root);
    }
}
