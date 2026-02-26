//! PubGrub solver implementation
//! 
//! Main solving loop with unit propagation, conflict detection, and resolution.

use super::version_set::{PackedVersion, VersionSet, VersionRange};
use super::term::Term;
use super::incompatibility::{Incompatibility, Cause, DerivationTree};
use super::partial_solution::PartialSolution;
use super::vsids::AdaptiveHeuristic;  // JAGR-3: Adaptive heuristic

use std::collections::HashMap;
use std::rc::Rc;

pub type PubGrubResult<T> = Result<T, PubGrubError>;

/// PubGrub solver errors
#[derive(Debug, Clone)]
pub enum PubGrubError {
    /// No solution exists - includes derivation tree for debugging
    NoSolution(DerivationTree),
    /// Solver timed out
    Timeout,
    /// Internal error
    InternalError(String),
    /// Package metadata error
    PackageError(String, String),
}

impl std::fmt::Display for PubGrubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PubGrubError::NoSolution(tree) => write!(f, "No solution: {}", tree),
            PubGrubError::Timeout => write!(f, "Solver timed out"),
            PubGrubError::InternalError(msg) => write!(f, "Internal error: {}", msg),
            PubGrubError::PackageError(pkg, msg) => write!(f, "Package {}: {}", pkg, msg),
        }
    }
}

impl std::error::Error for PubGrubError {}

/// Solution: mapping of package names to versions
pub type Solution = HashMap<String, PackedVersion>;

/// PubGrub dependency solver
pub struct PubGrubSolver {
    /// Incompatibilities indexed by package for fast lookup
    incompatibilities: HashMap<String, Vec<Rc<Incompatibility>>>,
    /// All incompatibilities for iteration
    incompatibility_store: Vec<Rc<Incompatibility>>,
    /// Current partial solution
    partial_solution: PartialSolution,
    /// Available versions for each package
    available_versions: HashMap<String, Vec<PackedVersion>>,
    /// Root package name
    root_package: String,
    /// VSIDS heuristic for variable selection
    vsids: AdaptiveHeuristic,
    /// Maximum decision depth before timeout
    max_decision_depth: usize,
    /// Statistics
    stats: SolverStats,
}

/// Solver statistics for debugging and profiling
#[derive(Debug, Default, Clone)]
pub struct SolverStats {
    pub decisions: usize,
    pub propagations: usize,
    pub conflicts: usize,
    pub backtracks: usize,
    pub incompatibilities_added: usize,
}

impl PubGrubSolver {
    /// Create a new solver for the given root package
    pub fn new(root_package: String) -> Self {
        Self {
            incompatibilities: HashMap::new(),
            incompatibility_store: Vec::new(),
            partial_solution: PartialSolution::new(),
            available_versions: HashMap::new(),
            root_package,
            vsids: AdaptiveHeuristic::new(),
            max_decision_depth: 10000,
            stats: SolverStats::default(),
        }
    }

