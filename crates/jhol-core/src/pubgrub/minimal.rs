//! JAGR-3: Minimal Version Selection
//! 
//! O(n) resolution instead of O(2^n) search.
//! Based on Russ Cox's insight: when constraints specify minimums,
//! the minimum satisfying all is THE answer (no backtracking needed).

use semver::{Version, VersionReq, Comparator, Op};
use std::collections::HashMap;

/// Resolution error types
#[derive(Debug, Clone)]
pub enum ResolutionError {
    Unsatisfiable {
        package: String,
        constraints: Vec<VersionReq>,
    },
    NoMinimumVersion {
        package: String,
    },
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionError::Unsatisfiable { package, constraints } => {
                write!(f, "Cannot resolve {}: conflicting constraints {:?}", package, constraints)
            }
            ResolutionError::NoMinimumVersion { package } => {
                write!(f, "No minimum version found for {}", package)
            }
        }
    }
}

impl std::error::Error for ResolutionError {}

/// Minimal version selector - O(n) instead of O(2^n)
pub struct MinimalVersionSelector {
    /// Track minimum required version per package
    minima: HashMap<String, Version>,
    /// Track all constraints for validation
    constraints: HashMap<String, Vec<VersionReq>>,
}

impl MinimalVersionSelector {
    pub fn new() -> Self {
        Self {
            minima: HashMap::new(),
            constraints: HashMap::new(),
        }
    }
    
    /// Add a version constraint
    pub fn add_constraint(&mut self, package: &str, req: VersionReq) {
        // Extract minimum version from this constraint
        if let Some(min) = req.minimum_version() {
            // Update the maximum of all minimums
            self.minima
                .entry(package.to_string())
                .and_modify(|current| {
                    if min > *current {
                        *current = min.clone();
                    }
                })
                .or_insert(min);
        }
        
        // Store constraint for validation
        self.constraints
            .entry(package.to_string())
            .or_default()
            .push(req);
    }
    
    /// Resolve all packages in O(n) time
    pub fn resolve(self) -> Result<HashMap<String, Version>, ResolutionError> {
        let mut solution = HashMap::new();
        
        for (package, min_version) in self.minima {
            let constraints = self.constraints.get(&package).unwrap();
            
            // Verify this minimum satisfies ALL constraints
            if !constraints.iter().all(|c| c.matches(&min_version)) {
                return Err(ResolutionError::Unsatisfiable {
                    package,
                    constraints: constraints.clone(),
                });
            }
            
            solution.insert(package, min_version);
        }
        
        Ok(solution)
    }
    
    /// Check if we can use minimal selection (all constraints are minimums)
    pub fn is_suitable(&self) -> bool {
        // Minimal selection works when all constraints specify minimums
        // (i.e., no < or <= constraints)
        for constraints in self.constraints.values() {
            for req in constraints {
                for comparator in &req.comparators {
                    match comparator.op {
                        Op::Less | Op::LessEq => {
                            // Has maximum constraint, minimal selection may not work
                            return false;
                        }
                        _ => {}
                    }
                }
            }
        }
        true
    }
}

impl Default for MinimalVersionSelector {
    fn default() -> Self {
        Self::new()
    }
}

/// Extension to extract minimum version from VersionReq
trait VersionReqExt {
    fn minimum_version(&self) -> Option<Version>;
}

impl VersionReqExt for VersionReq {
    fn minimum_version(&self) -> Option<Version> {
        if self.comparators.is_empty() {
            // Wildcard (*) matches everything, minimum is 0.0.0
            return Some(Version::new(0, 0, 0));
        }
        
        // For multiple comparators, find the maximum minimum
        let mut max_min: Option<Version> = None;
        
        for comparator in &self.comparators {
            let min = comparator_minimum(comparator);
            max_min = Some(match max_min {
                None => min,
                Some(current) => if min > current { min } else { current },
            });
        }
        
        max_min
    }
}

/// Extract minimum version from a single comparator
fn comparator_minimum(comparator: &Comparator) -> Version {
    let major = comparator.major;
    let minor = comparator.minor.unwrap_or(0);
    let patch = comparator.patch.unwrap_or(0);
    
    match comparator.op {
        Op::Exact => {
            // Must be exact version
            Version {
                major,
                minor,
                patch,
                pre: comparator.pre.clone(),
                build: semver::BuildMetadata::EMPTY,
            }
        }
        Op::Greater | Op::GreaterEq => {
            // Minimum is specified version
            Version {
                major,
                minor,
                patch,
                pre: comparator.pre.clone(),
                build: semver::BuildMetadata::EMPTY,
            }
        }
        Op::Less | Op::LessEq => {
            // No minimum, just maximum - return 0.0.0
            Version::new(0, 0, 0)
        }
        Op::Tilde => {
            // ~1.2.3 means >=1.2.3, <1.3.0
            // Minimum is the specified version
            Version {
                major,
                minor,
                patch,
                pre: comparator.pre.clone(),
                build: semver::BuildMetadata::EMPTY,
            }
        }
        Op::Caret => {
            // ^1.2.3 means >=1.2.3, <2.0.0
            // Minimum is the specified version
            Version {
                major,
                minor,
                patch,
                pre: comparator.pre.clone(),
                build: semver::BuildMetadata::EMPTY,
            }
        }
        _ => {
            // Unknown operator, return 0.0.0
            Version::new(0, 0, 0)
        }
    }
}

