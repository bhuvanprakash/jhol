//! Partial solution tracking for PubGrub solver
//! 
//! Tracks the current state of the solver including assignments,
//! decision levels, and causes for backtracking.

use super::term::Term;
use super::version_set::{PackedVersion, VersionSet};
use super::incompatibility::Incompatibility;
use std::rc::Rc;
use std::collections::HashMap;

/// An assignment made during solving
#[derive(Clone, Debug)]
pub struct Assignment {
    pub package: String,
    pub version: PackedVersion,
    pub decision_level: usize,
    pub cause: Option<Rc<Incompatibility>>,
}

/// Partial solution during resolution
/// Tracks all assignments and their causes for conflict analysis
pub struct PartialSolution {
    /// All assignments in order they were made
    assignments: Vec<Assignment>,
    
    /// Current decision level (increments with each decision)
    decision_level: usize,
    
    /// Previous decision level (for backtracking)
    previous_level: usize,
    
    /// Current assignment for each package
    current_assignment: HashMap<String, PackedVersion>,
    
    /// Root requirements that must be satisfied
    pub root_requirements: HashMap<String, VersionSet>,
}

impl PartialSolution {
    /// Create a new empty partial solution
    pub fn new() -> Self {
        Self {
            assignments: Vec::new(),
            decision_level: 0,
            previous_level: 0,
            current_assignment: HashMap::new(),
            root_requirements: HashMap::new(),
        }
    }

    /// Add root package requirements
    pub fn add_root_requirements(&mut self, requirements: HashMap<String, VersionSet>) {
        for (package, version_set) in requirements {
            self.root_requirements.insert(package, version_set);
        }
    }

    /// Make a decision (assign a package to a version)
    pub fn decide(&mut self, package: String, version: PackedVersion) {
        self.decision_level += 1;
        self.assignments.push(Assignment {
            package: package.clone(),
            version,
            decision_level: self.decision_level,
            cause: None,
        });
        self.current_assignment.insert(package, version);
    }

    /// Derive an assignment from propagation (not a decision)
    pub fn derive(
        &mut self,
        package: String,
        version: PackedVersion,
        cause: Rc<Incompatibility>,
    ) {
        self.assignments.push(Assignment {
            package: package.clone(),
            version,
            decision_level: self.decision_level,
            cause: Some(cause),
        });
        self.current_assignment.insert(package, version);
    }

    /// Check if a package has been assigned
    pub fn has_decision(&self, package: &str) -> bool {
        self.current_assignment.contains_key(package)
    }

    /// Get the assigned version for a package
    pub fn get_assignment(&self, package: &str) -> Option<&PackedVersion> {
        self.current_assignment.get(package)
    }

    /// Check if the solution is complete (all root requirements satisfied)
    pub fn is_solved(&self) -> bool {
        // Check if all root requirements have assignments
        self.root_requirements.iter().all(|(package, version_set)| {
            self.current_assignment
                .get(package)
                .map(|v| version_set.contains(v))
                .unwrap_or(false)
        })
    }

    /// Check if a term is satisfied by current assignments
    pub fn satisfies_term(&self, term: &Term) -> bool {
        if let Some(assigned_version) = self.current_assignment.get(&term.package) {
            term.satisfies(assigned_version)
        } else {
            // Unassigned packages don't satisfy positive terms
            !term.is_positive
        }
    }

    /// Check if a version is compatible with current assignments for a package
    pub fn is_compatible(&self, package: &str, version: &PackedVersion) -> bool {
        // Check against root requirements
        if let Some(req) = self.root_requirements.get(package) {
            if !req.contains(version) {
                return false;
            }
        }
        
        // Check against current assignment (if any)
        if let Some(assigned) = self.current_assignment.get(package) {
            return assigned == version;
        }
        
        true
    }

    /// Check if current assignments conflict with an incompatibility
    pub fn conflicts_with(&self, incompat: &Incompatibility) -> bool {
        // A conflict occurs when all terms in the incompatibility are satisfied
        // (which is impossible by definition of incompatibility)
        incompat.terms.iter().all(|term| self.satisfies_term(term))
    }

    /// Find the decision level to backtrack to for a conflict
    pub fn find_backtrack_level(&self, conflict: &Incompatibility) -> usize {
        // Find the highest decision level among the conflict terms
        let mut max_level = 0;
        
        for term in &conflict.terms {
            for assignment in &self.assignments {
                if assignment.package == term.package {
                    max_level = max_level.max(assignment.decision_level);
                }
            }
        }
        
        // Backtrack to the second-highest decision level
        max_level.saturating_sub(1)
    }

    /// Backtrack to a specific decision level
    pub fn backtrack(&mut self, level: usize) {
        self.previous_level = self.decision_level;
        self.decision_level = level;
        
        // Remove assignments made after the target level
        self.assignments.retain(|a| a.decision_level <= level);
        
        // Rebuild current assignment from remaining assignments
        self.current_assignment.clear();
        for assignment in &self.assignments {
            self.current_assignment.insert(assignment.package.clone(), assignment.version);
        }
    }

    /// Extract the final solution
    pub fn extract_solution(&self) -> HashMap<String, PackedVersion> {
        self.current_assignment.clone()
    }

    /// Get the number of assignments
    pub fn assignment_count(&self) -> usize {
        self.assignments.len()
    }

    /// Get the current decision level
    pub fn decision_level(&self) -> usize {
        self.decision_level
    }

    /// Get all assignments for debugging
    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }
}

impl Default for PartialSolution {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::version_set::VersionRange;

    #[test]
    fn test_decide_and_derive() {
        let mut solution = PartialSolution::new();
        
        // Make a decision
        solution.decide("pkg-a".to_string(), PackedVersion::parse("1.0.0").unwrap());
        assert_eq!(solution.decision_level(), 1);
        assert!(solution.has_decision("pkg-a"));
        
        // Derive another assignment
        let incompat = Rc::new(Incompatibility::new(
            vec![],
            super::incompatibility::Cause::Root,
        ));
        solution.derive(
            "pkg-b".to_string(),
            PackedVersion::parse("2.0.0").unwrap(),
            incompat,
        );
        assert!(solution.has_decision("pkg-b"));
    }

    #[test]
    fn test_backtrack() {
        let mut solution = PartialSolution::new();
        
        solution.decide("pkg-a".to_string(), PackedVersion::parse("1.0.0").unwrap());
        solution.decide("pkg-b".to_string(), PackedVersion::parse("2.0.0").unwrap());
        solution.decide("pkg-c".to_string(), PackedVersion::parse("3.0.0").unwrap());
        
        assert_eq!(solution.decision_level(), 3);
        assert_eq!(solution.assignment_count(), 3);
        
        // Backtrack to level 1
        solution.backtrack(1);
        
        assert_eq!(solution.decision_level(), 1);
        assert!(solution.has_decision("pkg-a"));
        assert!(!solution.has_decision("pkg-b"));
        assert!(!solution.has_decision("pkg-c"));
    }

    #[test]
    fn test_satisfies_term() {
        let mut solution = PartialSolution::new();
        solution.decide("pkg".to_string(), PackedVersion::parse("1.5.0").unwrap());
        
        let term = Term::allowed(
            "pkg".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("1.0.0").unwrap(),
                max: PackedVersion::parse("2.0.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );
        
        assert!(solution.satisfies_term(&term));
        
        let negative_term = Term::disallowed(
            "pkg".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("1.0.0").unwrap(),
                max: PackedVersion::parse("2.0.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );
        
        assert!(!solution.satisfies_term(&negative_term));
    }
}
