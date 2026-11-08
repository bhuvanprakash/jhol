//! `jhol why-not <pkg>@<range>` — why a range is not available, and what to relax.
//!
//! "Resolution failed" is the least useful sentence a package manager can say.
//! The reason a range is unavailable is always the same shape: the versions that
//! satisfy the request are exactly the ones some dependents forbid. Because
//! versions are matched by predicate, that question is a **set intersection**,
//! and the smallest set of requirements whose intersection is empty is found by
//! shrinking greedily: drop a requirement, and if the intersection is still
//! empty, it was not needed.
//!
//! No clause database, no nogoods — the representation already is the solver's
//! explanation language. That is why this is cheap enough to run on request.

use std::collections::BTreeSet;
use std::path::Path;

/// One requirement from the installed tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Requirement {
    /// `name@version`, or `<root>` for the project manifest.
    pub dependent: String,
    pub spec: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyNotReport {
    pub package: String,
    pub requested: String,
    /// Versions that satisfy the request on its own (newest first, capped).
    pub candidates: Vec<String>,
    /// Every requirement the installed tree places on this package.
    pub requirements: Vec<Requirement>,
    /// A smallest set of requirements that cannot hold together with the request.
    /// Empty when the request is actually satisfiable.
    pub core: Vec<Requirement>,
    /// A representative published version, for the "relax to this" hint.
    pub newest_published: Option<String>,
    /// An already-installed copy that satisfies the request, if any. A range that
    /// is in use is not "unavailable", and answering otherwise would be worse
    /// than useless.
    pub installed_copy: Option<(String, String)>,
    pub satisfiable: bool,
}

impl WhyNotReport {
    pub fn render(&self) -> String {
        if let Some((version, path)) = &self.installed_copy {
            return format!(
                "{}@{} is already installed and satisfies the request: {} at {}",
                self.package, self.requested, version, path
            );
        }
        if self.satisfiable {
            return format!(
                "{}@{} can be satisfied; newest matching published version is {}",
                self.package,
                self.requested,
                self.candidates
                    .first()
                    .cloned()
                    .or_else(|| self.newest_published.clone())
                    .unwrap_or_else(|| "?".to_string())
            );
        }
        let mut out = format!(
            "no published version of {} satisfies {} together with the installed tree.\n",
            self.package, self.requested
        );
        if self.candidates.is_empty() {
            out.push_str(&format!(
                "  nothing published matches {} at all{}.\n",
                self.requested,
                self.newest_published
                    .as_ref()
                    .map(|v| format!(" (newest published is {})", v))
                    .unwrap_or_default()
            ));
            return out;
        }
        out.push_str(&format!(
            "  the only versions matching it are: {}\n",
            self.candidates.join(", ")
        ));
        out.push_str("  and these requirements forbid every one of them:\n");
        for r in &self.core {
            out.push_str(&format!("    {} needs {} {}\n", r.dependent, self.package, r.spec));
        }
        out.push_str(
            "  relaxing any one of them (or the request) would allow a version to be chosen.\n",
        );
        out
    }
}

/// Smallest set of requirements that cannot hold together with the request.
///
/// Greedy shrinking is exact for this purpose: a requirement that can be dropped
/// without making the intersection non-empty was never part of any minimal core,
/// and one pass suffices because the property ("intersection is empty") is
/// monotone in the set of requirements.
fn minimal_core(
    base: &BTreeSet<String>,
    matching: &[(Requirement, BTreeSet<String>)],
) -> Vec<Requirement> {
    let mut keep: Vec<usize> = (0..matching.len()).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for idx in 0..keep.len() {
            let candidate: Vec<usize> = keep
                .iter()
                .copied()
                .enumerate()
                .filter(|(pos, _)| *pos != idx)
                .map(|(_, v)| v)
                .collect();
            let mut alive = base.clone();
            for i in &candidate {
                alive = alive
                    .intersection(&matching[*i].1)
                    .cloned()
                    .collect::<BTreeSet<String>>();
            }
            if alive.is_empty() {
                keep = candidate;
                changed = true;
                break;
            }
        }
    }
    keep.into_iter().map(|i| matching[i].0.clone()).collect()
}

/// Decide whether `requested` can be met given `requirements`.
fn analyze(
    package: &str,
    requested: &str,
    versions_desc: &[String],
    requirements: &[Requirement],
) -> WhyNotReport {
    let req = crate::registry::compile_spec(requested);
    let matches = |spec: &crate::registry::CompiledSpec, v: &str| {
        semver::Version::parse(v)
            .map(|p| spec.matches(&p))
            .unwrap_or(false)
    };

    let candidates: Vec<String> = versions_desc
        .iter()
        .filter(|v| matches(&req, v))
        .cloned()
        .collect();
    let base: BTreeSet<String> = candidates.iter().cloned().collect();

    let matching: Vec<(Requirement, BTreeSet<String>)> = requirements
        .iter()
        .map(|r| {
            let c = crate::registry::compile_spec(&r.spec);
            (
                r.clone(),
                versions_desc
                    .iter()
                    .filter(|v| matches(&c, v))
                    .cloned()
                    .collect::<BTreeSet<String>>(),
            )
        })
        .collect();

    if base.is_empty() {
        return WhyNotReport {
            package: package.to_string(),
            requested: requested.to_string(),
            candidates,
            requirements: requirements.to_vec(),
            core: Vec::new(),
            newest_published: versions_desc.first().cloned(),
            installed_copy: None,
            satisfiable: false,
        };
    }

    let mut alive = base.clone();
    for (_, set) in &matching {
        alive = alive.intersection(set).cloned().collect::<BTreeSet<String>>();
    }
    let satisfiable = !alive.is_empty();
    let core = if satisfiable {
        Vec::new()
    } else {
        minimal_core(&base, &matching)
    };

    WhyNotReport {
        package: package.to_string(),
        requested: requested.to_string(),
        candidates,
        requirements: requirements.to_vec(),
        core,
        newest_published: versions_desc.first().cloned(),
        installed_copy: None,
        satisfiable,
    }
}

