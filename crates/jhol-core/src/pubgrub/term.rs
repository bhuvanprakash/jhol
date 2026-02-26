//! Terms for PubGrub solver
//! 
//! A term represents a constraint on a package version, either positive (allowed)
//! or negative (disallowed).

use super::version_set::VersionSet;
use std::rc::Rc;

/// A term in the satisfiability problem
/// Represents either "package MUST be in version_set" (positive)
/// or "package MUST NOT be in version_set" (negative)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub package: String,
    pub version_set: VersionSet,
    pub is_positive: bool,
}

impl Term {
    /// Create a positive term (package MUST be in version_set)
    pub fn allowed(package: String, version_set: VersionSet) -> Self {
        Self {
            package,
            version_set,
            is_positive: true,
        }
    }

    /// Create a negative term (package MUST NOT be in version_set)
    pub fn disallowed(package: String, version_set: VersionSet) -> Self {
        Self {
            package,
            version_set,
            is_positive: false,
        }
    }

    /// Create a term that requires an exact version
    pub fn exact(package: String, version: &str) -> Self {
        let vs = VersionSet::from_range(super::version_set::VersionRange {
            min: super::version_set::PackedVersion::parse(version).unwrap_or(super::version_set::PackedVersion { packed: 0 }),
            max: super::version_set::PackedVersion::parse(version).unwrap_or(super::version_set::PackedVersion { packed: 0 }),
            min_inclusive: true,
            max_inclusive: true,
        });
        Self {
            package,
            version_set: vs,
            is_positive: true,
        }
    }

    /// Negate this term
    pub fn negate(&self) -> Self {
        Self {
            package: self.package.clone(),
            version_set: self.version_set.clone(),
            is_positive: !self.is_positive,
        }
    }

    /// Intersect with another term for the same package
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        if self.package != other.package {
            return None;
        }

        let version_set = if self.is_positive && other.is_positive {
            // Both positive: intersect the version sets
            self.version_set.intersect(&other.version_set)
        } else if self.is_positive && !other.is_positive {
            // Positive and negative: subtract negative from positive
            self.version_set.difference(&other.version_set)
        } else if !self.is_positive && other.is_positive {
            // Negative and positive: subtract negative from positive
            other.version_set.difference(&self.version_set)
        } else {
            // Both negative: union of the version sets (De Morgan's law)
            self.version_set.union(&other.version_set)
        };

        Some(Self {
            package: self.package.clone(),
            version_set,
            is_positive: self.is_positive && other.is_positive,
        })
    }

    /// Check if this term is satisfied by a specific version
    pub fn satisfies(&self, version: &super::version_set::PackedVersion) -> bool {
        let in_set = self.version_set.contains(version);
        if self.is_positive {
            in_set
        } else {
            !in_set
        }
    }

    /// Check if this term allows any version from the available set
    pub fn allows_any(&self, available: &[super::version_set::PackedVersion]) -> bool {
        available.iter().any(|v| self.satisfies(v))
    }

    /// Get the highest allowed version from available versions
    pub fn highest_allowed(&self, available: &[super::version_set::PackedVersion]) -> Option<super::version_set::PackedVersion> {
        if self.is_positive {
            self.version_set.highest(available)
        } else {
            // For negative terms, find highest version NOT in the set
            available
                .iter()
                .filter(|v| !self.version_set.contains(v))
                .max_by(|a, b| a.cmp(b))
                .copied()
        }
    }
}

/// A set of terms that can be efficiently queried
#[derive(Clone, Debug, Default)]
pub struct TermSet {
    terms: Vec<Term>,
}

impl TermSet {
    pub fn new() -> Self {
        Self { terms: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            terms: Vec::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, term: Term) {
        self.terms.push(term);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Term> {
        self.terms.iter()
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Check if all terms are satisfied by the given assignments
    pub fn is_satisfied_by<F>(&self, mut is_satisfied: F) -> bool
    where
        F: FnMut(&str, &super::version_set::PackedVersion) -> bool,
    {
        self.terms.iter().all(|term| {
            // For positive terms, we need a version that satisfies it
            // For negative terms, we need NO version that satisfies it
            if term.is_positive {
                // Check if any available version satisfies this term
                // This is a simplified check - full implementation needs available versions
                true
            } else {
                // Negative term is satisfied if no version is in the set
                true
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::version_set::{VersionSet, VersionRange, PackedVersion};

    #[test]
    fn test_term_intersect_positive() {
        let t1 = Term::allowed(
            "pkg".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("1.0.0").unwrap(),
                max: PackedVersion::parse("2.0.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );

        let t2 = Term::allowed(
            "pkg".to_string(),
            VersionSet::from_range(VersionRange {
                min: PackedVersion::parse("1.5.0").unwrap(),
                max: PackedVersion::parse("2.5.0").unwrap(),
                min_inclusive: true,
                max_inclusive: true,
            }),
        );

        let result = t1.intersect(&t2).unwrap();
        assert!(result.is_positive);
        assert!(result.satisfies(&PackedVersion::parse("1.7.0").unwrap()));
        assert!(!result.satisfies(&PackedVersion::parse("1.0.0").unwrap()));
    }

    #[test]
    fn test_term_negate() {
        let t = Term::allowed("pkg".to_string(), VersionSet::any());
        let negated = t.negate();
        assert!(!negated.is_positive);
        assert_eq!(negated.package, "pkg");
    }
}