    /// Set maximum decision depth (prevents infinite loops)
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_decision_depth = max_depth;
        self
    }

    /// Add root package requirements
    pub fn add_root_requirements(&mut self, requirements: HashMap<String, VersionSet>) {
        let mut terms = Vec::new();
        
        for (package, version_set) in &requirements {
            terms.push(Term::allowed(package.clone(), version_set.clone()));
            
            // Create incompatibility: package MUST be in version_set
            let incompat = Rc::new(Incompatibility::new(
                vec![Term::disallowed(package.clone(), version_set.clone())],
                Cause::Root,
            ));
            
            self.incompatibilities
                .entry(package.clone())
                .or_default()
                .push(incompat.clone());
            self.incompatibility_store.push(incompat);
        }
        
        self.partial_solution.add_root_requirements(requirements);
    }

    /// Add root requirements from string specs
    pub fn add_root_requirements_from_specs(
        &mut self,
        specs: HashMap<String, String>,
    ) -> Result<(), PubGrubError> {
        let mut requirements = HashMap::new();
        
        for (package, spec) in specs {
            let version_set = parse_version_spec(&spec)
                .ok_or_else(|| PubGrubError::PackageError(
                    package.clone(),
                    format!("Invalid version spec: {}", spec),
                ))?;
            requirements.insert(package, version_set);
        }
        
        self.add_root_requirements(requirements);
        Ok(())
    }

    /// Set available versions for a package
    pub fn set_available_versions(&mut self, package: &str, versions: Vec<PackedVersion>) {
        self.available_versions.insert(package.to_string(), versions);
    }

    /// Set available versions from string versions
    pub fn set_available_versions_from_strings(
        &mut self,
        package: &str,
        version_strings: Vec<String>,
    ) {
        let versions: Vec<PackedVersion> = version_strings
            .iter()
            .filter_map(|s| PackedVersion::parse(s))
            .collect();
        self.set_available_versions(package, versions);
    }

    /// Main solving loop
    pub fn solve(mut self) -> PubGrubResult<Solution> {
        let timeout = std::time::Instant::now() + std::time::Duration::from_secs(300); // 5 min timeout
        
        loop {
            // Check timeout
            if std::time::Instant::now() > timeout {
                return Err(PubGrubError::Timeout);
            }
            
            // Check decision depth limit
            if self.partial_solution.decision_level() > self.max_decision_depth {
                return Err(PubGrubError::InternalError(
                    "Exceeded maximum decision depth".to_string(),
                ));
            }
            
            // 1. Propagate all incompatibilities
            self.propagate()?;
            
            // 2. Check if solved
            if self.partial_solution.is_solved() {
                return Ok(self.partial_solution.extract_solution());
            }
            
            // 3. Make a decision (choose next package/version)
            let (package, version) = self.choose_package_and_version();
            self.partial_solution.decide(package.clone(), version);
            self.stats.decisions += 1;
            
            // Update VSIDS with conflict information
            let conflict_vars: Vec<String> = vec![package.clone()];
            self.vsids.on_conflict(5.0, &conflict_vars);
            
            // 4. Check for conflicts
            if let Some(conflict) = self.detect_conflict() {
                self.resolve_conflict(conflict)?;
            }
        }
    }

    /// Unit propagation: derive implications from current assignments
    fn propagate(&mut self) -> PubGrubResult<()> {
        loop {
            let mut derived_any = false;
            
            // Clone the store to avoid borrow issues
            let incompatibilities: Vec<_> = self.incompatibility_store.clone();
            
            for incompat in &incompatibilities {
                if let Some(derived) = self.propagate_incompatibility(incompat)? {
                    self.partial_solution.derive(
                        derived.package,
                        derived.version,
                        derived.cause,
                    );
                    derived_any = true;
                    self.stats.propagations += 1;
                }
            }
            
            if !derived_any {
                break;
            }
        }
        Ok(())
    }

    /// Propagate a single incompatibility
    fn propagate_incompatibility(
        &self,
        incompat: &Rc<Incompatibility>,
    ) -> PubGrubResult<Option<DerivedAssignment>> {
        // Count satisfied and unsatisfied terms
        let mut unsatisfied_term = None;
        let mut satisfied_count = 0;
        
        for term in &incompat.terms {
            if self.partial_solution.satisfies_term(term) {
                satisfied_count += 1;
            } else if unsatisfied_term.is_none() {
                unsatisfied_term = Some(term);
            } else {
                // More than one unsatisfied term - can't propagate
                return Ok(None);
            }
        }
        
        // If all but one are satisfied, derive the last one
        if satisfied_count == incompat.terms.len() - 1 {
            if let Some(term) = unsatisfied_term {
                // Get available versions for this package
                let available = self.available_versions.get(&term.package)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                
                // Find a version that satisfies the term
                let version = if term.is_positive {
                    term.version_set.highest(available)
                } else {
                    // For negative terms, find a version NOT in the set
                    available.iter()
                        .find(|v| !term.version_set.contains(v))
                        .copied()
                };
                
                if let Some(version) = version {
                    return Ok(Some(DerivedAssignment {
                        package: term.package.clone(),
                        version,
                        cause: Rc::clone(incompat),
                    }));
                }
            }
        }
        
        Ok(None)
    }

    /// Choose the next package and version to try
    fn choose_package_and_version(&self) -> (String, PackedVersion) {
        // Use VSIDS heuristic if we have activity data
        // Otherwise, use "fewest versions first" heuristic
        
        let mut best_package = None;
        let mut best_version = None;
        let mut min_versions = usize::MAX;
        
        // Get all packages we need to consider
        let mut packages_to_consider: Vec<&String> = self.available_versions.keys().collect();
        
        // Filter out already-decided packages
        packages_to_consider.retain(|pkg| !self.partial_solution.has_decision(pkg));
        
        // Also consider root requirements
        for pkg in self.partial_solution.root_requirements.keys() {
            if !self.partial_solution.has_decision(pkg) {
                packages_to_consider.push(pkg);
            }
        }
        
        // Remove duplicates
        packages_to_consider.sort();
        packages_to_consider.dedup();
        
        for package in packages_to_consider {
            // Get available versions
            let versions = self.available_versions.get(package)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            
            if versions.is_empty() {
                continue;
            }
            
            // Filter to compatible versions
            let compatible_count = versions.iter()
                .filter(|v| self.partial_solution.is_compatible(package, v))
                .count();
            
            if compatible_count == 0 {
                continue;
            }

            let better_domain = compatible_count < min_versions;
            let tied_domain = compatible_count == min_versions;

            let candidates: Vec<String> = vec![package.clone()];
            let vsids_selected = self.vsids.select_variable(&candidates).is_some();
            let current_best_selected = best_package
                .as_ref()
                .map(|p: &String| self.vsids.select_variable(&[p.clone()]).is_some())
                .unwrap_or(false);

            if better_domain || (tied_domain && vsids_selected && !current_best_selected) {
                min_versions = compatible_count;
                best_version = versions
                    .iter()
                    .filter(|v| self.partial_solution.is_compatible(package, v))
                    .max_by(|a, b| a.cmp(b))
                    .copied();
                best_package = Some(package.clone());
            }
        }
        
        // Fallback to root package if nothing found
        let package = best_package.unwrap_or_else(|| self.root_package.clone());
        let version = best_version.unwrap_or_else(|| {
            self.available_versions.get(&package)
                .and_then(|v| v.first())
                .copied()
                .unwrap_or_else(|| PackedVersion { packed: 0 })
        });
        
        (package, version)
    }

    /// Detect if current partial solution conflicts with any incompatibility
    fn detect_conflict(&self) -> Option<Rc<Incompatibility>> {
        for incompat in &self.incompatibility_store {
            if self.partial_solution.conflicts_with(incompat) {
                return Some(incompat.clone());
            }
        }
        None
    }

    /// Resolve a conflict using CDCL
    fn resolve_conflict(&mut self, mut conflict: Rc<Incompatibility>) -> PubGrubResult<()> {
        self.stats.conflicts += 1;
        
        // Collect packages involved in conflict for VSIDS
        let conflict_packages: Vec<String> = conflict.terms.iter()
            .map(|t| t.package.clone())
            .collect();
        
        loop {
            // Find the decision level to backtrack to
            let backtrack_level = self.partial_solution.find_backtrack_level(&conflict);
            
            // Backtrack
            self.partial_solution.backtrack(backtrack_level);
            self.stats.backtracks += 1;
            
            // Learn new incompatibility
            let learned = self.learn_incompatibility(&conflict);
            self.incompatibility_store.push(learned.clone());
            self.stats.incompatibilities_added += 1;
            
            // Add to package index
            for term in &learned.terms {
                self.incompatibilities
                    .entry(term.package.clone())
                    .or_default()
                    .push(learned.clone());
            }
            
            // Check if conflict is resolved
            if !self.partial_solution.conflicts_with(&learned) {
                // Update VSIDS with conflict information
                self.vsids.on_conflict(5.0, &conflict_packages);
                break;
            }
            
            conflict = learned;
        }
        
        Ok(())
    }

    /// Learn a new incompatibility from a conflict
    fn learn_incompatibility(&self, conflict: &Incompatibility) -> Rc<Incompatibility> {
        // Simplified learning: just return the conflict
        // Full implementation would resolve with other incompatibilities

        // Create a derived cause
        let conflict_rc = Rc::new(Incompatibility::new(
            conflict.terms.clone(),
            conflict.cause.clone(),
        ));
        
        let cause = Cause::Derived {
            conflict1: Rc::clone(&conflict_rc),
            conflict2: Rc::clone(&conflict_rc),
        };

        Rc::new(Incompatibility::new(conflict.terms.clone(), cause))
    }

    /// Get solver statistics
    pub fn stats(&self) -> &SolverStats {
        &self.stats
    }
}