/// Every requirement the installed tree places on `package`, from the lockfile.
///
/// Scanned independently of which copies are installed, because the question
/// "why can I not use this range" is usually asked about a range that is *not*
/// installed.
pub fn installed_requirements(dir: &Path, package: &str) -> Option<Vec<Requirement>> {
    let path = ["package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())?;
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    let packages = doc.get("packages")?.as_object()?;
    let mut out: BTreeSet<Requirement> = BTreeSet::new();
    for (key, entry) in packages {
        let dependent = if key.is_empty() {
            "<root>".to_string()
        } else {
            let name = entry
                .get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    key.rsplit("node_modules/")
                        .next()
                        .unwrap_or(key)
                        .to_string()
                });
            match entry.get("version").and_then(|v| v.as_str()) {
                Some(v) => format!("{}@{}", name, v),
                None => "<root>".to_string(),
            }
        };
        for section in ["dependencies", "optionalDependencies", "peerDependencies"] {
            let Some(spec) = entry
                .get(section)
                .and_then(|d| d.get(package))
                .and_then(|s| s.as_str())
            else {
                continue;
            };
            out.insert(Requirement {
                dependent: dependent.clone(),
                spec: spec.to_string(),
            });
        }
    }
    Some(out.into_iter().collect())
}

/// Answer `why-not` using the cached metadata for `package`.
pub fn explain(
    cache_dir: &Path,
    project_dir: &Path,
    package: &str,
    requested: &str,
) -> Option<WhyNotReport> {
    let (index, _) = crate::jagr_index::load_cache_dir(cache_dir);
    let pkg = index.packages.get(package)?;
    let mut versions_desc: Vec<String> = pkg.versions.clone();
    versions_desc.sort_by(|a, b| {
        let ka = semver::Version::parse(a).ok();
        let kb = semver::Version::parse(b).ok();
        kb.cmp(&ka)
    });
    let requirements = installed_requirements(project_dir, package).unwrap_or_default();
    let mut report = analyze(package, requested, &versions_desc, &requirements);
    // A range that is already in use is answered as in use, before any conflict
    // analysis: "you cannot use ^4" would be false when 4.3.0 is installed.
    if let Some(why) = crate::why::explain(project_dir, package) {
        let want = crate::registry::compile_spec(requested);
        for copy in &why.copies {
            if let Ok(v) = semver::Version::parse(&copy.version) {
                if want.matches(&v) {
                    report.installed_copy = Some((copy.version.clone(), copy.path.clone()));
                    report.satisfiable = true;
                    break;
                }
            }
        }
    }
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report(
        versions: &[&str],
        requested: &str,
        reqs: &[(&str, &str)],
    ) -> WhyNotReport {
        let mut versions_desc: Vec<String> = versions.iter().map(|v| (*v).to_string()).collect();
        versions_desc.sort_by(|a, b| {
            semver::Version::parse(b).ok().cmp(&semver::Version::parse(a).ok())
        });
        let requirements: Vec<Requirement> = reqs
            .iter()
            .map(|(d, s)| Requirement {
                dependent: (*d).to_string(),
                spec: (*s).to_string(),
            })
            .collect();
        analyze("p", requested, &versions_desc, &requirements)
    }

    /// A real multi-major conflict: two dependents pin the two majors, so a
    /// request for the other major cannot be met — and the core is both of them,
    /// not the whole tree.
    #[test]
    fn core_is_the_minimal_conflicting_set() {
        let r = report(
            &["4.3.0", "5.2.0"],
            "^4.0.0",
            &[
                ("a@1.0.0", "^5.0.0"),
                ("b@1.0.0", "^4.0.0"),
                ("c@1.0.0", "^4.1.0"),
            ],
        );
        assert!(!r.satisfiable);
        assert_eq!(r.candidates, vec!["4.3.0"]);
        assert_eq!(
            r.core,
            vec![Requirement {
                dependent: "a@1.0.0".into(),
                spec: "^5.0.0".into()
            }],
            "only the ^5 requirement blocks ^4.0.0; unrelated ones must not appear"
        );
        assert!(r.render().contains("a@1.0.0 needs p ^5.0.0"));
    }

    #[test]
    fn satisfiable_request_says_so_and_names_the_version() {
        let r = report(&["4.3.0", "5.2.0"], "^4.0.0", &[("b@1.0.0", "^4.0.0")]);
        assert!(r.satisfiable);
        assert_eq!(r.candidates.first().map(|s| s.as_str()), Some("4.3.0"));
        assert!(r.render().contains("4.3.0"), "{}", r.render());
    }

    #[test]
    fn empty_match_reports_that_nothing_publishes_it() {
        let r = report(&["1.0.0"], "^9.0.0", &[]);
        assert!(!r.satisfiable);
        assert!(r.candidates.is_empty());
        assert!(r.render().contains("nothing published matches ^9.0.0"));
    }
}
