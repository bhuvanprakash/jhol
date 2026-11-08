//! `jhol why` — answer "why is this package here?" from the lockfile alone.
//!
//! No registry, no re-resolution, no network: the lockfile already records every
//! dependency declaration and every installed version, which is exactly the
//! derivation of the tree. That is the point — a resolver that can only explain
//! itself by resolving again has not explained anything, and explains it slowly.
//!
//! The answer names each installed copy, the range each dependent asked for, and
//! whether the root manifest asked for it directly, so "why is `chalk@2` here?"
//! is answered with `log-symbols@4.1.0 needs chalk ^2.0.0`.

use std::collections::BTreeMap;
use std::path::Path;

/// One installed copy of a package and who demanded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    /// Lockfile path, e.g. `node_modules/chalk` or `node_modules/log-symbols/node_modules/chalk`.
    pub path: String,
    pub version: String,
    /// `(dependent, spec)` pairs that require this package. `<root>` is the
    /// project manifest.
    pub required_by: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyReport {
    pub package: String,
    pub copies: Vec<Copy>,
    /// Specs the project manifest asks for, if any.
    pub root_spec: Option<String>,
}

impl WhyReport {
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty()
    }

    /// Human-readable answer, stable ordering throughout.
    pub fn render(&self) -> String {
        if self.copies.is_empty() {
            return format!("{} is not installed (no entry in the lockfile)", self.package);
        }
        let mut out = String::new();
        let n = self.copies.len();
        out.push_str(&format!(
            "{}{}\n",
            if n == 1 { "1 installed copy of " } else { "" },
            if n == 1 {
                self.package.clone()
            } else {
                format!("{} installed copies of {}", n, self.package)
            }
        ));
        for c in &self.copies {
            out.push_str(&format!("  {}  ({})\n", c.version, c.path));
            match &self.root_spec {
                Some(_) if c.required_by.iter().any(|(d, _)| d == "<root>") => {
                    out.push_str(&format!(
                        "    asked for by package.json: {}\n",
                        self.root_spec.clone().unwrap_or_default()
                    ));
                }
                _ => {}
            }
            if c.required_by.is_empty() {
                out.push_str("    nothing in the lockfile declares it (extraneous?)\n");
            }
            for (dependent, spec) in &c.required_by {
                if dependent == "<root>" {
                    continue;
                }
                out.push_str(&format!("    {} needs {} {}\n", dependent, self.package, spec));
            }
        }
        out
    }
}