/// A derived assignment from propagation
struct DerivedAssignment {
    package: String,
    version: PackedVersion,
    cause: Rc<Incompatibility>,
}

/// Parse a version spec string into a VersionSet
fn parse_version_spec(spec: &str) -> Option<VersionSet> {
    let spec = spec.trim();
    
    // Handle exact versions
    if spec.starts_with('=') {
        let version = spec.trim_start_matches('=');
        return Some(VersionSet::from_range(VersionRange {
            min: PackedVersion::parse(version)?,
            max: PackedVersion::parse(version)?,
            min_inclusive: true,
            max_inclusive: true,
        }));
    }
    
    // Handle semver ranges
    if let Ok(req) = spec.parse::<semver::VersionReq>() {
        return Some(VersionSet::from_req(&req));
    }
    
    // Try parsing as exact version
    PackedVersion::parse(spec).map(|v| VersionSet::from_range(VersionRange {
        min: v,
        max: v,
        min_inclusive: true,
        max_inclusive: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_resolution() {
        let mut solver = PubGrubSolver::new("root".to_string());
        
        let mut requirements = HashMap::new();
        requirements.insert(
            "pkg-a".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("1.0.0").unwrap(),
                max: PackedVersion::parse("2.0.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );
        
        solver.add_root_requirements(requirements);
        solver.set_available_versions_from_strings("pkg-a", vec![
            "1.0.0".to_string(),
            "1.5.0".to_string(),
            "2.0.0".to_string(),
        ]);
        
        let result = solver.solve();
        assert!(result.is_ok());
        let solution = result.unwrap();
        assert!(solution.contains_key("pkg-a"));
    }

    #[test]
    fn test_conflicting_requirements() {
        let mut solver = PubGrubSolver::new("root".to_string());
        
        let mut requirements = HashMap::new();
        requirements.insert(
            "pkg-a".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("1.0.0").unwrap(),
                max: PackedVersion::parse("1.0.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );
        requirements.insert(
            "pkg-a".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("2.0.0").unwrap(),
                max: PackedVersion::parse("2.0.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );
        
        solver.add_root_requirements(requirements);
        
        let result = solver.solve();
        // Should fail with NoSolution error
        assert!(matches!(result, Err(PubGrubError::NoSolution(_))));
    }
}