/// Quick check if a set of constraints can be resolved with minimal selection
pub fn can_use_minimal_selection(deps: &HashMap<String, String>) -> bool {
    for spec in deps.values() {
        if let Ok(req) = spec.parse::<VersionReq>() {
            for comparator in &req.comparators {
                match comparator.op {
                    Op::Less | Op::LessEq => {
                        // Maximum constraints require full search
                        return false;
                    }
                    _ => {}
                }
            }
        }
    }
    true
}

/// Fast path resolution using minimal version selection
pub fn resolve_minimal(deps: &HashMap<String, String>) -> Result<HashMap<String, Version>, ResolutionError> {
    let mut selector = MinimalVersionSelector::new();
    
    for (name, spec) in deps {
        if let Ok(req) = spec.parse::<VersionReq>() {
            selector.add_constraint(name, req);
        }
    }
    
    selector.resolve()
}

/// Early conflict detection - detect obvious conflicts before search
pub fn detect_early_conflicts(deps: &HashMap<String, String>) -> Vec<String> {
    let mut conflicts = Vec::new();
    
    // Group constraints by package
    let mut constraints_by_pkg: HashMap<String, Vec<VersionReq>> = HashMap::new();
    for (pkg, spec) in deps {
        if let Ok(req) = spec.parse::<VersionReq>() {
            constraints_by_pkg
                .entry(pkg.clone())
                .or_default()
                .push(req);
        }
    }
    
    // Check for obvious conflicts
    for (pkg, constraints) in constraints_by_pkg {
        // Check 1: Mutually exclusive major versions
        if let Some(reason) = check_mutually_exclusive(&constraints) {
            conflicts.push(format!("{}: {}", pkg, reason));
        }
        
        // Check 2: No version satisfies all constraints
        if let Some(reason) = check_no_common_version(&constraints) {
            conflicts.push(format!("{}: {}", pkg, reason));
        }
    }
    
    conflicts
}

fn check_mutually_exclusive(constraints: &[VersionReq]) -> Option<String> {
    // Example: ^1.0.0 and ^2.0.0 are mutually exclusive
    let mut majors = Vec::new();
    for req in constraints {
        if let Some(comparator) = req.comparators.first() {
            majors.push(comparator.major);
        }
    }
    
    if majors.len() > 1 {
        let min_major = majors.iter().min().unwrap();
        let max_major = majors.iter().max().unwrap();
        
        if max_major > min_major {
            return Some(format!(
                "Conflicting major versions: ^{}.x.x and ^{}.x.x",
                min_major, max_major
            ));
        }
    }
    
    None
}

fn check_no_common_version(constraints: &[VersionReq]) -> Option<String> {
    // Find intersection of all constraints
    let mut intersection: Option<VersionReq> = None;
    for req in constraints {
        intersection = match intersection {
            None => Some(req.clone()),
            Some(current) => {
                // Simplified intersection check
                if let (Some(min1), Some(min2)) = (current.minimum_version(), req.minimum_version()) {
                    if min1 > min2 {
                        Some(current)
                    } else {
                        Some(req.clone())
                    }
                } else {
                    Some(current)
                }
            }
        };
    }
    
    if let Some(final_req) = intersection {
        if final_req.comparators.is_empty() && !constraints.is_empty() {
            return Some("No version satisfies all constraints".to_string());
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_minimal_selection() {
        let mut deps = HashMap::new();
        deps.insert("react".to_string(), "^18.0.0".to_string());
        deps.insert("lodash".to_string(), "^4.17.0".to_string());
        
        let result = resolve_minimal(&deps).unwrap();
        
        assert_eq!(result.get("react").unwrap().major, 18);
        assert_eq!(result.get("lodash").unwrap().major, 4);
    }

    #[test]
    fn test_multiple_constraints() {
        let mut selector = MinimalVersionSelector::new();
        selector.add_constraint("pkg", VersionReq::parse("^1.0.0").unwrap());
        selector.add_constraint("pkg", VersionReq::parse(">=1.2.0").unwrap());
        selector.add_constraint("pkg", VersionReq::parse("~1.1.0").unwrap());
        
        let result = selector.resolve().unwrap();
        
        // Should pick 1.2.0 (maximum of minimums)
        assert_eq!(result.get("pkg").unwrap(), &Version::new(1, 2, 0));
    }

    #[test]
    fn test_conflicting_constraints() {
        let mut selector = MinimalVersionSelector::new();
        selector.add_constraint("pkg", VersionReq::parse("^1.0.0").unwrap());
        selector.add_constraint("pkg", VersionReq::parse("^2.0.0").unwrap());
        
        let result = selector.resolve();
        
        // Should fail (no version satisfies both ^1.0.0 and ^2.0.0)
        assert!(result.is_err());
    }

    #[test]
    fn test_can_use_minimal_selection() {
        let mut deps_good = HashMap::new();
        deps_good.insert("pkg".to_string(), "^1.0.0".to_string());
        deps_good.insert("pkg2".to_string(), ">=2.0.0".to_string());
        assert!(can_use_minimal_selection(&deps_good));
        
        let mut deps_bad = HashMap::new();
        deps_bad.insert("pkg".to_string(), "<2.0.0".to_string());
        assert!(!can_use_minimal_selection(&deps_bad));
    }
}