/// Explain every installed copy of `package`, from the lockfile in `dir`.
///
/// Returns `None` when there is no readable lockfile — a different situation from
/// "not installed", and the caller should say so rather than claim absence.
pub fn explain(dir: &Path, package: &str) -> Option<WhyReport> {
    let path = ["package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())?;
    let text = std::fs::read_to_string(&path).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    let packages = doc.get("packages")?.as_object()?;

    // Every path that is an installed copy of this package.
    let needle = format!("node_modules/{}", package);
    let mut copies: BTreeMap<String, Copy> = BTreeMap::new();
    for (key, entry) in packages {
        let is_copy = key == &needle || key.ends_with(&format!("/{}", needle));
        if !is_copy {
            continue;
        }
        let version = entry
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        copies.insert(
            key.clone(),
            Copy {
                path: key.clone(),
                version,
                required_by: Vec::new(),
            },
        );
    }

    // Who declares this package, anywhere in the lockfile.
    for (key, entry) in packages {
        let dependent = if key.is_empty() {
            "<root>".to_string()
        } else {
            // `node_modules/a/node_modules/b` -> `a@version`
            let name = entry
                .get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| key.rsplit("node_modules/").next().unwrap_or(key).to_string());
            match entry.get("version").and_then(|v| v.as_str()) {
                Some(v) => format!("{}@{}", name, v),
                None => "<root>".to_string(),
            }
        };
        for section in ["dependencies", "optionalDependencies", "peerDependencies"] {
            let Some(deps) = entry.get(section).and_then(|d| d.as_object()) else {
                continue;
            };
            let Some(spec) = deps.get(package).and_then(|s| s.as_str()) else {
                continue;
            };
            // Attribute the requirement to the copy *nearest* the declaring
            // entry: a package nested under `a` demands the copy nested under
            // `a`, not the hoisted one. Falls back to the hoisted copy only when
            // the dependent has no nested copy of its own.
            let nested_prefix = format!("{}/node_modules/", key);
            let target = copies
                .keys()
                .filter(|c| c.starts_with(&nested_prefix))
                .min_by_key(|c| c.len())
                .cloned()
                .or_else(|| copies.contains_key(&needle).then(|| needle.clone()));
            if let Some(t) = target {
                if let Some(c) = copies.get_mut(&t) {
                    let entry = (dependent.clone(), spec.to_string());
                    if !c.required_by.contains(&entry) {
                        c.required_by.push(entry);
                    }
                }
            }
        }
    }
    for c in copies.values_mut() {
        c.required_by.sort();
    }

    // The project manifest's own request, if any.
    let root_spec = std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|pkg| {
            for section in ["dependencies", "devDependencies", "optionalDependencies"] {
                if let Some(s) = pkg
                    .get(section)
                    .and_then(|d| d.get(package))
                    .and_then(|spec| spec.as_str())
                {
                    return Some(s.to_string());
                }
            }
            None
        });

    Some(WhyReport {
        package: package.to_string(),
        copies: copies.into_values().collect(),
        root_spec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(lock: &str, manifest: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package-lock.json"), lock).unwrap();
        std::fs::write(tmp.path().join("package.json"), manifest).unwrap();
        tmp
    }

    #[test]
    fn explains_a_transitive_copy_and_its_dependent() {
        let lock = r#"{"packages":{
            "":{"name":"app"},
            "node_modules/app-dep":{"version":"2.0.0","dependencies":{"chalk":"^2.0.0"}},
            "node_modules/app-dep/node_modules/chalk":{"version":"2.4.2"},
            "node_modules/chalk":{"version":"5.3.0"}
        }}"#;
        let manifest = r#"{"name":"app","dependencies":{"app-dep":"^2"}}"#;
        let tmp = project(lock, manifest);
        let report = explain(tmp.path(), "chalk").expect("lockfile");
        assert_eq!(report.copies.len(), 2);
        let nested = report
            .copies
            .iter()
            .find(|c| c.version == "2.4.2")
            .expect("nested copy");
        assert_eq!(
            nested.required_by,
            vec![("app-dep@2.0.0".to_string(), "^2.0.0".to_string())]
        );
        let text = report.render();
        assert!(text.contains("app-dep@2.0.0 needs chalk ^2.0.0"), "{text}");
        assert!(text.contains("2 installed copies of chalk"), "{text}");
    }

    #[test]
    fn attributes_a_direct_dependency_to_the_root() {
        let lock = r#"{"packages":{
            "":{"name":"app","dependencies":{"lodash":"^4.17.0"}},
            "node_modules/lodash":{"version":"4.17.21"}
        }}"#;
        let manifest = r#"{"name":"app","dependencies":{"lodash":"^4.17.0"}}"#;
        let tmp = project(lock, manifest);
        let report = explain(tmp.path(), "lodash").unwrap();
        assert_eq!(report.root_spec.as_deref(), Some("^4.17.0"));
        assert_eq!(report.copies[0].version, "4.17.21");
        assert!(report.render().contains("asked for by package.json: ^4.17.0"));
    }

    #[test]
    fn absent_package_is_reported_as_absent_not_missing() {
        let tmp = project(
            r#"{"packages":{"":{"name":"app"},"node_modules/a":{"version":"1.0.0"}}}"#,
            r#"{"name":"app"}"#,
        );
        let report = explain(tmp.path(), "not-here").unwrap();
        assert!(report.is_empty());
        assert!(report.render().contains("is not installed"));
    }

    /// No lockfile is a different answer from "not installed".
    #[test]
    fn no_lockfile_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(explain(tmp.path(), "x").is_none());
    }

    #[test]
    fn extraneous_copy_says_so_instead_of_inventing_a_dependent() {
        let tmp = project(
            r#"{"packages":{"":{"name":"app"},"node_modules/stray":{"version":"1.0.0"}}}"#,
            r#"{"name":"app"}"#,
        );
        let text = explain(tmp.path(), "stray").unwrap().render();
        assert!(text.contains("nothing in the lockfile declares it"), "{text}");
    }
}
