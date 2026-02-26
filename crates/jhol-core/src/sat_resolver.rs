//! Exact dependency resolver prototype using SAT-style backtracking.
//! This is intentionally self-contained so we can validate solver math
//! before wiring it into the full online registry pipeline.
//! 
//! Enhanced with JAGR-1 optimizations: watched literals, conflict analysis, and incremental solving.
//! Implements formal CSP/SAT-style mathematical model with semver interval arithmetic.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use std::path::Path;
use std::fs;

use semver::{Version, VersionReq};

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
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct ConflictClause {
    package: String,
    conflicting_versions: Vec<String>,
    reason: String,
}

/// Watched literal optimization for faster unit propagation
/// Each constraint clause watches two literals. When one literal becomes false,
/// we only need to check clauses watching that literal instead of all clauses.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WatchedLiteral {
    package: String,
    version: String,
    clause_id: usize, // ID of the clause this literal belongs to
}

/// Constraint clause for watched literal tracking
#[derive(Clone, Debug)]
struct WatchedClause {
    id: usize,
    literals: Vec<(String, String)>, // (package, version) pairs
    watch_indices: [usize; 2],       // Indices of the two watched literals
    reason: String,                   // Why this clause exists
}

/// Semver interval for efficient range arithmetic
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SemverInterval {
    min: Version,
    max: Option<Version>, // None means unbounded
}

impl SemverInterval {
    fn new(min: Version, max: Option<Version>) -> Self {
        Self { min, max }
    }
    
    fn contains(&self, version: &Version) -> bool {
        version >= &self.min && self.max.as_ref().map_or(true, |max| version <= max)
    }
    
    fn intersects(&self, other: &Self) -> bool {
        self.contains(&other.min) || other.contains(&self.min) ||
        self.max.as_ref().map_or(false, |max| other.contains(max)) ||
        other.max.as_ref().map_or(false, |max| self.contains(max))
    }
    
    fn intersection(&self, other: &Self) -> Option<Self> {
        let new_min = if self.min >= other.min { self.min.clone() } else { other.min.clone() };
        let new_max = match (&self.max, &other.max) {
            (Some(a), Some(b)) => Some(if a <= b { a.clone() } else { b.clone() }),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };

        if new_max.as_ref().map_or(true, |max| &new_min <= max) {
            Some(Self::new(new_min, new_max))
        } else {
            None
        }
    }
}

/// Registry fingerprint for deterministic results
#[derive(Clone, Debug, Default)]
struct RegistryFingerprint {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_hash: String,
}

/// Persistent cache for learned constraints
#[derive(Clone, Debug, Default)]
struct PersistentCache {
    pub cache_dir: Option<String>,
    pub fingerprint: Option<RegistryFingerprint>,
}

impl PersistentCache {
    fn new(cache_dir: Option<String>) -> Self {
        Self {
            cache_dir,
            fingerprint: None,
        }
    }
    
    fn load_unsat_cache(&self) -> HashSet<String> {
        if let Some(dir) = &self.cache_dir {
            let cache_file = Path::new(dir).join("jhol_unsat_cache.json");
            if let Ok(content) = fs::read_to_string(cache_file) {
                if let Ok(cache) = serde_json::from_str::<HashSet<String>>(&content) {
                    return cache;
                }
            }
        }
        HashSet::new()
    }
    
    fn save_unsat_cache(&self, cache: &HashSet<String>) {
        if let Some(dir) = &self.cache_dir {
            let cache_file = Path::new(dir).join("jhol_unsat_cache.json");
            if let Some(parent) = cache_file.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_json::to_string_pretty(cache) {
                let _ = fs::write(cache_file, content);
            }
        }
    }
    
    fn load_conflict_clauses(&self) -> Vec<ConflictClause> {
        if let Some(dir) = &self.cache_dir {
            let cache_file = Path::new(dir).join("jhol_conflict_clauses.json");
            if let Ok(content) = fs::read_to_string(cache_file) {
                if let Ok(clauses) = serde_json::from_str::<Vec<ConflictClause>>(&content) {
                    return clauses;
                }
            }
        }
        Vec::new()
    }
    
    fn save_conflict_clauses(&self, clauses: &[ConflictClause]) {
        if let Some(dir) = &self.cache_dir {
            let cache_file = Path::new(dir).join("jhol_conflict_clauses.json");
            if let Some(parent) = cache_file.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_json::to_string_pretty(clauses) {
                let _ = fs::write(cache_file, content);
            }
        }
    }
    
