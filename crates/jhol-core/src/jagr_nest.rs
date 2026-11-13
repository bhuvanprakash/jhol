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
//! * **Bounded** — instance and cumulative worklist caps, including all retries.
//!   A known conflict can rewind to an earlier release choice. A metadata
//!   request goes back to the provider without being classified as a conflict.
//!
//! ## Deliberate limits (documented, not forgotten)
//!
//! * Peers are recorded but do not constrain instances: npm installs and warns,
//!   and making them binding would turn a working tree into a failure. The data
//!   is kept so `--strict-peer-deps` and `jhol why` can use it.
//! * Hoisting is a separate step: this produces the instance graph, not the
//!   directory layout. `flatten_for_tree` gives the deterministic hoisting order.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
    /// Exact consumer and provider IDs. A missing consumer denotes the root.
    pub instance_edges: BTreeMap<(Option<usize>, String), usize>,
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
pub(crate) fn expand_alias(name: &str, spec: &str) -> (String, String) {
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

pub(crate) fn compile_registry_requirement(spec: &str, packument: &Value) -> crate::registry::CompiledSpec {
    let spec = spec.trim();
    let compiled = crate::registry::compile_spec(spec);
    if !matches!(compiled, crate::registry::CompiledSpec::Unsupported) {
        return compiled;
    }
    packument.get("dist-tags").and_then(|tags| tags.get(spec)).and_then(Value::as_str)
        .and_then(|version| semver::Version::parse(version).ok())
        .map(crate::registry::CompiledSpec::Exact)
        .unwrap_or(crate::registry::CompiledSpec::Unsupported)
}

fn release_platform_applies(packument: &Value, version: &str) -> bool {
    packument.get("versions").and_then(|versions| versions.get(version))
        .is_some_and(jhol_treecheck::entry_platform_applies)
}

/// Pick a version that satisfies this demand **and** as many demands already
/// waiting in the worklist as possible.
///
/// Two ranges that overlap must share one directory. Choosing the newest matching
/// version in isolation and meeting the second demand afterwards creates a second
/// copy of a package that never conflicted: every avoidable instance is one more
/// directory to write, verify and ship, and the tree grows past what the
/// dependency graph requires.
///
/// Coverage is a tie-breaker, not a preference. `versions_desc` is newest-first,
/// so a candidate only replaces the incumbent when it covers strictly more
/// pending demands — with nothing competing, the newest release still wins and a
/// lone `^1.0.0` is never silently downgraded.
fn choose_version(
    package: &str,
    doc: &Value,
    versions_desc: &[String],
    published: &HashMap<&str, i64>,
    compiled: &crate::registry::CompiledSpec,
    pending: &[crate::registry::CompiledSpec],
    rejected: Option<&HashSet<String>>,
    options: &ResolveOptions,
    optional: bool,
) -> Option<String> {
    // Policy first: a version is eligible only if it satisfies the range and every
    // resolution input (`--as-of`, licences, banned versions).
    let eligible = |v: &str| -> Option<(usize, semver::Version)> {
        let parsed = semver::Version::parse(v).ok()?;
        if !compiled.matches(&parsed)
            || options.is_banned(package, v)
            || rejected.is_some_and(|versions| versions.contains(v))
            || (optional && !release_platform_applies(doc, v))
        {
            return None;
        }
        let dated = match options.cutoff {
            None => true,
            Some(t) => published.get(v).map(|p| *p <= t).unwrap_or(false),
        };
        if !dated || !options.license_allows(doc, v) {
            return None;
        }
        let covered = pending.iter().filter(|c| c.matches(&parsed)).count();
        Some((covered, parsed))
    };

    // **Start from the `latest` tag.** A fresh install should reproduce what the registry
    // advertises as current: npm, bun, pnpm and yarn all install the `latest`-tagged version
    // for an ordinary range, even when a published-but-untagged release also satisfies it
    // (measured: `get-intrinsic@1.3.0` for `^1.2.5`, with 1.3.1 published and untagged). Jho
    // taking 1.3.1 there made the same `package.json` produce a different tree than every
    // other tool — the divergence the scorecard exists to catch.
    //
    // Coverage still wins over the tag, exactly as before: this only sets the incumbent that
    // a candidate must strictly beat, so the tie-breaker rule is unchanged.
    let mut best: Option<(usize, String)> = None;
    if let Some(tag) = doc
        .get("dist-tags")
        .and_then(|t| t.get("latest"))
        .and_then(|v| v.as_str())
    {
        if versions_desc.iter().any(|v| v == tag) {
            if let Some((covered, _)) = eligible(tag) {
                best = Some((covered, tag.to_string()));
            }
        }
    }

    for v in versions_desc {
        let Some((covered, _)) = eligible(v) else {
            continue;
        };
        if best.as_ref().map(|(b, _)| covered > *b).unwrap_or(true) {
            best = Some((covered, v.clone()));
            if covered == pending.len() {
                break; // nothing in the worklist can be covered better
            }
        }
    }
    best.map(|(_, v)| v)
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
    /// Versions that must not be chosen, as `(package, version)` — known
    /// vulnerabilities. A security exclusion that only reports afterwards leaves
    /// the user to re-run the install and hope; making it an input means the
    /// resolver *finds* a tree that avoids them.
    pub banned: Option<std::collections::HashSet<(String, String)>>,
}

impl ResolveOptions {
    pub fn is_default(&self) -> bool {
        self.cutoff.is_none()
            && self.allowed_licenses.is_none()
            && !self.allow_unknown_license
            && self.banned.is_none()
    }

    /// Is this exact release excluded by a security policy?
    fn is_banned(&self, package: &str, version: &str) -> bool {
        match &self.banned {
            Some(set) => set.contains(&(package.to_string(), version.to_string())),
            None => false,
        }
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

/// Everything the resolver reads from one packument, borrowed so the cache is
/// never copied.
struct Meta<'a> {
    /// The packument itself, for policy fields (license) read per version.
    doc: &'a Value,
    versions: Vec<String>,
    /// Publish time per version, when the packument is a full one.
    published: HashMap<&'a str, i64>,
    deps: HashMap<&'a str, Vec<(&'a str, &'a str, bool)>>,
    peers: HashMap<&'a str, Vec<(&'a str, &'a str)>>,
}

/// Read one packument into the shape resolution wants.
///
/// `None` means "not a usable packument" (no `versions` object); the caller
/// reports it as a missing packument rather than pretending it is empty.
fn build_meta<'a>(doc: &'a Value) -> Option<Meta<'a>> {
    let versions = doc.get("versions").and_then(|v| v.as_object())?;
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
        let optional_deps = info.get("optionalDependencies").and_then(Value::as_object);
        for (section, optional) in [("dependencies", false), ("optionalDependencies", true)] {
            if let Some(map) = info.get(section).and_then(Value::as_object) {
                for (target, spec) in map {
                    if optional || optional_deps.is_none_or(|deps| !deps.contains_key(target)) {
                        d.push((target.as_str(), spec.as_str().unwrap_or("<invalid>"), optional));
                    }
                }
            }
        }
        deps.insert(vkey.as_str(), d);
        if let Some(map) = info.get("peerDependencies").and_then(|v| v.as_object()) {
            peers.insert(
                vkey.as_str(),
                map.iter()
                    .map(|(t, s)| (t.as_str(), s.as_str().unwrap_or("<invalid>")))
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
    Some(Meta {
        doc,
        versions: sorted.into_iter().map(|(_, k)| k).collect(),
        published,
        deps,
        peers,
    })
}

/// Metadata for `name`, built the first time it is reached and reused after.
///
/// The cache is keyed by a `&str` borrowed from `packuments` so a hit costs one
/// hash and no allocation; nothing is built for packages the tree never touches.
fn meta_for<'b, 'a>(
    cache: &'b mut HashMap<&'a str, Meta<'a>>,
    packuments: &'a HashMap<String, Value>,
    name: &str,
) -> Option<&'b Meta<'a>> {
    if !cache.contains_key(name) {
        let doc = packuments.get(name)?;
        let meta = build_meta(doc)?;
        let key = packuments.get_key_value(name)?.0.as_str();
        cache.insert(key, meta);
    }
    cache.get(name)
}

/// Parse a `--as-of` value into a unix timestamp: an RFC 3339 instant, or a
/// calendar date taken as the end of that day. `None` means "not a date", and the
/// caller must refuse rather than silently treating it as today.
pub fn parse_cutoff(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.timestamp())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(23, 59, 59))
                .map(|dt| dt.and_utc().timestamp())
        })
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
    resolve_with_optional_roots(packuments, roots, options, &HashSet::new())
}

