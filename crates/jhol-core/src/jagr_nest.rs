//! Nested resolution: more than one version of a package per tree.
//!
//! ## Why this exists
//!
//! `rvm` resolves with **one domain per package**. That is the right model for a
//! flat tree and the wrong model for npm's, where `chalk@4` for `jest-cli` and
//! `chalk@2` for `log-symbols` coexist because `node_modules` is a tree of
//! directories, not a set of names. Measured consequence: every real root
//! (`jest`, `yargs`, `@types/node`) failed propagation with a genuine multi-major
//! conflict. Historically the failure was hidden — bitmaps anchored at different
//! ordinals were compared position-wise, silently accepting versions that
//! violated a requirement — so the flat solver "worked" by being wrong.
//!
//! This module resolves **instances** instead of packages: a requirement creates
//! an instance of the target, and only requirements whose ranges cannot share a
//! version get a second instance. That is npm's nesting, expressed directly.
//!
//! ## What it is not
//!
//! Not a copy of any published algorithm. It is deliberately the smallest thing
//! that can represent a multi-major tree, with three properties this project
//! insists on:
//!
//! * **Deterministic** — the worklist and every instance map are ordered by name,
//!   and version choice is "newest satisfying". The same manifest resolves to the
//!   same instances on any machine.
//! * **Explainable** — a conflict names the package, the range, and the chain of
//!   dependents that demanded it, because the requirement records who asked.
//! * **Bounded** — instance and step caps, so a pathological graph fails loudly
//!   instead of growing without limit.
//!
//! ## Deliberate limits (documented, not forgotten)
//!
//! * Peers are recorded but do not constrain instances: npm installs and warns,
//!   and making them binding would turn a working tree into a failure. The data
//!   is kept so `--strict-peer-deps` and `jhol why` can use it.
//! * Hoisting is a separate step: this produces the instance graph, not the
//!   directory layout. `flatten_for_tree` gives the deterministic hoisting order.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;

/// One resolved package instance. The same `package` may appear more than once
/// with different versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// The package actually resolved.
    pub package: String,
    /// The name it is installed as. Differs from `package` for `npm:` aliases
    /// (`"@jest/react-is-18": "npm:react-is@^18"` installs react-is under the
    /// alias name), so the tree layout must use this.
    pub installed_as: String,
    /// The version chosen for this instance.
    pub version: String,
    /// Every requirement that this instance satisfies, as `(dependent, spec)`.
    /// Sorted, so the derivation is stable and printable.
    pub required_by: Vec<(String, String)>,
}

/// Why a requirement could not be satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestFailure {
    /// The package has no packument in the supplied set. Not a resolution
    /// failure: the caller fetches it and asks again.
    MissingPackument,
    /// The package is known but nothing it publishes satisfies the range.
    Unsatisfiable,
    /// A guard fired (instance or step cap).
    Bounded,
    /// A cutoff was requested but the metadata carries no publish dates, so no
    /// version can be shown to have existed then. Reported instead of silently
    /// resolving to today, which is the one outcome a reproducibility feature
    /// must never produce.
    MissingDates,
}

/// A requirement that could not be satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestConflict {
    pub failure: NestFailure,
    pub package: String,
    pub spec: String,
    pub required_by: String,
    /// Versions that exist but do not satisfy, newest first (max 6).
    pub available: Vec<String>,
}

impl NestConflict {
    /// True when retrying after fetching `package` could succeed.
    pub fn needs_fetch(&self) -> bool {
        self.failure == NestFailure::MissingPackument
    }
}