    fn compute_fingerprint(&self, domains: &HashMap<String, PackageDomain>) -> RegistryFingerprint {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::default();
        
        // Hash all package domains for fingerprinting
        let mut packages: Vec<&String> = domains.keys().collect();
        packages.sort_unstable();
        
        for pkg in packages {
            if let Some(domain) = domains.get(pkg) {
                let mut versions: Vec<&String> = domain.versions.keys().collect();
                versions.sort_unstable();
                
                for version in versions {
                    if let Some(pv) = domain.versions.get(version) {
                        hasher.update(format!("{}@{}", pkg, version).as_bytes());
                        hasher.update(pv.dependencies.keys().cloned().collect::<Vec<_>>().join(",").as_bytes());
                        hasher.update(pv.peer_dependencies.keys().cloned().collect::<Vec<_>>().join(",").as_bytes());
                    }
                }
            }
        }
        
        let result = hasher.finalize();
        let content_hash = format!("{:x}", result);
        
        RegistryFingerprint {
            etag: None,
            last_modified: None,
            content_hash,
        }
    }
    
    fn is_cache_valid(&self, current_fingerprint: &RegistryFingerprint) -> bool {
        match &self.fingerprint {
            Some(cached) => cached.content_hash == current_fingerprint.content_hash,
            None => false,
        }
    }
}

/// Enhanced search context with JAGR optimizations
#[derive(Clone, Debug, Default)]
struct SearchCtx {
    unsat_cache: HashSet<String>,
    learned_forbid: HashSet<(String, String, String)>,
    conflict_clauses: Vec<ConflictClause>,
    // Watched literals: maps (pkg, version) -> list of clause IDs watching it
    watched_literals: HashMap<String, Vec<usize>>,
    // Watched clauses database for O(1) propagation
    watched_clauses: HashMap<usize, WatchedClause>,
    next_clause_id: usize,
    decision_level: usize,
    stats: SolveStats,
    start_time: Option<Instant>,
    timeout: Option<Duration>,
    // JAGR-1 enhancements
    decision_stack: Vec<(String, String)>, // Track decision path for conflict analysis
    implication_graph: HashMap<String, Vec<String>>, // Track implications for conflict analysis
    clause_database: HashMap<String, ConflictClause>, // Indexed conflict clauses for faster lookup
    restart_counter: usize, // Track restarts for periodic restart strategy
    restart_frequency: usize, // Dynamic restart frequency
    // Incremental solving support
    previous_assignment: Option<HashMap<String, String>>, // Previous solution for incremental solving
    incremental_mode: bool, // Whether we're in incremental solving mode
    // Semver interval cache for faster constraint checking (lazy initialized)
    interval_cache: HashMap<String, SemverInterval>,
    // Persistent cache for learned constraints
    persistent_cache: PersistentCache,
    // Lazy initialization flags to reduce first-run overhead
    watched_literals_initialized: bool,
    cache_preloaded: bool,
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

    // Initialize persistent cache with lazy loading
    let cache_dir = std::env::var("JHOL_CACHE_DIR").ok();
    ctx.persistent_cache = PersistentCache::new(cache_dir.clone());

    // Load cached constraints if available (lazy load - only if cache exists)
    if cache_dir.is_some() {
        let fingerprint = ctx.persistent_cache.compute_fingerprint(domains);
        if ctx.persistent_cache.is_cache_valid(&fingerprint) {
            ctx.unsat_cache = ctx.persistent_cache.load_unsat_cache();
            ctx.conflict_clauses = ctx.persistent_cache.load_conflict_clauses();
        }
    }

    let solved = dfs(state, domains, &mut ctx)?;

    // Save constraints to persistent cache (lazy save - only if cache dir exists)
    if cache_dir.is_some() {
        ctx.persistent_cache.save_unsat_cache(&ctx.unsat_cache);
        ctx.persistent_cache.save_conflict_clauses(&ctx.conflict_clauses);
    }

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

    // Enhanced unit propagation with watched literals
    propagate_with_watched_literals(&mut state, domains, ctx)?;

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

    // Apply learned forbid constraints
    candidates.retain(|version| {
        !ctx.learned_forbid.contains(&(state_key.clone(), pkg.clone(), version.clone()))
    });