pub(crate) fn resolve_with_optional_roots(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
    options: &ResolveOptions,
    optional_roots: &HashSet<String>,
) -> Result<NestedResolution, NestConflict> {
    let mut search = Search::default();
    let mut first_conflict = None;
    loop {
        let conflict = match resolve_branch(packuments, roots, options, optional_roots, &mut search) {
            Ok(resolved) => return Ok(resolved),
            Err(conflict) if conflict.failure == NestFailure::Unsatisfiable => conflict,
            Err(incomplete) => return Err(incomplete),
        };
        let conflict = first_conflict.get_or_insert(conflict);
        if !search.rewind() {
            return Err(conflict.clone());
        }
    }
}

#[derive(Clone)]
enum Pick {
    Release(String),
    Omit,
}

#[derive(Default)]
struct Choice {
    selected: Option<Pick>,
    rejected: HashSet<String>,
}

/// A deterministic decision prefix. Replaying it rebuilds the instance
/// graph and worklist from the fixed inputs, so a failed release cannot leave
/// behind child demands, edges, or peer records. A successful first branch
/// uses the normal worklist. Failed branches share one cumulative step budget.
#[derive(Default)]
struct Search {
    choices: Vec<Choice>,
    visited: usize,
    steps: usize,
}

impl Search {
    fn record(&mut self, selected: Pick) {
        if let Some(choice) = self.choices.get_mut(self.visited) {
            choice.selected = Some(selected);
        } else {
            self.choices.push(Choice {
                selected: Some(selected),
                rejected: HashSet::new(),
            });
        }
        self.visited += 1;
    }

