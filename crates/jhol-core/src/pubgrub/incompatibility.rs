//! Incompatibilities and derivation trees for PubGrub solver
//! 
//! An incompatibility is a set of terms that cannot all be true simultaneously.
//! Derivation trees provide human-readable error explanations.

use super::term::Term;
use std::rc::Rc;
use std::fmt;

/// Why an incompatibility exists
#[derive(Clone, Debug)]
pub enum Cause {
    /// Root package requirements
    Root,
    
    /// Package dependency: pkg1 depends on pkg2
    Dependency {
        package: String,
        dependent: String,
    },
    
    /// User-specified constraints (overrides, resolutions)
    Custom(String),
    
    /// Derived from two other incompatibilities (conflict resolution)
    Derived {
        conflict1: Rc<Incompatibility>,
        conflict2: Rc<Incompatibility>,
    },
}

/// An incompatibility: a set of terms that cannot all be true
#[derive(Clone)]
pub struct Incompatibility {
    pub terms: Vec<Term>,
    pub cause: Cause,
}

impl Incompatibility {
    /// Create a new incompatibility
    pub fn new(terms: Vec<Term>, cause: Cause) -> Self {
        Self { terms, cause }
    }

    /// Create a root incompatibility from package requirements
    pub fn root(package: String, version_set: VersionSet) -> Rc<Self> {
        Rc::new(Self {
            terms: vec![Term::allowed(package, version_set)],
            cause: Cause::Root,
        })
    }

    /// Create a dependency incompatibility
    pub fn dependency(
        package: String,
        package_version: String,
        dependent: String,
        dependent_version_set: VersionSet,
    ) -> Rc<Self> {
        Rc::new(Self {
            terms: vec![
                Term::exact(package.clone(), &package_version),
                Term::disallowed(dependent.clone(), dependent_version_set),
            ],
            cause: Cause::Dependency {
                package,
                dependent,
            },
        })
    }

    /// Check if this incompatibility is satisfied by given assignments
    pub fn is_satisfied_by<F>(&self, mut is_assigned: F) -> bool
    where
        F: FnMut(&str, &super::version_set::PackedVersion) -> bool,
    {
        // An incompatibility is satisfied if at least one term is false
        // (i.e., not all terms can be true simultaneously)
        self.terms.iter().all(|term| {
            // Simplified check - full implementation needs version assignments
            false
        })
    }

    /// Get the number of terms
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Check if this is a unit incompatibility (only one term)
    pub fn is_unit(&self) -> bool {
        self.terms.len() == 1
    }

    /// Get the first term (useful for unit incompatibilities)
    pub fn first_term(&self) -> Option<&Term> {
        self.terms.first()
    }
}

impl fmt::Debug for Incompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Incompatibility {{ terms: {:?}, cause: {:?} }}", self.terms, self.cause)
    }
}

impl PartialEq for Incompatibility {
    fn eq(&self, other: &Self) -> bool {
        self.terms == other.terms
    }
}

impl Eq for Incompatibility {}

/// Derivation tree for error messages
/// Shows the chain of reasoning that led to a conflict
#[derive(Clone, Debug)]
pub struct DerivationTree {
    pub conflict: Rc<Incompatibility>,
    pub partial_solutions: Vec<String>,
    pub depth: usize,
}

impl DerivationTree {
    /// Create a new derivation tree
    pub fn new(conflict: Rc<Incompatibility>, partial_solutions: Vec<String>) -> Self {
        Self {
            conflict,
            partial_solutions,
            depth: 1,
        }
    }

    /// Format as a human-readable error message
    pub fn format_message(&self) -> String {
        let mut message = String::from("❌ Resolution failed:\n\n");
        
        message.push_str("The following packages have incompatible version requirements:\n\n");
        
        // Group terms by package
        let mut package_terms: std::collections::HashMap<String, Vec<&Term>> = 
            std::collections::HashMap::new();
        
        for term in &self.conflict.terms {
            package_terms
                .entry(term.package.clone())
                .or_default()
                .push(term);
        }

        // Format each package's constraints
        for (package, terms) in &package_terms {
            message.push_str(&format!("  📦 {}:\n", package));
            for term in terms {
                let polarity = if term.is_positive { "requires" } else { "excludes" };
                message.push_str(&format!("    {} {}\n", polarity, format_version_set(&term.version_set)));
            }
            message.push('\n');
        }

        // Add suggestions if possible
        if let Some(suggestion) = self.generate_suggestion(&package_terms) {
            message.push_str(&format!("💡 Suggestion: {}\n", suggestion));
        }

        message
    }

    /// Generate a helpful suggestion for resolving the conflict
    fn generate_suggestion(
        &self,
        package_terms: &std::collections::HashMap<String, Vec<&Term>>,
    ) -> Option<String> {
        // Look for packages with conflicting version requirements
        for (package, terms) in package_terms {
            if terms.len() > 1 {
                // Multiple constraints on same package - suggest relaxing
                return Some(format!(
                    "Try updating {} to a version that satisfies all requirements, \
                     or add an override to force a specific version.",
                    package
                ));
            }
        }
        None
    }

    /// Get the depth of the derivation tree
    pub fn depth(&self) -> usize {
        self.depth
    }
}

impl fmt::Display for DerivationTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_message())
    }
}

/// Format a version set for display
fn format_version_set(vs: &VersionSet) -> String {
    if vs.ranges.is_empty() {
        return "∅".to_string();
    }

    let parts: Vec<String> = vs.ranges.iter().map(|range| {
        let min_ver = range.min.to_version();
        let max_ver = range.max.to_version();
        
        let min_str = if range.min_inclusive {
            format!("{}", min_ver)
        } else {
            format!(">{}", min_ver)
        };
        
        let max_str = if range.max_inclusive {
            format!("{}", max_ver)
        } else {
            format!("<{}", max_ver)
        };
        
        if min_ver == max_ver && range.min_inclusive && range.max_inclusive {
            format!("{}", min_ver)
        } else {
            format!("{} - {}", min_str, max_str)
        }
    }).collect();

    parts.join(" | ")
}

// Re-export VersionSet for convenience
pub use super::version_set::VersionSet;

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::version_set::{VersionRange, PackedVersion};

    #[test]
    fn test_incompatibility_creation() {
        let terms = vec![
            Term::allowed("pkg-a".to_string(), VersionSet::any()),
            Term::disallowed("pkg-b".to_string(), VersionSet::any()),
        ];
        
        let incompat = Incompatibility::new(terms, Cause::Root);
        assert_eq!(incompat.term_count(), 2);
        assert!(!incompat.is_unit());
    }

    #[test]
    fn test_derivation_tree_format() {
        let terms = vec![
            Term::allowed("react".to_string(), VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("18.0.0").unwrap(),
                max: PackedVersion::parse("18.255.255").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            })),
            Term::disallowed("react".to_string(), VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("17.0.0").unwrap(),
                max: PackedVersion::parse("17.255.255").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            })),
        ];
        
        let incompat = Incompatibility::new(terms, Cause::Root);
        let tree = DerivationTree::new(Rc::new(incompat), vec![]);
        let message = tree.format_message();
        
        assert!(message.contains("react"));
        assert!(message.contains("Resolution failed"));
    }
}