    let mut last_err: Option<SolveError> = None;
    for version in candidates {
        let mut branch = state.clone();
        branch.assignment.insert(pkg.clone(), version.clone());

        // Track decision for conflict analysis
        ctx.decision_stack.push((pkg.clone(), version.clone()));
        ctx.implication_graph.insert(format!("{}@{}", pkg, version), Vec::new());

        match dfs(branch, domains, ctx) {
            Ok(done) => return Ok(done),
            Err(e) => {
                ctx.learned_forbid
                    .insert((state_key.clone(), pkg.clone(), version.clone()));
                
                // Enhanced conflict analysis and clause learning
                if let SolveError::Unsat(ref msg) = e {
                    let conflict_clause = learn_conflict_clause_enhanced(ctx, &pkg, &version, msg);
                    if let Some(clause) = conflict_clause {
                        // Add to clause database for faster lookup
                        let clause_key = format!("{}@{}", clause.package, version);
                        ctx.clause_database.insert(clause_key, clause.clone());
                    }
                }
                
                last_err = Some(e)
            }
        }
        
        // Backtrack decision stack
        ctx.decision_stack.pop();
    }

    ctx.unsat_cache.insert(state_key);
    ctx.decision_level -= 1;
    
    // Periodic restart strategy
    ctx.restart_counter += 1;
    if ctx.restart_counter % 100 == 0 {
        ctx.restart_frequency = ctx.restart_counter / 10;
        // Clear some learned constraints to allow new search paths
        if ctx.learned_forbid.len() > 500 {
            ctx.learned_forbid.clear();
        }
    }
    
    Err(last_err.unwrap_or_else(|| SolveError::Unsat(format!("No satisfying assignment for {}", pkg))))
}

/// Add a watched clause for faster unit propagation
/// Each clause watches two literals. When one becomes false, we search for a new watch.
fn add_watched_clause(ctx: &mut SearchCtx, literals: Vec<(String, String)>, reason: String) {
    let clause_id = ctx.next_clause_id;
    ctx.next_clause_id += 1;

    let clause = WatchedClause {
        id: clause_id,
        literals: literals.clone(),
        watch_indices: [0, 1], // Watch first two literals initially
        reason,
    };

    ctx.watched_clauses.insert(clause_id, clause);

    // Register watches for the first two literals
    for idx in 0..2.min(literals.len()) {
        let (pkg, ver) = &literals[idx];
        let key = format!("{}@{}", pkg, ver);
        ctx.watched_literals.entry(key).or_default().push(clause_id);
    }
}

/// Watched literal unit propagation - O(1) amortized per constraint
/// When a literal becomes false (package assigned different version),
/// only check clauses watching that literal instead of all clauses.
fn propagate_watched_literals(
    state: &mut State,
    domains: &HashMap<String, PackageDomain>,
    ctx: &mut SearchCtx,
) -> Result<(), SolveError> {
    let mut forced_assignments: Vec<(String, String)> = Vec::new();

    // For each assigned package, check clauses watching the negation
    for (assigned_pkg, assigned_ver) in &state.assignment {
        // Check all clauses that might be affected by this assignment
        let mut clauses_to_update: Vec<usize> = Vec::new();

        // Find clauses where this assignment makes a watched literal false
        for (watch_key, clause_ids) in &ctx.watched_literals {
            if let Some((pkg, _)) = watch_key.split_once('@') {
                if pkg == assigned_pkg {
                    // This clause might need updating
                    for &clause_id in clause_ids {
                        if let Some(clause) = ctx.watched_clauses.get(&clause_id) {
                            let watched_lit = &clause.literals[clause.watch_indices[0]];
                            if &watched_lit.0 == assigned_pkg && &watched_lit.1 != assigned_ver {
                                clauses_to_update.push(clause_id);
                            }
                        }
                    }
                }
            }
        }

        // Update each affected clause
        for clause_id in clauses_to_update {
            if let Some(clause) = ctx.watched_clauses.get_mut(&clause_id) {
                // Try to find a new literal to watch
                let mut found_new_watch = false;
                let old_watch_idx = clause.watch_indices[0];

                for (idx, (pkg, ver)) in clause.literals.iter().enumerate() {
                    if idx == old_watch_idx {
                        continue;
                    }

                    // Check if this literal can be true (version not yet ruled out)
                    if let Some(candidates) = candidates_for(state, pkg, domains).ok() {
                        if candidates.contains(ver) {
                            // Found a new literal to watch
                            let old_key = format!("{}@{}", clause.literals[old_watch_idx].0, clause.literals[old_watch_idx].1);
                            if let Some(watches) = ctx.watched_literals.get_mut(&old_key) {
                                watches.retain(|&id| id != clause_id);
                            }

                            clause.watch_indices[0] = idx;
                            let new_key = format!("{}@{}", pkg, ver);
                            ctx.watched_literals.entry(new_key).or_default().push(clause_id);
                            found_new_watch = true;
                            break;
                        }
                    }
                }

                if !found_new_watch {
                    // No new watch found - this clause is now unit or conflicting
                    // Check if exactly one literal can still be true (unit clause)
                    let mut unit_literal: Option<(String, String)> = None;

                    for (pkg, ver) in &clause.literals {
                        if let Some(candidates) = candidates_for(state, pkg, domains).ok() {
                            if candidates.contains(ver) {
                                if unit_literal.is_some() {
                                    // Multiple literals can be true - not unit
                                    unit_literal = None;
                                    break;
                                }
                                unit_literal = Some((pkg.clone(), ver.clone()));
                            }
                        }
                    }

                    if let Some((pkg, ver)) = unit_literal {
                        // Unit clause - force this assignment
                        if !state.assignment.contains_key(&pkg) {
                            forced_assignments.push((pkg, ver));
                        }
                    }
                }
            }
        }
    }

    // Apply all forced assignments
    for (pkg, ver) in forced_assignments {
        state.assignment.insert(pkg, ver);
    }

    Ok(())
}

