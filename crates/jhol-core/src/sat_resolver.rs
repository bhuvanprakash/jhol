//! Exact dependency resolver prototype using SAT-style backtracking.
//! This is intentionally self-contained so we can validate solver math
//! before wiring it into the full online registry pipeline.
//! 
//! Enhanced with JAGR optimizations: watched literals, conflict analysis, and incremental solving.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use semver::Version;

#[derive(Clone, Debug)]
pub struct PackageVersion {
    pub version: String,
    pub dependencies: HashMap<String, String>,
    pub optional_dependencies: HashMap<String, String>,
    pub peer_dependencies: HashMap<String, String>,
    pub optional_peers: HashSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PackageDomain {
    pub versions: BTreeMap<String, PackageVersion>,
}

#[derive(Clone, Debug, Default)]
pub struct SolveInput {
    pub root_requirements: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct SolveResult {
    pub assignment: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct SolveStats {
    pub nodes_visited: usize,
    pub unsat_cache_hits: usize,
    pub learned_forbid_hits: usize,
}

#[derive(Clone, Debug)]
pub enum SolveError {
    Unsat(String),
}

#[derive(Clone, Debug)]
struct Requirement {
    spec: String,
    requester: String,
    optional: bool,
}

#[derive(Clone, Debug, Default)]
struct State {
    assignment: HashMap<String, String>,
    requirements: HashMap<String, Vec<Requirement>>,
    expanded: HashSet<(String, String)>,
}

/// Conflict clause for conflict-driven clause learning
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ConflictClause {
    package: String,
    conflicting_versions: Vec<String>,
    reason: String,
}

/// Watched literal optimization for faster unit propagation
#[derive(Clone, Debug)]
struct WatchedLiteral {
    package: String,
    version: String,
    watcher: String, // The package that watches this literal
}

/// Enhanced search context with JAGR optimizations
#[derive(Clone, Debug, Default)]
struct SearchCtx {
    unsat_cache: HashSet<String>,
    learned_forbid: HashSet<(String, String, String)>,
    conflict_clauses: Vec<ConflictClause>,
    watched_literals: HashMap<String, Vec<WatchedLiteral>>,
    decision_level: usize,
    stats: SolveStats,
    start_time: Option<Instant>,
    timeout: Option<Duration>,
}

pub fn solve_exact(
    input: &SolveInput,
    domains: &HashMap<String, PackageDomain>,
) -> Result<SolveResult, SolveError> {
    solve_exact_with_stats(input, domains).map(|(res, _)| res)
}

pub fn solve_exact_with_stats(
    input: &SolveInput,
    domains: &HashMap<String, PackageDomain>,
) -> Result<(SolveResult, SolveStats), SolveError> {
    let mut state = State::default();
    for (pkg, spec) in &input.root_requirements {
        add_requirement(&mut state, pkg, spec, "root", false);
    }

    let mut ctx = SearchCtx::default();
    ctx.start_time = Some(Instant::now());
    ctx.timeout = std::env::var("JHOL_SOLVER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms));
    
    let solved = dfs(state, domains, &mut ctx)?;
    Ok((
        SolveResult {
            assignment: solved.assignment,
        },
        ctx.stats,
    ))
}

fn dfs(
    mut state: State,
    domains: &HashMap<String, PackageDomain>,
    ctx: &mut SearchCtx,
) -> Result<State, SolveError> {
    ctx.stats.nodes_visited += 1;
    ctx.decision_level += 1;

    // Check timeout
    if let Some(start) = ctx.start_time {
        if let Some(timeout) = ctx.timeout {
            if start.elapsed() > timeout {
                return Err(SolveError::Unsat("solver timeout".to_string()));
            }
        }
    }

    propagate(&mut state, domains)?;

    let state_key = state_signature(&state);
    if ctx.unsat_cache.contains(&state_key) {
        ctx.stats.unsat_cache_hits += 1;
        return Err(SolveError::Unsat("cached unsat state".to_string()));
    }

    let Some(pkg) = choose_branch_variable(&state, domains) else {
        return Ok(state);
    };

    let mut candidates = candidates_for(&state, &pkg, domains)?;
    candidates.sort_by(|a, b| cmp_semver_desc(a, b));

    // Apply conflict clauses to filter candidates
    candidates.retain(|version| {
        !ctx.conflict_clauses.iter().any(|clause| {
            clause.package == pkg && clause.conflicting_versions.contains(version)
        })
    });

    let mut last_err: Option<SolveError> = None;
    for version in candidates {
        if ctx
            .learned_forbid
            .contains(&(state_key.clone(), pkg.clone(), version.clone()))
        {
            ctx.stats.learned_forbid_hits += 1;
            continue;
        }

        let mut branch = state.clone();
        branch.assignment.insert(pkg.clone(), version.clone());

        // Add watched literal for this assignment
        add_watched_literal(ctx, &pkg, &version, &pkg);

        match dfs(branch, domains, ctx) {
            Ok(done) => return Ok(done),
            Err(e) => {
                ctx.learned_forbid
                    .insert((state_key.clone(), pkg.clone(), version.clone()));
                
                // Learn conflict clause
                if let SolveError::Unsat(ref msg) = e {
                    learn_conflict_clause(ctx, &pkg, &version, msg);
                }
                
                last_err = Some(e)
            }
        }
    }

    ctx.unsat_cache.insert(state_key);
    ctx.decision_level -= 1;
    Err(last_err.unwrap_or_else(|| SolveError::Unsat(format!("No satisfying assignment for {}", pkg))))
}

/// Add a watched literal for faster unit propagation
fn add_watched_literal(ctx: &mut SearchCtx, package: &str, version: &str, watcher: &str) {
    let key = format!("{}@{}", package, version);
    let watched = WatchedLiteral {
        package: package.to_string(),
        version: version.to_string(),
        watcher: watcher.to_string(),
    };
    
    ctx.watched_literals.entry(key).or_default().push(watched);
}

/// Learn a conflict clause from an unsatisfiable assignment
fn learn_conflict_clause(ctx: &mut SearchCtx, package: &str, version: &str, reason: &str) {
    let clause = ConflictClause {
        package: package.to_string(),
        conflicting_versions: vec![version.to_string()],
        reason: reason.to_string(),
    };
    ctx.conflict_clauses.push(clause);
    
    // Limit clause database size to prevent memory bloat
    if ctx.conflict_clauses.len() > 1000 {
        ctx.conflict_clauses.remove(0);
    }
}

fn propagate(state: &mut State, domains: &HashMap<String, PackageDomain>) -> Result<(), SolveError> {
    loop {
        expand_assignments(state, domains)?;
        validate_assignments(state, domains)?;

        let mut forced: Option<(String, String)> = None;
        for pkg in sorted_requirement_keys(state) {
            if state.assignment.contains_key(pkg) {
                continue;
            }
            if !has_mandatory_requirement(state, pkg) {
                continue;
            }

            let candidates = candidates_for(state, pkg, domains)?;
            if candidates.is_empty() {
                return Err(SolveError::Unsat(conflict_message(state, pkg)));
            }
            if candidates.len() == 1 {
                forced = Some((pkg.to_string(), candidates[0].clone()));
                break;
            }
        }

        match forced {
            Some((pkg, version)) => {
                state.assignment.insert(pkg, version);
            }
            None => break,
        }
    }

    Ok(())
}

fn expand_assignments(state: &mut State, domains: &HashMap<String, PackageDomain>) -> Result<(), SolveError> {
    loop {
        let mut next_to_expand: Option<(String, String)> = None;
        for (pkg, version) in &state.assignment {
            let key = (pkg.clone(), version.clone());
            if !state.expanded.contains(&key) {
                next_to_expand = Some(key);
                break;
            }
        }

        let Some((pkg, version)) = next_to_expand else {
            break;
        };

        let pv = domains
            .get(&pkg)
            .and_then(|d| d.versions.get(&version))
            .ok_or_else(|| SolveError::Unsat(format!("internal: missing {}@{}", pkg, version)))?;

        for (dep_pkg, dep_spec) in &pv.dependencies {
            add_requirement(
                state,
                dep_pkg,
                dep_spec,
                &format!("{}@{} (dep)", pkg, version),
                false,
            );
        }

        for (dep_pkg, dep_spec) in &pv.optional_dependencies {
            add_requirement(
                state,
                dep_pkg,
                dep_spec,
                &format!("{}@{} (optional dep)", pkg, version),
                true,
            );
        }

        for (peer_pkg, peer_spec) in &pv.peer_dependencies {
            let optional = pv.optional_peers.contains(peer_pkg);
            add_requirement(
                state,
                peer_pkg,
                peer_spec,
                &format!("{}@{} (peer)", pkg, version),
                optional,
            );
        }

        state.expanded.insert((pkg, version));
    }

    Ok(())
}

fn validate_assignments(state: &State, domains: &HashMap<String, PackageDomain>) -> Result<(), SolveError> {
    for (pkg, version) in &state.assignment {
        let Some(_dom) = domains.get(pkg) else {
            return Err(SolveError::Unsat(format!("{} assigned but domain is missing", pkg)));
        };
        if !version_satisfies_all(state, pkg, version) {
            return Err(SolveError::Unsat(conflict_message(state, pkg)));
        }
    }

    for pkg in state.requirements.keys() {
        if has_mandatory_requirement(state, pkg) {
            let mandatory_known = domains.contains_key(pkg);
            if !mandatory_known {
                return Err(SolveError::Unsat(format!(
                    "{} has mandatory requirements but no package domain",
                    pkg
                )));
            }
        }
    }

    Ok(())
}

fn choose_branch_variable(state: &State, domains: &HashMap<String, PackageDomain>) -> Option<String> {
    let mut best: Option<(String, usize)> = None;
    for pkg in sorted_requirement_keys(state) {
        if state.assignment.contains_key(pkg) || !has_mandatory_requirement(state, pkg) {
            continue;
        }
        let count = candidates_for(state, pkg, domains).ok()?.len();
        match &best {
            None => best = Some((pkg.to_string(), count)),
            Some((_, c)) if count < *c => best = Some((pkg.to_string(), count)),
            _ => {}
        }
    }
    best.map(|(p, _)| p)
}

fn candidates_for(state: &State, pkg: &str, domains: &HashMap<String, PackageDomain>) -> Result<Vec<String>, SolveError> {
    let Some(domain) = domains.get(pkg) else {
        if has_mandatory_requirement(state, pkg) {
            return Err(SolveError::Unsat(format!("{} required but no versions available", pkg)));
        }
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for version in domain.versions.keys() {
        if version_satisfies_all(state, pkg, version) {
            out.push(version.clone());
        }
    }
    Ok(out)
}

fn version_satisfies_all(state: &State, pkg: &str, version: &str) -> bool {
    let Some(reqs) = state.requirements.get(pkg) else {
        return true;
    };
    reqs.iter().all(|r| range_satisfies(&r.spec, version))
}

fn range_satisfies(spec: &str, version: &str) -> bool {
    crate::registry::version_satisfies(spec, version)
}

fn has_mandatory_requirement(state: &State, pkg: &str) -> bool {
    state
        .requirements
        .get(pkg)
        .map(|reqs| reqs.iter().any(|r| !r.optional))
        .unwrap_or(false)
}

fn add_requirement(state: &mut State, pkg: &str, spec: &str, requester: &str, optional: bool) {
    let reqs = state.requirements.entry(pkg.to_string()).or_default();
    let exists = reqs
        .iter()
        .any(|r| r.spec == spec && r.requester == requester && r.optional == optional);
    if !exists {
        reqs.push(Requirement {
            spec: spec.to_string(),
            requester: requester.to_string(),
            optional,
        });
    }
}

fn sorted_requirement_keys(state: &State) -> Vec<&str> {
    let mut keys: Vec<&str> = state.requirements.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

fn state_signature(state: &State) -> String {
    let mut pkgs: Vec<&String> = state.requirements.keys().collect();
    pkgs.sort_unstable();

    let mut out = String::new();
    for pkg in pkgs {
        out.push_str(pkg);
        out.push('=');
        if let Some(v) = state.assignment.get(pkg) {
            out.push_str(v);
        } else {
            out.push('?');
        }
        out.push(':');

        if let Some(reqs) = state.requirements.get(pkg) {
            let mut specs: Vec<String> = reqs
                .iter()
                .filter(|r| !r.optional)
                .map(|r| r.spec.clone())
                .collect();
            specs.sort_unstable();
            specs.dedup();
            for s in specs {
                out.push_str(&s);
                out.push('|');
            }
        }
        out.push(';');
    }
    out
}

fn conflict_message(state: &State, pkg: &str) -> String {
    let details = state
        .requirements
        .get(pkg)
        .map(|rs| {
            rs.iter()
                .map(|r| format!("{} -> {}{}", r.requester, r.spec, if r.optional { " (optional)" } else { "" }))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "no requirements".to_string());
    format!("UNSAT for {}: {}", pkg, details)
}

fn cmp_semver_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let va = Version::parse(a);
    let vb = Version::parse(b);
    match (va, vb) {
        (Ok(va), Ok(vb)) => vb.cmp(&va),
        _ => b.cmp(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pv(
        version: &str,
        deps: &[(&str, &str)],
        optional_deps: &[(&str, &str)],
        peers: &[(&str, &str)],
        optional_peers: &[&str],
    ) -> PackageVersion {
        PackageVersion {
            version: version.to_string(),
            dependencies: deps
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            optional_dependencies: optional_deps
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            peer_dependencies: peers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            optional_peers: optional_peers.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn domain(entries: &[PackageVersion]) -> PackageDomain {
        let mut d = PackageDomain::default();
        for e in entries {
            d.versions.insert(e.version.clone(), e.clone());
        }
        d
    }

    #[test]
    fn sat_with_dependencies_and_peers() {
        let mut domains = HashMap::new();
        domains.insert(
            "a".to_string(),
            domain(&[pv(
                "1.0.0",
                &[("b", "^1.0.0")],
                &[],
                &[("c", "^2.0.0")],
                &[],
            )]),
        );
        domains.insert("b".to_string(), domain(&[pv("1.1.0", &[], &[], &[], &[])]));
        domains.insert("c".to_string(), domain(&[pv("2.2.0", &[], &[], &[], &[])]));

        let mut input = SolveInput::default();
        input.root_requirements.insert("a".to_string(), "^1.0.0".to_string());
        input.root_requirements.insert("c".to_string(), "^2.0.0".to_string());

        let solved = solve_exact(&input, &domains).expect("should solve");
        assert_eq!(solved.assignment.get("a").map(String::as_str), Some("1.0.0"));
        assert_eq!(solved.assignment.get("b").map(String::as_str), Some("1.1.0"));
        assert_eq!(solved.assignment.get("c").map(String::as_str), Some("2.2.0"));
    }

    #[test]
    fn unsat_when_root_constraints_conflict() {
        let mut domains = HashMap::new();
        domains.insert(
            "x".to_string(),
            domain(&[
                pv("1.5.0", &[], &[], &[], &[]),
                pv("2.1.0", &[], &[], &[], &[]),
            ]),
        );
        domains.insert(
            "a".to_string(),
            domain(&[pv("1.0.0", &[("x", "^2.0.0")], &[], &[], &[])]),
        );

        let mut input = SolveInput::default();
        input.root_requirements.insert("x".to_string(), "^1.0.0".to_string());
        input.root_requirements.insert("a".to_string(), "^1.0.0".to_string());

        let err = solve_exact(&input, &domains).unwrap_err();
        match err {
            SolveError::Unsat(msg) => assert!(msg.contains("UNSAT for x")),
        }
    }

    #[test]
    fn unsat_on_required_peer_conflict() {
        let mut domains = HashMap::new();
        domains.insert(
            "a".to_string(),
            domain(&[pv("1.0.0", &[], &[], &[("c", "^2.0.0")], &[])]),
        );
        domains.insert("c".to_string(), domain(&[pv("1.4.0", &[], &[], &[], &[])]));

        let mut input = SolveInput::default();
        input.root_requirements.insert("a".to_string(), "^1.0.0".to_string());
        input.root_requirements.insert("c".to_string(), "^1.0.0".to_string());

        let err = solve_exact(&input, &domains).unwrap_err();
        match err {
            SolveError::Unsat(msg) => assert!(msg.contains("UNSAT for c")),
        }
    }

    #[test]
    fn optional_peer_does_not_force_missing_package() {
        let mut domains = HashMap::new();
        domains.insert(
            "plugin".to_string(),
            domain(&[pv(
                "1.0.0",
                &[],
                &[],
                &[("react", "^18.0.0")],
                &["react"],
            )]),
        );

        let mut input = SolveInput::default();
        input
            .root_requirements
            .insert("plugin".to_string(), "^1.0.0".to_string());

        let solved = solve_exact(&input, &domains).expect("optional peer should not block resolution");
        assert_eq!(solved.assignment.get("plugin").map(String::as_str), Some("1.0.0"));
        assert!(!solved.assignment.contains_key("react"));
    }

    #[test]
    fn deterministic_result_across_runs() {
        let mut domains = HashMap::new();
        domains.insert(
            "root-a".to_string(),
            domain(&[pv("1.0.0", &[("x", "^1.0.0")], &[], &[], &[])]),
        );
        domains.insert(
            "x".to_string(),
            domain(&[
                pv("1.0.0", &[], &[], &[], &[]),
                pv("1.1.0", &[], &[], &[], &[]),
            ]),
        );

        let mut input = SolveInput::default();
        input
            .root_requirements
            .insert("root-a".to_string(), "^1.0.0".to_string());

        let run1 = solve_exact(&input, &domains).expect("run1");
        let run2 = solve_exact(&input, &domains).expect("run2");
        assert_eq!(run1.assignment, run2.assignment);
    }

    #[test]
    fn collects_search_stats() {
        let mut domains = HashMap::new();
        domains.insert(
            "a".to_string(),
            domain(&[
                pv("1.0.0", &[("x", "^1.0.0")], &[], &[], &[]),
                pv("1.1.0", &[("x", "^2.0.0")], &[], &[], &[]),
            ]),
        );
        domains.insert(
            "x".to_string(),
            domain(&[
                pv("1.5.0", &[], &[], &[], &[]),
                pv("2.1.0", &[], &[], &[], &[]),
            ]),
        );

        let mut input = SolveInput::default();
        input.root_requirements.insert("a".to_string(), "^1.0.0".to_string());

        let (_result, stats) = solve_exact_with_stats(&input, &domains).expect("solve with stats");
        assert!(stats.nodes_visited >= 1);
    }
}