impl NestConflict {
    /// One-line explanation naming who asked for what.
    pub fn describe(&self) -> String {
        match self.failure {
            NestFailure::MissingPackument => format!(
                "{} (required by {}) has no metadata in this set; fetch it and retry",
                self.package, self.required_by
            ),
            NestFailure::Unsatisfiable => format!(
                "no version of {} satisfies {} (required by {}); published: [{}]",
                self.package,
                self.spec,
                self.required_by,
                self.available.join(", ")
            ),
            NestFailure::Bounded => format!(
                "resolution on {} (required by {}) exceeded the instance or step budget",
                self.package, self.required_by
            ),
            NestFailure::MissingDates => format!(
                "--as-of cannot be honoured for {} (required by {}): the metadata in use has no \
                 publish dates. Dated resolution needs full packuments; the abbreviated form \
                 served from the cache omits `time`.",
                self.package, self.required_by
            ),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NestedResolution {
    /// Instances in deterministic order (by package, then version descending).
    pub instances: Vec<Instance>,
    /// `(dependent key, package)` → index into `instances`. The dependent key is
    /// `""` for a root requirement, or `package@version` for a dependency.
    pub edges: BTreeMap<(String, String), usize>,
    /// Peer requirements seen while resolving, for advisory reporting.
    pub peers: Vec<(String, String, String)>,
}

impl NestedResolution {
    /// Number of distinct packages that got more than one version.
    pub fn duplicated_packages(&self) -> usize {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for i in &self.instances {
            *counts.entry(i.package.as_str()).or_default() += 1;
        }
        counts.values().filter(|c| **c > 1).count()
    }

    /// The version each package would occupy at the top of `node_modules` under a
    /// simple hoisting rule: the instance required by the most dependents wins,
    /// ties broken by highest version. Everything else stays nested under its
    /// dependent. Deterministic by construction.
    pub fn flatten_for_tree(&self) -> HashMap<String, String> {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for i in &self.instances {
            *count.entry(i.package.as_str()).or_default() += 1;
        }
        let mut best: HashMap<&str, (&Instance, usize)> = HashMap::new();
        for i in &self.instances {
            let demanders = i.required_by.len();
            match best.get(i.package.as_str()) {
                Some((cur, cur_demanders)) => {
                    let better = demanders > *cur_demanders
                        || (demanders == *cur_demanders
                            && version_key(&i.version) > version_key(&cur.version));
                    if better {
                        best.insert(i.package.as_str(), (i, demanders));
                    }
                }
                None => {
                    best.insert(i.package.as_str(), (i, demanders));
                }
            }
        }
        best.into_iter()
            .map(|(name, (inst, _))| (name.to_string(), inst.version.clone()))
            .collect()
    }
}

/// Expand an `npm:` alias requirement into the package it really names.
///
/// `"@jest/react-is-18": "npm:react-is@^18.2.0"` means "install react-is ^18.2.0
/// under the name @jest/react-is-18". Without this the resolver asks the registry
/// for `@jest/react-is-18`, which does not exist (HTTP 404), and a real jest tree
/// stops there.
fn expand_alias(name: &str, spec: &str) -> (String, String) {
    let Some(rest) = spec.trim().strip_prefix("npm:") else {
        return (name.to_string(), spec.to_string());
    };
    // Split at the last '@' so scoped reals (`npm:@scope/pkg@^1`) parse.
    match rest.rsplit_once('@') {
        Some((real, range)) if !real.is_empty() => {
            let real = real.to_string();
            let range = if range.trim().is_empty() {
                "*".to_string()
            } else {
                range.to_string()
            };
            (real, range)
        }
        _ => (rest.to_string(), "*".to_string()),
    }
}

/// Newest published version satisfying `compiled`, at or before `cutoff`.
fn newest_matching(
    doc: &Value,
    versions_desc: &[String],
    published: &HashMap<&str, i64>,
    compiled: &crate::registry::CompiledSpec,
    options: &ResolveOptions,
) -> Option<String> {
    versions_desc
        .iter()
        .find(|v| {
            let ok = semver::Version::parse(v)
                .map(|p| compiled.matches(&p))
                .unwrap_or(false);
            if !ok {
                return false;
            }
            let dated = match options.cutoff {
                None => true,
                // No date means we cannot prove it existed then; exclude.
                Some(t) => published.get(v.as_str()).map(|p| *p <= t).unwrap_or(false),
            };
            dated && options.license_allows(doc, v)
        })
        .cloned()
}

/// Inputs beyond "what does the manifest ask for".
///
/// Both are *resolution* inputs, not post-hoc filters: a tree that violates policy
/// must never be built and then audited.
#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// Only versions published at or before this unix timestamp (`--as-of`).
    pub cutoff: Option<i64>,
    /// When set, only versions whose declared license is in this list are
    /// eligible (`--allow-license`).
    pub allowed_licenses: Option<Vec<String>>,
    /// Whether a version with no license field may be used anyway. Default false:
    /// a license policy that silently accepts unknown licenses is not a policy.
    pub allow_unknown_license: bool,
}

impl ResolveOptions {
    pub fn is_default(&self) -> bool {
        self.cutoff.is_none() && self.allowed_licenses.is_none() && !self.allow_unknown_license
    }

    /// License declared by `version` in `doc`, normalised to lower case.
    ///
    /// npm allows a string (`"MIT"`) or an object (`{"type": "MIT"}`); SPDX
    /// expressions (`"MIT OR Apache-2.0"`) are matched by any of their parts.
    fn licenses_of(&self, doc: &Value, version: &str) -> Vec<String> {
        let Some(info) = doc.get("versions").and_then(|v| v.get(version)) else {
            return Vec::new();
        };
        let raw = info.get("license").or_else(|| info.get("licenses"));
        let mut out = Vec::new();
        let push = |s: &str, out: &mut Vec<String>| {
            for part in s.split(" OR ").flat_map(|p| p.split(" AND ")) {
                let p = part.trim().trim_start_matches('(').trim_end_matches(')');
                if !p.is_empty() {
                    out.push(p.to_lowercase());
                }
            }
        };
        match raw {
            Some(Value::String(s)) => push(s, &mut out),
            Some(Value::Object(o)) => {
                if let Some(t) = o.get("type").and_then(|t| t.as_str()) {
                    push(t, &mut out);
                }
            }
            Some(Value::Array(items)) => {
                for item in items {
                    if let Some(t) = item.get("type").and_then(|t| t.as_str()) {
                        push(t, &mut out);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// May `version` be used under this policy?
    fn license_allows(&self, doc: &Value, version: &str) -> bool {
        let Some(allowed) = &self.allowed_licenses else {
            return true;
        };
        let allowed_lower: Vec<String> = allowed.iter().map(|l| l.trim().to_lowercase()).collect();
        let found = self.licenses_of(doc, version);
        if found.is_empty() {
            return self.allow_unknown_license;
        }
        found.iter().any(|l| allowed_lower.iter().any(|a| a == l))
    }
}

fn version_key(v: &str) -> (u64, u64, u64, String) {
    match semver::Version::parse(v) {
        Ok(p) => (
            p.major,
            p.minor,
            p.patch,
            p.pre.as_str().to_string(),
        ),
        Err(_) => (0, 0, 0, v.to_string()),
    }
}

/// Resolve a manifest into instances.
///
/// `roots` maps package name to the range the manifest asks for. The returned
/// graph contains at least one instance per reachable package, as many more as
/// the requirements demand, and a derivation per instance.
pub fn resolve(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
) -> Result<NestedResolution, NestConflict> {
    resolve_with_cutoff(packuments, roots, None)
}

/// Resolve as of a point in time: only versions published at or before `cutoff`
/// (a unix timestamp) are eligible.
///
/// Reproducible historical trees are the point. The fail-safe rule: a packument
/// without publish dates (the abbreviated form) cannot prove a version existed
/// before the cutoff, so that version is *excluded* rather than assumed old
/// enough. A caller wanting `--as-of` must supply full packuments, and gets a
/// conflict naming the reason if it did not.
pub fn resolve_with_cutoff(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
    cutoff: Option<i64>,
) -> Result<NestedResolution, NestConflict> {
    resolve_with_options(
        packuments,
        roots,
        &ResolveOptions {
            cutoff,
            ..Default::default()
        },
    )
}

/// Resolve under every policy input: history (`--as-of`) and license policy.
pub fn resolve_with_options(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
    options: &ResolveOptions,
) -> Result<NestedResolution, NestConflict> {
    const MAX_INSTANCES: usize = 20_000;
    const MAX_STEPS: usize = 200_000;

    // Package name -> published versions (descending) plus per-version deps, read
    // once. `None` means "not in the packument set": the caller decides whether
    // that is fatal, we report it as a conflict with an empty published list.
    struct Meta<'a> {
        /// The packument itself, for policy fields (license) that are read per
        /// version and are not worth copying into their own map.
        doc: &'a Value,
        versions: Vec<String>,
        /// Publish time per version, when the packument is a full one.
        published: HashMap<&'a str, i64>,
        deps: HashMap<&'a str, Vec<(&'a str, &'a str, bool)>>,
        peers: HashMap<&'a str, Vec<(&'a str, &'a str)>>,
    }
    let mut meta: HashMap<&str, Meta> = HashMap::new();
    for (name, doc) in packuments {
        let Some(versions) = doc.get("versions").and_then(|v| v.as_object()) else {
            continue;
        };
        let mut sorted: Vec<(semver::Version, String)> = versions
            .keys()
            .filter_map(|k| semver::Version::parse(k).ok().map(|v| (v, k.clone())))
            .collect();
        sorted.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
        let mut deps: HashMap<&str, Vec<(&str, &str, bool)>> = HashMap::new();
        let mut peers: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for (_parsed, ver) in &sorted {
            // Borrow the key from the packument, not from `sorted`, so the maps
            // outlive this loop.
            let Some((vkey, info)) = versions.get_key_value(ver.as_str()) else {
                continue;
            };
            let mut d = Vec::new();
            for (section, optional) in
                [("dependencies", false), ("optionalDependencies", true)]
            {
                if let Some(map) = info.get(section).and_then(|v| v.as_object()) {
                    for (t, s) in map {
                        // Targets that are not in the packument set are checked
                        // when the edge is expanded, where "optional" is known.
                        d.push((t.as_str(), s.as_str().unwrap_or("*"), optional));
                    }
                }
            }
            deps.insert(vkey.as_str(), d);
            if let Some(map) = info.get("peerDependencies").and_then(|v| v.as_object()) {
                peers.insert(
                    vkey.as_str(),
                    map.iter()
                        .map(|(t, s)| (t.as_str(), s.as_str().unwrap_or("*")))
                        .collect(),
                );
            }
        }
        let mut published: HashMap<&str, i64> = HashMap::new();
        if let Some(times) = doc.get("time").and_then(|t| t.as_object()) {
            for (ver, at) in times {
                if let Some(ts) = at
                    .as_str()
                    .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
                    .map(|dt| dt.timestamp())
                {
                    if let Some((k, _)) = versions.get_key_value(ver.as_str()) {
                        published.insert(k.as_str(), ts);
                    }
                }
            }
        }
        meta.insert(
            name.as_str(),
            Meta {
                doc,
                versions: sorted.into_iter().map(|(_, k)| k).collect(),
                published,
                deps,
                peers,
            },
        );
    }

    // Instances: index -> (package, version, required_by).
    let mut instances: Vec<Instance> = Vec::new();
    // package -> indices of its instances
    let mut by_package: HashMap<String, Vec<usize>> = HashMap::new();
    let mut edges: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut peers_seen: BTreeSet<(String, String, String)> = BTreeSet::new();

    // Worklist of requirements, processed in a deterministic order: sorted roots,
    // then dependencies in sorted (name, spec) order per instance.
    let mut queue: std::collections::VecDeque<(String, String, String)> =
        std::collections::VecDeque::new();
    for (name, spec) in roots.iter() {
        queue.push_back((name.clone(), spec.clone(), String::new()));
    }
    queue.make_contiguous().sort();

    let mut steps = 0usize;
    while let Some((target, spec, dependent)) = queue.pop_front() {
        steps += 1;
        if steps > MAX_STEPS {
            return Err(NestConflict {
                failure: NestFailure::Bounded,
                package: target,
                spec,
                required_by: dependent,
                available: Vec::new(),
            });
        }
        // `npm:` aliases resolve the real package under an alternate name.
        let (lookup, lookup_spec) = expand_alias(&target, &spec);
        if lookup != target {
            let Some(real_meta) = meta.get(lookup.as_str()) else {
                return Err(NestConflict {
                    failure: NestFailure::MissingPackument,
                    package: lookup,
                    spec: lookup_spec,
                    required_by: dependent,
                    available: Vec::new(),
                });
            };
            let _ = real_meta;
        }
        let Some(meta) = meta.get(lookup.as_str()) else {
            // No metadata for this package: the caller fetches and retries.
            return Err(NestConflict {
                failure: NestFailure::MissingPackument,
                package: target,
                spec,
                required_by: dependent,
                available: Vec::new(),
            });
        };

        // Reuse an existing instance whose version satisfies this requirement.
        let compiled = crate::registry::compile_spec(&lookup_spec);
        let existing = by_package.get(&lookup).and_then(|ids| {
            ids.iter().copied().find(|i| {
                semver::Version::parse(&instances[*i].version)
                    .map(|v| compiled.matches(&v))
                    .unwrap_or(false)
            })
        });
        let idx = match existing {
            Some(idx) => idx,
            None => {
                // Newest published version that satisfies the requirement and,
                // when a cutoff is set, existed then.
                let chosen = newest_matching(
                    meta.doc,
                    &meta.versions,
                    &meta.published,
                    &compiled,
                    options,
                );
                let Some(chosen) = chosen else {
                    if options.cutoff.is_some() && meta.published.is_empty() {
                        return Err(NestConflict {
                            failure: NestFailure::MissingDates,
                            package: target,
                            spec,
                            required_by: dependent,
                            available: meta.versions.iter().take(6).cloned().collect(),
                        });
                    }
                    let mut available: Vec<String> = meta.versions.iter().take(6).cloned().collect();
                    available.sort_by(|a, b| version_key(b).cmp(&version_key(a)));
                    return Err(NestConflict {
                        failure: NestFailure::Unsatisfiable,
                        package: target,
                        spec,
                        required_by: dependent,
                        available,
                    });
                };
                if instances.len() >= MAX_INSTANCES {
                    return Err(NestConflict {
                        failure: NestFailure::Bounded,
                        package: target,
                        spec,
                        required_by: dependent,
                        available: Vec::new(),
                    });
                }
                let idx = instances.len();
                instances.push(Instance {
                    package: lookup.clone(),
                    installed_as: target.clone(),
                    version: chosen.clone(),
                    required_by: Vec::new(),
                });
                by_package.entry(lookup.clone()).or_default().push(idx);

                // Enqueue this instance's own dependencies, sorted for
                // determinism, before moving on.
                let key = format!("{}@{}", target, chosen);
                let mut child: Vec<(String, String, bool)> = meta
                    .deps
                    .get(chosen.as_str())
                    .map(|d| {
                        d.iter()
                            .map(|(t, s, opt)| (t.to_string(), s.to_string(), *opt))
                            .collect()
                    })
                    .unwrap_or_default();
                child.sort();
                for (dep_name, dep_spec, optional) in child {
                    // Check availability under the *real* name: an `npm:` alias
                    // points at a package that exists, under a name that does
                    // not. Enqueue the alias name so `installed_as` is preserved.
                    let (real, _) = expand_alias(&dep_name, &dep_spec);
                    if !packuments.contains_key(&real) {
                        if optional {
                            continue; // optional edge to an unavailable package
                        }
                        return Err(NestConflict {
                            failure: NestFailure::MissingPackument,
                            package: dep_name,
                            spec: dep_spec,
                            required_by: key,
                            available: Vec::new(),
                        });
                    }
                    queue.push_back((dep_name, dep_spec, key.clone()));
                }
                // Peers: recorded, not binding (see module docs).
                if let Some(ps) = meta.peers.get(chosen.as_str()) {
                    for (p, s) in ps {
                        peers_seen.insert((key.clone(), (*p).to_string(), (*s).to_string()));
                    }
                }
                idx
            }
        };

        // Record the derivation and the edge from the dependent.
        let demander = if dependent.is_empty() {
            "<root>".to_string()
        } else {
            dependent.clone()
        };
        if !instances[idx]
            .required_by
            .iter()
            .any(|(d, s)| d == &demander && s == &spec)
        {
            instances[idx].required_by.push((demander, spec.clone()));
            instances[idx].required_by.sort();
        }
        edges.insert((dependent, target), idx);
    }

    // Deterministic instance order for callers and for diffs.
    let mut order: Vec<usize> = (0..instances.len()).collect();
    order.sort_by(|a, b| {
        let ia = &instances[*a];
        let ib = &instances[*b];
        (ia.package.as_str(), std::cmp::Reverse(version_key(&ia.version)))
            .cmp(&(ib.package.as_str(), std::cmp::Reverse(version_key(&ib.version))))
    });
    // Remap indices through the sorted order.
    let mut remap = vec![0usize; instances.len()];
    for (new_idx, old_idx) in order.iter().enumerate() {
        remap[*old_idx] = new_idx;
    }
    let sorted_instances: Vec<Instance> = order.iter().map(|i| instances[*i].clone()).collect();
    let edges = edges
        .into_iter()
        .map(|(k, v)| (k, remap[v]))
        .collect::<BTreeMap<_, _>>();

    Ok(NestedResolution {
        instances: sorted_instances,
        edges,
        peers: peers_seen.into_iter().collect(),
    })
}

/// Every package named by a dependency of a *known* package but absent from the
/// map, sorted and deduplicated.
///
/// Resolution reports one missing package at a time, which forces one registry
/// round trip per package: on `jest` that was ~270 sequential fetches and 54-70 s
/// of a cold install. This walks the frontier so the caller can fetch in
/// parallel. It over-approximates on purpose (it looks at every published
/// version, not just the one that will be chosen) — fetching a packument that
/// turns out not to be needed is cheap, and it avoids a second pass.
pub fn missing_packages(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
) -> Vec<String> {
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();

    for (name, _spec) in roots.iter() {
        match packuments.get_key_value(name.as_str()) {
            Some((k, _)) => {
                if seen.insert(k.as_str()) {
                    queue.push_back(k.as_str());
                }
            }
            None => {
                missing.insert(name.clone());
            }
        }
    }
    while let Some(name) = queue.pop_front() {
        let Some(doc) = packuments.get(name) else {
            continue;
        };
        let Some(versions) = doc.get("versions").and_then(|v| v.as_object()) else {
            continue;
        };
        for info in versions.values() {
            for section in ["dependencies", "optionalDependencies", "peerDependencies"] {
                let Some(map) = info.get(section).and_then(|v| v.as_object()) else {
                    continue;
                };
                for dep in map.keys() {
                    // An `npm:` alias targets a different package than its name.
                    let (real, _) = expand_alias(dep, map.get(dep).and_then(|v| v.as_str()).unwrap_or("*"));
                    if let Some((k, _)) = packuments.get_key_value(real.as_str()) {
                        if seen.insert(k.as_str()) {
                            queue.push_back(k.as_str());
                        }
                    } else {
                        missing.insert(real);
                    }
                }
            }
        }
    }
    missing.into_iter().collect()
}

/// Bounded variant of [`missing_packages`] for prefetching.
///
/// The first prefetch attempt followed `peerDependencies` and every published
/// version with no cap; the frontier never closed and a cold `jest` install hung
/// to a 600 s timeout with nothing installed. A prefetch is an optimisation, so it
/// must be impossible for it to be unbounded:
///
/// * **peers are not followed** — they are advisory here and pull in packages the
///   tree will never contain;
/// * `limit` caps how many names one round may return;
/// * the caller stops after `max_rounds` or when a round makes no progress.
///
/// Anything left over is fetched by the resolve loop one at a time, which is the
/// correctness path and always terminates.
pub fn missing_packages_bounded(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
    limit: usize,
) -> Vec<String> {
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();

    for name in roots.keys() {
        match packuments.get_key_value(name.as_str()) {
            Some((k, _)) => {
                if seen.insert(k.as_str()) {
                    queue.push_back(k.as_str());
                }
            }
            None => {
                missing.insert(name.clone());
                if missing.len() >= limit {
                    return missing.into_iter().collect();
                }
            }
        }
    }
    while let Some(name) = queue.pop_front() {
        let Some(doc) = packuments.get(name) else {
            continue;
        };
        let Some(versions) = doc.get("versions").and_then(|v| v.as_object()) else {
            continue;
        };
        for info in versions.values() {
            // Deliberately no peerDependencies: advisory, and they explode the
            // frontier (measured: never closed on a real jest tree).
            for section in ["dependencies", "optionalDependencies"] {
                let Some(map) = info.get(section).and_then(|v| v.as_object()) else {
                    continue;
                };
                for (dep, spec) in map {
                    let (real, _) =
                        expand_alias(dep, spec.as_str().unwrap_or("*"));
                    if let Some((k, _)) = packuments.get_key_value(real.as_str()) {
                        if seen.insert(k.as_str()) {
                            queue.push_back(k.as_str());
                        }
                    } else if missing.insert(real) && missing.len() >= limit {
                        return missing.into_iter().collect();
                    }
                }
            }
        }
    }
    missing.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pk(name: &str, versions: &[(&str, &[(&str, &str)])]) -> Value {
        let mut v = serde_json::Map::new();
        for (ver, deps) in versions {
            let mut d = serde_json::Map::new();
            for (t, s) in *deps {
                d.insert((*t).to_string(), json!(s));
            }
            v.insert(
                (*ver).to_string(),
                json!({ "version": ver, "dependencies": d }),
            );
        }
        json!({ "name": name, "versions": v })
    }

    /// The whole point: two dependents that need incompatible majors both get a
    /// version, which a flat resolve cannot do.
    #[test]
    fn resolves_two_majors_of_the_same_package() {
        let packuments = HashMap::from([
            (
                "app".to_string(),
                pk(
                    "app",
                    &[("1.0.0", &[("modern", "^1.0.0"), ("legacy", "^1.0.0")])],
                ),
            ),
            (
                "modern".to_string(),
                pk("modern", &[("1.0.0", &[("chalk", "^5.0.0")])]),
            ),
            (
                "legacy".to_string(),
                pk("legacy", &[("1.0.0", &[("chalk", "^2.0.0")])]),
            ),
            (
                "chalk".to_string(),
                pk("chalk", &[("2.4.2", &[]), ("5.3.0", &[])]),
            ),
        ]);
        let roots = HashMap::from([("app".to_string(), "^1.0.0".to_string())]);
        let resolved = resolve(&packuments, &roots).expect("multi-major tree resolves");

        let chalks: Vec<&str> = resolved
            .instances
            .iter()
            .filter(|i| i.package == "chalk")
            .map(|i| i.version.as_str())
            .collect();
        assert_eq!(chalks, vec!["5.3.0", "2.4.2"], "newest first, both majors");
        assert_eq!(resolved.duplicated_packages(), 1);

        // The derivation says who demanded which.
        let old = resolved
            .instances
            .iter()
            .find(|i| i.package == "chalk" && i.version == "2.4.2")
            .unwrap();
        assert_eq!(
            old.required_by,
            vec![("legacy@1.0.0".to_string(), "^2.0.0".to_string())]
        );
        // Hoisting picks the most-demanded version for the top level.
        let flat = resolved.flatten_for_tree();
        assert_eq!(flat.get("chalk").map(|s| s.as_str()), Some("5.3.0"));
    }

    /// Compatible requirements share one instance: nesting is a cost, not a habit.
    #[test]
    fn compatible_requirements_share_an_instance() {
        let packuments = HashMap::from([
            (
                "app".to_string(),
                pk("app", &[("1.0.0", &[("a", "^1.0.0"), ("b", "^1.0.0")])]),
            ),
            (
                "a".to_string(),
                pk("a", &[("1.0.0", &[("shared", "^2.0.0")])]),
            ),
            (
                "b".to_string(),
                pk("b", &[("1.0.0", &[("shared", "^2.1.0")])]),
            ),
            (
                "shared".to_string(),
                pk("shared", &[("2.0.0", &[]), ("2.1.0", &[])]),
            ),
        ]);
        let roots = HashMap::from([("app".to_string(), "*".to_string())]);
        let resolved = resolve(&packuments, &roots).unwrap();
        let shared: Vec<&str> = resolved
            .instances
            .iter()
            .filter(|i| i.package == "shared")
            .map(|i| i.version.as_str())
            .collect();
        assert_eq!(shared, vec!["2.1.0"], "one instance satisfying both");
        assert_eq!(resolved.duplicated_packages(), 0);
        assert_eq!(
            resolved.instances.iter().find(|i| i.package == "shared").unwrap().required_by.len(),
            2,
            "both dependents recorded on the shared instance"
        );
    }

    /// A genuinely unsatisfiable range fails with the chain, not a shrug.
    #[test]
    fn unsatisfiable_range_reports_the_chain() {
        let packuments = HashMap::from([
            ("app".to_string(), pk("app", &[("1.0.0", &[("dep", "^9.0.0")])])),
            ("dep".to_string(), pk("dep", &[("1.0.0", &[])])),
        ]);
        let roots = HashMap::from([("app".to_string(), "*".to_string())]);
        let err = resolve(&packuments, &roots).unwrap_err();
        assert_eq!(err.package, "dep");
        assert_eq!(err.spec, "^9.0.0");
        assert_eq!(err.required_by, "app@1.0.0");
        assert!(err.describe().contains("published: [1.0.0]"));
    }

    /// Same manifest, same graph — regardless of map insertion order.
    #[test]
    fn determinism_across_insertion_order() {
        let pkgs: Vec<(String, Value)> = vec![
            ("app".to_string(), pk("app", &[("1.0.0", &[("x", "^1"), ("y", "^1")])])),
            ("x".to_string(), pk("x", &[("1.0.0", &[("z", "^1")])])),
            ("y".to_string(), pk("y", &[("1.0.0", &[("z", "^2")])])),
            ("z".to_string(), pk("z", &[("1.5.0", &[]), ("2.5.0", &[])])),
        ];
        let roots = HashMap::from([("app".to_string(), "*".to_string())]);

        let a: HashMap<String, Value> = pkgs.iter().cloned().collect();
        let b: HashMap<String, Value> = pkgs.iter().rev().cloned().collect();
        let ra = resolve(&a, &roots).unwrap();
        let rb = resolve(&b, &roots).unwrap();
        assert_eq!(ra.instances, rb.instances);
    }

    #[test]
    fn optional_edges_to_missing_packages_are_dropped() {
        let packuments = HashMap::from([(
            "app".to_string(),
            json!({ "name": "app", "versions": { "1.0.0": {
                "optionalDependencies": { "not-published": "^1" } } } }),
        )]);
        let roots = HashMap::from([("app".to_string(), "*".to_string())]);
        let resolved = resolve(&packuments, &roots).expect("optional gap is not fatal");
        assert_eq!(resolved.instances.len(), 1);
    }
}

#[cfg(test)]
mod bounded_frontier_tests {
    use super::*;
    use serde_json::json;

    fn world(edges: usize) -> HashMap<String, Value> {
        let mut w = HashMap::new();
        let mut deps = serde_json::Map::new();
        for i in 0..edges {
            deps.insert(format!("dep{i}"), json!("^1"));
        }
        w.insert(
            "root".to_string(),
            json!({ "name": "root", "versions": { "1.0.0": { "dependencies": deps } } }),
        );
        w
    }

    /// The cap is a hard ceiling, not a hint: an optimisation that can enumerate
    /// an arbitrary graph is the bug that hung a cold install.
    #[test]
    fn limit_is_respected_exactly() {
        let w = world(50);
        let roots = HashMap::from([("root".to_string(), "*".to_string())]);
        let missing = missing_packages_bounded(&w, &roots, 10);
        assert_eq!(missing.len(), 10, "{:?}", missing);
        assert!(missing.iter().all(|m| m.starts_with("dep")));
    }

    #[test]
    fn unknown_root_is_reported_and_capped() {
        let w = HashMap::new();
        let roots = HashMap::from([("nope".to_string(), "*".to_string())]);
        assert_eq!(missing_packages_bounded(&w, &roots, 5), vec!["nope"]);
    }

    /// Peers must not be followed: they are advisory and were what made the
    /// frontier unclosable.
    #[test]
    fn peer_dependencies_are_not_followed() {
        let w: HashMap<String, Value> = HashMap::from([(
            "root".to_string(),
            json!({ "name": "root", "versions": { "1.0.0": {
                "dependencies": {},
                "peerDependencies": { "host": "^1" } } } }),
        )]);
        let roots = HashMap::from([("root".to_string(), "*".to_string())]);
        assert!(missing_packages_bounded(&w, &roots, 100).is_empty());
    }
}

#[cfg(test)]
mod as_of_tests {
    use super::*;
    use serde_json::json;

    fn world() -> HashMap<String, Value> {
        // Three releases with publish dates; the newest is 2.0.0.
        HashMap::from([(
            "app".to_string(),
            json!({
                "name": "app",
                "versions": {
                    "1.0.0": { "version": "1.0.0" },
                    "1.5.0": { "version": "1.5.0" },
                    "2.0.0": { "version": "2.0.0" }
                },
                "time": {
                    "1.0.0": "2023-01-01T00:00:00.000Z",
                    "1.5.0": "2023-06-01T00:00:00.000Z",
                    "2.0.0": "2024-06-01T00:00:00.000Z"
                }
            }),
        )])
    }

    fn roots() -> HashMap<String, String> {
        HashMap::from([("app".to_string(), "*".to_string())])
    }

    fn ts(iso: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(iso).unwrap().timestamp()
    }

    #[test]
    fn cutoff_excludes_versions_published_after_it() {
        let w = world();
        let r = resolve_with_cutoff(&w, &roots(), Some(ts("2023-07-01T00:00:00Z"))).unwrap();
        assert_eq!(r.instances[0].version, "1.5.0");
        let r = resolve_with_cutoff(&w, &roots(), Some(ts("2023-02-01T00:00:00Z"))).unwrap();
        assert_eq!(r.instances[0].version, "1.0.0");
        let r = resolve_with_cutoff(&w, &roots(), Some(ts("2025-01-01T00:00:00Z"))).unwrap();
        assert_eq!(r.instances[0].version, "2.0.0");
    }

    /// The fail-safe rule: without publish dates a version cannot be proven to
    /// have existed before the cutoff, so it is excluded — abbreviating the
    /// decision would silently produce a tree that is not reproducible.
    #[test]
    fn versions_without_dates_are_excluded_under_a_cutoff() {
        let w: HashMap<String, Value> = HashMap::from([(
            "app".to_string(),
            json!({ "name": "app", "versions": { "1.0.0": { "version": "1.0.0" } } }),
        )]);
        let err = resolve_with_cutoff(&w, &roots(), Some(0)).unwrap_err();
        assert_eq!(err.package, "app");
        assert_eq!(err.failure, NestFailure::MissingDates);
        assert!(err.describe().contains("no publish dates"), "{}", err.describe());
        // Without a cutoff the same world resolves.
        assert!(resolve(&w, &roots()).is_ok());
    }
}

#[cfg(test)]
mod license_policy_tests {
    use super::*;
    use serde_json::json;

    fn world() -> HashMap<String, Value> {
        HashMap::from([(
            "app".to_string(),
            json!({
                "name": "app",
                "versions": {
                    "1.0.0": { "version": "1.0.0", "license": "MIT" },
                    "1.1.0": { "version": "1.1.0", "license": "GPL-3.0" },
                    "1.2.0": { "version": "1.2.0" },
                    "1.3.0": { "version": "1.3.0", "license": { "type": "Apache-2.0" } },
                    "1.4.0": { "version": "1.4.0", "license": "MIT OR GPL-3.0" }
                }
            }),
        )])
    }

    fn roots() -> HashMap<String, String> {
        HashMap::from([("app".to_string(), "*".to_string())])
    }

    fn opts(allow: &[&str], unknown: bool) -> ResolveOptions {
        ResolveOptions {
            allowed_licenses: Some(allow.iter().map(|s| (*s).to_string()).collect()),
            allow_unknown_license: unknown,
            ..Default::default()
        }
    }

    /// Policy is a resolution input: the solver must pick a version that complies
    /// rather than build a tree and audit it afterwards.
    #[test]
    fn policy_chooses_the_newest_compliant_version() {
        // 1.4.0 declares `MIT OR GPL-3.0`, so MIT alone permits it and the newest
        // compliant release wins; 1.1.0 (GPL) and 1.3.0 (Apache) are skipped.
        let r = resolve_with_options(&world(), &roots(), &opts(&["MIT"], false)).unwrap();
        assert_eq!(r.instances[0].version, "1.4.0");
        // Matching is case-insensitive, because publishers are inconsistent.
        let r = resolve_with_options(&world(), &roots(), &opts(&["mit"], false)).unwrap();
        assert_eq!(r.instances[0].version, "1.4.0");
    }

    #[test]
    fn object_and_spdx_forms_are_understood() {
        let r = resolve_with_options(&world(), &roots(), &opts(&["Apache-2.0"], false)).unwrap();
        assert_eq!(r.instances[0].version, "1.3.0");
    }

    /// A policy that silently accepts unknown licenses is not a policy.
    #[test]
    fn unknown_license_is_excluded_unless_allowed() {
        let strict = resolve_with_options(&world(), &roots(), &opts(&["MIT"], false));
        assert!(strict.is_ok());
        let err = resolve_with_options(&world(), &roots(), &opts(&["BSD-3-Clause"], false));
        assert!(err.is_err(), "nothing is BSD-3-Clause");
        // 1.2.0 has no license at all; it is only reachable with the escape hatch,
        // and then it is the newest eligible version.
        let permissive =
            resolve_with_options(&world(), &roots(), &opts(&["BSD-3-Clause"], true)).unwrap();
        assert_eq!(permissive.instances[0].version, "1.2.0");
    }
}