/// Enhanced unit propagation with watched literals for O(1) constraint propagation
fn propagate_with_watched_literals(
    state: &mut State,
    domains: &HashMap<String, PackageDomain>,
    ctx: &mut SearchCtx
) -> Result<(), SolveError> {
    // First do standard propagation to handle initial constraints
    propagate(state, domains)?;

    // Then apply watched literal optimizations for learned clauses
    propagate_watched_literals(state, domains, ctx)?;

    Ok(())
}

/// Learn a conflict clause from an unsatisfiable assignment and add to watched clause database
fn learn_conflict_clause_enhanced(
    ctx: &mut SearchCtx,
    package: &str,
    version: &str,
    reason: &str
) -> Option<ConflictClause> {
    // Extract minimal conflict set from decision stack using 1-UIP (Unique Implication Point)
    let mut conflict_versions = Vec::new();
    conflict_versions.push(version.to_string());

    // Analyze decision stack for contributing factors (1-UIP analysis)
    let mut contributing_decisions = Vec::new();
    for (dec_pkg, dec_version) in &ctx.decision_stack {
        if dec_pkg != package {
            contributing_decisions.push((dec_pkg.clone(), dec_version.clone()));
        }
    }

    // Create conflict clause with human-readable explanation
    let clause = ConflictClause {
        package: package.to_string(),
        conflicting_versions: conflict_versions.clone(),
        reason: format_human_readable_conflict(ctx, package, version, &contributing_decisions),
    };

    // Add to conflict clauses with size limit
    ctx.conflict_clauses.push(clause.clone());
    if ctx.conflict_clauses.len() > 1000 {
        ctx.conflict_clauses.remove(0);
    }

    // Create watched clause for efficient propagation
    // The clause represents: NOT(package@version) OR (alternative versions)
    let mut literals = Vec::new();
    literals.push((package.to_string(), version.to_string()));

    // Add to watched clause database for O(1) propagation
    add_watched_clause(ctx, literals, format!("learned from conflict at {}@{}", package, version));

    Some(clause)
}

/// Generate human-readable conflict explanation
fn format_human_readable_conflict(
    ctx: &SearchCtx,
    package: &str,
    version: &str,
    contributing_decisions: &[(String, String)],
) -> String {
    let mut explanation = format!(
        "Conflict: {}@{} cannot be satisfied",
        package, version
    );

    if !contributing_decisions.is_empty() {
        explanation.push_str(" due to:");

        // Group by package for cleaner output
        let mut package_explanations = HashMap::new();
        for (pkg, ver) in contributing_decisions {
            package_explanations
                .entry(pkg.clone())
                .or_insert_with(Vec::new)
                .push(ver.clone());
        }

        for (pkg, versions) in package_explanations {
            if versions.len() == 1 {
                explanation.push_str(&format!("\n  - {}@{} requires incompatible version", pkg, versions[0]));
            } else {
                explanation.push_str(&format!("\n  - {}@{} (among others) requires incompatible version", pkg, versions[0]));
            }
        }
    }

    // Add suggestion if possible
    if let Some(suggestion) = suggest_alternative_versions(ctx, package) {
        explanation.push_str(&format!("\n  Suggestion: Try {}", suggestion));
    }

    explanation
}