    fn rewind(&mut self) -> bool {
        self.choices.truncate(self.visited);
        self.visited = 0;
        while let Some(choice) = self.choices.last_mut() {
            match choice.selected.take() {
                Some(Pick::Release(rejected)) => {
                    choice.rejected.insert(rejected);
                    return true;
                }
                _ => { self.choices.pop(); }
            }
        }
        false
    }
}

fn resolve_branch(
    packuments: &HashMap<String, Value>,
    roots: &HashMap<String, String>,
    options: &ResolveOptions,
    optional_roots: &HashSet<String>,
    search: &mut Search,
) -> Result<NestedResolution, NestConflict> {
    const MAX_INSTANCES: usize = 20_000;
    const MAX_STEPS: usize = 200_000;

    // Metadata is read **on demand**. A cache can hold ten thousand packuments
    // and a tree can need sixty; reading them all is work no user asked for, and
    // it makes resolution cost scale with the cache instead of with the tree.
    let mut meta: HashMap<&str, Meta> = HashMap::new();
    // Instances: index -> (package, version, required_by).
    let mut instances: Vec<Instance> = Vec::new();
    // package -> indices of its instances
    let mut by_package: HashMap<String, Vec<usize>> = HashMap::new();
    let mut edges: BTreeMap<(Option<usize>, String), usize> = BTreeMap::new();
    let mut peers_seen: BTreeSet<(String, String, String)> = BTreeSet::new();

    // Worklist of requirements, processed in a deterministic order: sorted roots,
    // then dependencies in sorted (name, spec) order per instance.
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct Demand {
        target: String,
        spec: String,
        consumer: Option<usize>,
        optional: bool,
    }

    let mut queue: std::collections::VecDeque<_> = roots.iter().map(|(name, spec)| Demand {
        target: name.clone(),
        spec: spec.clone(),
        consumer: None,
        optional: optional_roots.contains(name),
    }).collect();
    queue.make_contiguous().sort();
    while let Some(Demand { target, spec, consumer, optional }) = queue.pop_front() {
        let dependent = consumer.map(|id| {
            let instance = &instances[id];
            format!("{}@{}", instance.installed_as, instance.version)
        }).unwrap_or_default();
        search.steps += 1;
        if search.steps > MAX_STEPS {
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
        let Some(meta) = meta_for(&mut meta, packuments, &lookup) else {
            if optional {
                continue;
            }
            return Err(NestConflict {
                failure: NestFailure::MissingPackument,
                package: lookup,
                spec: lookup_spec,
                required_by: dependent,
                available: Vec::new(),
            });
        };

        // Reuse an existing instance whose version satisfies this requirement.
        let compiled = compile_registry_requirement(&lookup_spec, meta.doc);
        let existing = by_package.get(&lookup).and_then(|ids| {
            ids.iter().copied().find(|i| {
                // Identity is (installed name, version), not version alone. An
                // `npm:` alias and its real package are two directories even at
                // the same version: a dependent that asks for `alias` looks in
                // `node_modules/alias`, so folding it into `foo` loses the edge.
                if instances[*i].installed_as != target {
                    return false;
                }
                let v = &instances[*i].version;
                if options.is_banned(&lookup, v)
                    || (optional && !release_platform_applies(meta.doc, v))
                {
                    return false;
                }
                semver::Version::parse(v)
                    .map(|p| compiled.matches(&p))
                    .unwrap_or(false)
            })
        });
        let idx = match existing {
            Some(idx) => idx,
            None => {
                // Newest published version that satisfies the requirement and,
                // when a cutoff is set, existed then.
                // Demands for this same install name that are already waiting.
                // Bounded: past this the worklist is large enough that the extra
                // comparisons cost more than a duplicate copy saves.
                let decision = search.choices.get(search.visited);
                let chosen = decision.and_then(|choice| choice.selected.clone()).or_else(|| {
                    let pending: Vec<crate::registry::CompiledSpec> = if queue.len() <= 4096 {
                        queue
                            .iter()
                            .filter_map(|demand| {
                                if demand.target != target || demand.optional {
                                    return None;
                                }
                                let (package, range) = expand_alias(&demand.target, &demand.spec);
                                (package == lookup)
                                    .then(|| compile_registry_requirement(&range, meta.doc))
                            })
                            .take(64)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    choose_version(
                        &lookup,
                        meta.doc,
                        &meta.versions,
                        &meta.published,
                        &compiled,
                        &pending,
                        decision.map(|choice| &choice.rejected),
                        options,
                        optional,
                    ).map(Pick::Release)
                });
                let Some(chosen) = chosen else {
                    if optional {
                        if decision.is_some() {
                            search.record(Pick::Omit);
                        }
                        continue;
                    }
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
                let Pick::Release(chosen) = chosen else {
                    search.record(Pick::Omit);
                    continue;
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
                search.record(Pick::Release(chosen.clone()));
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
                    queue.push_back(Demand { target: dep_name, spec: dep_spec, consumer: Some(idx), optional });
                }
                if let Some(peers) = meta.peers.get(chosen.as_str()) {
                    for (peer, spec) in peers {
                        peers_seen.insert((key.clone(), (*peer).to_string(), (*spec).to_string()));
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
        edges.insert((consumer, target), idx);
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
    let mut instance_edges = BTreeMap::new();
    let mut printable_edges = BTreeMap::new();
    for ((consumer, name), provider) in edges {
        let key = consumer.map(|id| format!("{}@{}", instances[id].installed_as, instances[id].version))
            .unwrap_or_default();
        printable_edges.insert((key, name.clone()), remap[provider]);
        instance_edges.insert((consumer.map(|id| remap[id]), name), remap[provider]);
    }

    Ok(NestedResolution {
        instances: sorted_instances,
        edges: printable_edges,
        instance_edges,
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
        // Only the newest release's dependencies. The resolver picks the newest
        // version that satisfies the range, so older releases' dependencies are, in
        // the common case, never needed — and walking *every* version inflated the
        // frontier two to three times over (196 documents for a 72-package tree; 4.7 s
        // of a 6.3 s one-package add). When a range does force an older release, the
        // resolver's `needs_fetch` names it and it is fetched then.
        //
        // Deliberately no peerDependencies: advisory, and they explode the frontier
        // (measured: never closed on a real jest tree).
        let newest = versions
            .keys()
            .filter_map(|k| semver::Version::parse(k).ok().map(|v| (v, k)))
            .max_by(|a, b| a.0.cmp(&b.0));
        let Some((_, newest_key)) = newest else {
            continue;
        };
        let Some(info) = versions.get(newest_key.as_str()) else {
            continue;
        };
        {
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

    #[test]
    fn hardening_nested_rewinds_release_and_discards_failed_branch_edges() {
        let world = HashMap::from([
            ("parent".into(), pk("parent", &[("1.0.0", &[]), ("1.1.0", &[("child", "^1")])])),
            ("child".into(), pk("child", &[("1.0.0", &[("missing-major", "^2")])])),
            ("missing-major".into(), pk("missing-major", &[("1.0.0", &[])])),
        ]);
        let roots = HashMap::from([("parent".into(), "^1".into())]);
        let plan = resolve_with_options(&world, &roots, &ResolveOptions::default()).unwrap();
        assert_eq!(plan.instances.len(), 1);
        assert_eq!(plan.instances[0].version, "1.0.0");
        assert_eq!(plan.edges.len(), 1);
        assert!(plan.peers.is_empty());

        let mut incomplete = world;
        incomplete.remove("missing-major");
        let failure = resolve_with_options(&incomplete, &roots, &ResolveOptions::default()).unwrap_err();
        assert_eq!(failure.failure, NestFailure::MissingPackument);
        assert_eq!(failure.package, "missing-major");
    }

    #[test]
    fn hardening_alias_cannot_bypass_registry_identity_policy() {
        let world = HashMap::from([(
            "original".into(),
            pk("original", &[("1.0.0", &[]), ("1.1.0", &[])]),
        )]);
        let roots = HashMap::from([("local-name".into(), "npm:original@^1".into())]);
        let options = ResolveOptions {
            banned: Some([("original".into(), "1.1.0".into())].into()),
            ..ResolveOptions::default()
        };
        let plan = resolve_with_options(&world, &roots, &options).unwrap();
        assert_eq!(plan.instances[0].package, "original");
        assert_eq!(plan.instances[0].installed_as, "local-name");
        assert_eq!(plan.instances[0].version, "1.0.0");
    }

    /// `--as-of` accepts an instant or a calendar date, and refuses anything else
    /// rather than falling back to "today" — the one outcome a reproducibility
    /// input must never produce on its own.
    #[test]
    fn cutoff_parsing_is_strict() {
        assert_eq!(parse_cutoff("2020-06-01"), Some(1_591_055_999));
        assert_eq!(
            parse_cutoff("2020-06-01T00:00:00Z"),
            Some(1_590_969_600)
        );
        for bad in ["", "yesterday", "2020-13-40", "01/06/2020"] {
            assert_eq!(parse_cutoff(bad), None, "accepted {bad:?}");
        }
    }

    /// An `npm:` alias and the real package are two directories, even at the same
    /// version. Reuse must be keyed on the *installed name*, not only on the
    /// version, or the alias is silently materialised under the real name and the
    /// dependent that asked for the alias finds nothing.
    #[test]
    fn alias_does_not_reuse_the_real_name() {
        let packuments = HashMap::from([
            (
                "app".to_string(),
                pk(
                    "app",
                    &[("1.0.0", &[("foo", "^1.0.0"), ("bar", "^1.0.0")])],
                ),
            ),
            (
                "bar".to_string(),
                pk("bar", &[("1.0.0", &[("alias", "npm:foo@^1.0.0")])]),
            ),
            ("foo".to_string(), pk("foo", &[("1.0.0", &[])])),
        ]);
        let roots = HashMap::from([("app".to_string(), "^1.0.0".to_string())]);
        let r = resolve_with_options(&packuments, &roots, &ResolveOptions::default()).unwrap();
        let names: Vec<&str> = r.instances.iter().map(|i| i.installed_as.as_str()).collect();
        assert!(names.contains(&"foo"), "real name missing: {names:?}");
        assert!(
            names.contains(&"alias"),
            "alias was folded into the real name: {names:?}"
        );
        let alias = r
            .instances
            .iter()
            .find(|i| i.installed_as == "alias")
            .expect("alias instance");
        assert_eq!(alias.package, "foo", "alias must resolve to the real package");
        assert_eq!(alias.version, "1.0.0");
    }

    /// Two ranges that overlap must share one directory. The greedy order — first
    /// demand picks the newest, the second then cannot use it — produced a second
    /// instance for a conflict that does not exist.
    #[test]
    fn overlapping_ranges_share_one_instance() {
        let packuments = HashMap::from([
            (
                "app".to_string(),
                pk(
                    "app",
                    // Sorted deps mean `aaa` is processed before `zzz`, so the
                    // wide range picks a version first and the narrow one
                    // arrives second: the unlucky order, on purpose.
                    &[("1.0.0", &[("aaa", "^1.0.0"), ("zzz", "^1.0.0")])],
                ),
            ),
            ("aaa".to_string(), pk("aaa", &[("1.0.0", &[("c", "^1.0.0")])])),
            ("zzz".to_string(), pk("zzz", &[("1.0.0", &[("c", "1.2.3")])])),
            (
                "c".to_string(),
                pk("c", &[("1.2.3", &[]), ("1.9.0", &[])]),
            ),
        ]);
        let roots = HashMap::from([("app".to_string(), "^1.0.0".to_string())]);
        let r = resolve_with_options(&packuments, &roots, &ResolveOptions::default()).unwrap();
        let cs: Vec<&str> = r
            .instances
            .iter()
            .filter(|i| i.package == "c")
            .map(|i| i.version.as_str())
            .collect();
        assert_eq!(cs, vec!["1.2.3"], "one directory satisfies both demands");
    }

    /// Sharing must not quietly downgrade a package nobody competes for: with no
    /// second demand in flight, `^1.0.0` still gets the newest release.
    #[test]
    fn unshared_demand_still_takes_the_newest() {
        let packuments = HashMap::from([
            ("app".to_string(), pk("app", &[("1.0.0", &[("c", "^1.0.0")])])),
            ("c".to_string(), pk("c", &[("1.2.3", &[]), ("1.9.0", &[])])),
        ]);
        let roots = HashMap::from([("app".to_string(), "^1.0.0".to_string())]);
        let r = resolve_with_options(&packuments, &roots, &ResolveOptions::default()).unwrap();
        let c = r.instances.iter().find(|i| i.package == "c").unwrap();
        assert_eq!(c.version, "1.9.0");
    }

    /// The invariant both sharing rules must never break: every demand an
    /// instance records is satisfied by the version that instance ended up with.
    /// Look-ahead may change *which* version is chosen; it may never break the
    /// promise that caused the instance to exist.
    #[test]
    fn every_recorded_demand_is_satisfied() {
        let packuments = HashMap::from([
            (
                "app".to_string(),
                pk(
                    "app",
                    &[(
                        "1.0.0",
                        &[
                            ("aaa", "^1.0.0"),
                            ("zzz", "^1.0.0"),
                            ("pin", "1.0.0"),
                            ("aliased", "npm:foo@^1.0.0"),
                            ("maj", "^1.0.0"),
                        ],
                    )],
                ),
            ),
            ("aaa".to_string(), pk("aaa", &[("1.0.0", &[("c", "^1.0.0")])])),
            ("zzz".to_string(), pk("zzz", &[("1.0.0", &[("c", "1.2.3")])])),
            ("pin".to_string(), pk("pin", &[("1.0.0", &[("c", "^1.0.0")])])),
            ("maj".to_string(), pk("maj", &[("1.0.0", &[("c", "^2.0.0")])])),
            ("foo".to_string(), pk("foo", &[("1.0.0", &[])])),
            (
                "c".to_string(),
                pk("c", &[("1.2.3", &[]), ("1.9.0", &[]), ("2.1.0", &[])]),
            ),
        ]);
        let roots = HashMap::from([("app".to_string(), "^1.0.0".to_string())]);
        let r = resolve_with_options(&packuments, &roots, &ResolveOptions::default()).unwrap();
        assert!(r.instances.len() >= 6, "graph too small to be a test");
        for inst in &r.instances {
            for (dependent, spec) in &inst.required_by {
                let (_, real) = expand_alias(&inst.installed_as, spec);
                let compiled = crate::registry::compile_spec(&real);
                let version = semver::Version::parse(&inst.version).expect("semver");
                assert!(
                    compiled.matches(&version),
                    "{}@{} does not satisfy `{}` demanded by {}",
                    inst.installed_as,
                    inst.version,
                    spec,
                    dependent
                );
            }
        }
        // and the alias is still its own directory at the right version
        let alias = r.instances.iter().find(|i| i.installed_as == "aliased");
        assert_eq!(alias.map(|a| a.version.as_str()), Some("1.0.0"));
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

#[cfg(test)]
mod banned_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    fn world() -> HashMap<String, Value> {
        HashMap::from([(
            "app".to_string(),
            json!({
                "name": "app",
                "versions": {
                    "1.0.0": { "version": "1.0.0" },
                    "1.1.0": { "version": "1.1.0" },
                    "1.2.0": { "version": "1.2.0" }
                }
            }),
        )])
    }

    fn roots() -> HashMap<String, String> {
        HashMap::from([("app".to_string(), "^1".to_string())])
    }

    /// A security exclusion is a resolution input: the newest vulnerable release is
    /// skipped and the newest *clean* one is chosen, without the user having to pin.
    #[test]
    fn banned_versions_are_skipped() {
        let banned: HashSet<(String, String)> = HashSet::from([(
            "app".to_string(),
            "1.2.0".to_string(),
        )]);
        let opts = ResolveOptions {
            banned: Some(banned),
            ..Default::default()
        };
        let r = resolve_with_options(&world(), &roots(), &opts).unwrap();
        assert_eq!(r.instances[0].version, "1.1.0");
    }

    #[test]
    fn banning_every_candidate_is_an_honest_conflict() {
        let banned: HashSet<(String, String)> = ["1.2.0", "1.1.0", "1.0.0"]
            .iter()
            .map(|v| ("app".to_string(), (*v).to_string()))
            .collect();
        let opts = ResolveOptions {
            banned: Some(banned),
            ..Default::default()
        };
        let err = resolve_with_options(&world(), &roots(), &opts).unwrap_err();
        assert_eq!(err.failure, NestFailure::Unsatisfiable);
        assert!(err.describe().contains("published"), "{}", err.describe());
    }
}