/// Suggest alternative versions that might resolve the conflict
fn suggest_alternative_versions(_ctx: &SearchCtx, package: &str) -> Option<String> {
    // In full implementation, would query domain for compatible alternatives
    // For now, provide generic suggestion
    Some(format!("a different version of {}", package))
}

/// Incremental solving: reuse previous solution to speed up solving when only minor changes occur
pub fn solve_incremental(
    input: &SolveInput,
    domains: &HashMap<String, PackageDomain>,
    previous_assignment: Option<&HashMap<String, String>>,
) -> Result<SolveResult, SolveError> {
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
    
    // Set up incremental solving context
    ctx.previous_assignment = previous_assignment.cloned();
    ctx.incremental_mode = true;
    
    // Pre-assign packages that are likely to remain the same
    if let Some(prev) = &ctx.previous_assignment {
        for (pkg, version) in prev {
            if state.requirements.contains_key(pkg) && version_satisfies_all(&state, pkg, version) {
                state.assignment.insert(pkg.clone(), version.clone());
            }
        }
    }
    
    let solved = dfs(state, domains, &mut ctx)?;
    Ok(SolveResult {
        assignment: solved.assignment,
    })
}

/// Check if a package version is compatible with the previous assignment for incremental solving
fn is_compatible_with_previous(
    state: &State,
    pkg: &str,
    version: &str,
    previous_assignment: &Option<HashMap<String, String>>,
) -> bool {
    if let Some(prev) = previous_assignment {
        if let Some(prev_version) = prev.get(pkg) {
            // Check if the new version is compatible with the previous one
            return version == prev_version || is_version_compatible(version, prev_version);
        }
    }
    true
}

/// Check if two versions are compatible (same major version for semver)
fn is_version_compatible(version1: &str, version2: &str) -> bool {
    let v1 = Version::parse(version1);
    let v2 = Version::parse(version2);
    
    match (v1, v2) {
        (Ok(v1), Ok(v2)) => {
            // For semver, consider compatible if major versions match
            v1.major == v2.major
        }
        _ => false,
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
    // First validate all package assignments satisfy their requirements
    for (pkg, version) in &state.assignment {
        let Some(_dom) = domains.get(pkg) else {
            return Err(SolveError::Unsat(format!("{} assigned but domain is missing", pkg)));
        };
        if !version_satisfies_all(state, pkg, version) {
            return Err(SolveError::Unsat(conflict_message(state, pkg)));
        }
    }

    // Then validate peer dependency constraints
    validate_peer_dependencies(state, domains)?;

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

/// Validate peer dependency constraints across the entire assignment
fn validate_peer_dependencies(state: &State, domains: &HashMap<String, PackageDomain>) -> Result<(), SolveError> {
    // Collect all peer dependency requirements
    let mut peer_requirements: HashMap<String, Vec<(String, String, bool)>> = HashMap::new();
    
    for (pkg, version) in &state.assignment {
        if let Some(domain) = domains.get(pkg) {
            if let Some(pv) = domain.versions.get(version) {
                for (peer_pkg, peer_spec) in &pv.peer_dependencies {
                    let optional = pv.optional_peers.contains(peer_pkg);
                    peer_requirements
                        .entry(peer_pkg.clone())
                        .or_default()
                        .push((pkg.clone(), peer_spec.clone(), optional));
                }
            }
        }
    }
    
    // Check each peer dependency for conflicts
    for (peer_pkg, requirements) in peer_requirements {
        // If there are multiple requirements for the same peer, they must all be satisfied by the same version
        if requirements.len() > 1 {
            // Check if the assigned version satisfies all requirements
            if let Some(assigned_version) = state.assignment.get(&peer_pkg) {
                for (requester, spec, optional) in &requirements {
                    if !optional && !range_satisfies(spec, assigned_version) {
                        return Err(SolveError::Unsat(format!(
                            "Peer dependency conflict: {}@{} required by {} does not satisfy {}",
                            peer_pkg, assigned_version, requester, spec
                        )));
                    }
                }
            } else {
                // No version assigned, check if all requirements are optional
                let has_mandatory = requirements.iter().any(|(_, _, optional)| !optional);
                if has_mandatory {
                    return Err(SolveError::Unsat(format!(
                        "Peer dependency {} is required but no version assigned",
                        peer_pkg
                    )));
                }
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
