//! Version sets and range arithmetic for PubGrub solver
//! 
//! Implements efficient version range operations with packed u64 representation
//! for fast SIMD-accelerated comparisons.

use semver::{Version, VersionReq, Comparator, Op};
use std::cmp::Ordering;

/// Packed version for fast comparison (stores major.minor.patch in u64)
/// Format: (major << 40) | (minor << 20) | patch
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedVersion {
    pub packed: u64,
}

impl std::fmt::Debug for PackedVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_version())
    }
}

impl PackedVersion {
    /// Parse a version string into packed format
    pub fn parse(version: &str) -> Option<Self> {
        // Fast path for simple versions: "1.2.3"
        let parts: [u64; 3] = parse_version_fast(version)?;
        let packed = (parts[0] << 40) | (parts[1] << 20) | parts[2];
        Some(Self { packed })
    }

    /// Create from semver Version
    pub fn from_version(version: &Version) -> Self {
        let packed = (version.major << 40) | ((version.minor as u64) << 20) | (version.patch as u64);
        Self { packed }
    }

    /// Convert to semver Version
    pub fn to_version(&self) -> Version {
        Version {
            major: self.packed >> 40,
            minor: ((self.packed >> 20) & 0xFFFFF) as u64,
            patch: (self.packed & 0xFFFFF) as u64,
            pre: semver::Prerelease::EMPTY,
            build: semver::BuildMetadata::EMPTY,
        }
    }

    /// Check if this version satisfies a range
    #[inline]
    pub fn satisfies(&self, range: &VersionRange) -> bool {
        self.packed >= range.min.packed && self.packed <= range.max.packed
    }

    /// Compare two packed versions
    #[inline]
    pub fn cmp(&self, other: &Self) -> Ordering {
        self.packed.cmp(&other.packed)
    }
}

/// Fast version string parsing without full semver validation
fn parse_version_fast(version: &str) -> Option<[u64; 3]> {
    let version = version.trim_start_matches('v').trim_start_matches('=');
    let mut parts = [0u64; 3];
    let mut current = 0u64;
    let mut part_idx = 0;

    for c in version.chars() {
        match c {
            '0'..='9' => {
                current = current * 10 + (c as u64 - '0' as u64);
                // Prevent overflow for reasonable version numbers
                if current > 0xFFFFF {
                    return None;
                }
            }
            '.' => {
                if part_idx >= 2 {
                    return None; // Too many parts
                }
                parts[part_idx] = current;
                part_idx += 1;
                current = 0;
            }
            '-' | '+' => {
                // Pre-release or build metadata - ignore for packing
                break;
            }
            _ => {
                // Allow caret, tilde, etc. at start
                if part_idx > 0 || current > 0 {
                    return None;
                }
            }
        }
    }

    if part_idx < 2 {
        parts[part_idx] = current;
    }

    // Fill remaining parts with 0
    for i in part_idx..3 {
        if i == part_idx && part_idx < 2 {
            parts[i] = current;
        }
    }

    Some(parts)
}

/// A continuous range of versions
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRange {
    pub min: PackedVersion,
    pub max: PackedVersion,
    pub min_inclusive: bool,
    pub max_inclusive: bool,
}

impl VersionRange {
    /// Create a new version range
    pub fn new(
        min: PackedVersion,
        max: PackedVersion,
        min_inclusive: bool,
        max_inclusive: bool,
    ) -> Self {
        Self {
            min,
            max,
            min_inclusive,
            max_inclusive,
        }
    }

    /// Create from a semver Comparator
    pub fn from_comparator(comparator: &Comparator) -> Self {
        let min_version = Version {
            major: comparator.major,
            minor: comparator.minor.unwrap_or(0),
            patch: comparator.patch.unwrap_or(0),
            pre: comparator.pre.clone(),
            build: semver::BuildMetadata::EMPTY,
        };
        let min = PackedVersion::from_version(&min_version);

        match comparator.op {
            Op::Exact => Self {
                min,
                max: min,
                min_inclusive: true,
                max_inclusive: true,
            },
            Op::Greater => Self {
                min,
                max: PackedVersion { packed: u64::MAX },
                min_inclusive: false,
                max_inclusive: true,
            },
            Op::GreaterEq => Self {
                min,
                max: PackedVersion { packed: u64::MAX },
                min_inclusive: true,
                max_inclusive: true,
            },
            Op::Less => Self {
                min: PackedVersion { packed: 0 },
                max: min,
                min_inclusive: true,
                max_inclusive: false,
            },
            Op::LessEq => Self {
                min: PackedVersion { packed: 0 },
                max: min,
                min_inclusive: true,
                max_inclusive: true,
            },
            Op::Tilde => {
                // ~1.2.3 := >=1.2.3, <1.3.0
                let mut max_version = min_version.clone();
                max_version.minor += 1;
                max_version.patch = 0;
                Self {
                    min,
                    max: PackedVersion::from_version(&max_version),
                    min_inclusive: true,
                    max_inclusive: false,
                }
            }
            Op::Caret => {
                // ^1.2.3 := >=1.2.3, <2.0.0 (if major > 0)
                let mut max_version = min_version.clone();
                if max_version.major > 0 {
                    max_version.major += 1;
                    max_version.minor = 0;
                    max_version.patch = 0;
                } else if max_version.minor > 0 {
                    max_version.minor += 1;
                    max_version.patch = 0;
                } else {
                    max_version.patch += 1;
                }
                Self {
                    min,
                    max: PackedVersion::from_version(&max_version),
                    min_inclusive: true,
                    max_inclusive: false,
                }
            }
            _ => Self {
                min: PackedVersion { packed: 0 },
                max: PackedVersion { packed: u64::MAX },
                min_inclusive: true,
                max_inclusive: true,
            },
        }
    }

    /// Check if a version is in this range
    #[inline]
    pub fn contains(&self, version: &PackedVersion) -> bool {
        let min_ok = if self.min_inclusive {
            version.packed >= self.min.packed
        } else {
            version.packed > self.min.packed
        };

        let max_ok = if self.max_inclusive {
            version.packed <= self.max.packed
        } else {
            version.packed < self.max.packed
        };

        min_ok && max_ok
    }

    /// Intersect two ranges
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let new_min_packed = self.min.packed.max(other.min.packed);
        let new_max_packed = self.max.packed.min(other.max.packed);

        if new_min_packed > new_max_packed {
            return None;
        }

        let new_min = PackedVersion { packed: new_min_packed };
        let new_max = PackedVersion { packed: new_max_packed };

        Some(Self {
            min: new_min,
            max: new_max,
            min_inclusive: self.min_inclusive && other.min_inclusive,
            max_inclusive: self.max_inclusive && other.max_inclusive,
        })
    }

    /// Union of two ranges (merges if overlapping or adjacent)
    pub fn union(&self, other: &Self) -> Self {
        let min_packed = self.min.packed.min(other.min.packed);
        let max_packed = self.max.packed.max(other.max.packed);

        Self {
            min: PackedVersion { packed: min_packed },
            max: PackedVersion { packed: max_packed },
            min_inclusive: self.min_inclusive || other.min_inclusive,
            max_inclusive: self.max_inclusive || other.max_inclusive,
        }
    }

    /// Check if ranges overlap
    pub fn overlaps(&self, other: &Self) -> bool {
        self.intersect(other).is_some()
    }

    /// Check if ranges touch (can be merged)
    pub fn touches(&self, other: &Self) -> bool {
        self.max.packed == other.min.packed && (self.max_inclusive || other.min_inclusive)
            || self.min.packed == other.max.packed && (self.min_inclusive || other.max_inclusive)
    }
}

/// A set of versions, represented as disjoint ranges
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionSet {
    pub ranges: Vec<VersionRange>,
}

impl VersionSet {
    /// Create an empty version set
    pub fn empty() -> Self {
        Self { ranges: Vec::new() }
    }

    /// Create a version set containing all versions
    pub fn any() -> Self {
        Self {
            ranges: vec![VersionRange {
                min: PackedVersion { packed: 0 },
                max: PackedVersion { packed: u64::MAX },
                min_inclusive: true,
                max_inclusive: true,
            }],
        }
    }

    /// Create a VersionSet from a semver requirement
    pub fn from_req(req: &VersionReq) -> Self {
        if req.comparators.is_empty() {
            return Self::any();
        }

        let mut ranges = Vec::new();
        for comparator in &req.comparators {
            let range = VersionRange::from_comparator(comparator);
            ranges.push(range);
        }

        // If multiple comparators, intersect them (AND logic)
        if ranges.len() > 1 {
            let mut result = ranges[0].clone();
            for range in &ranges[1..] {
                if let Some(intersection) = result.intersect(range) {
                    result = intersection;
                } else {
                    return Self::empty(); // Conflicting requirements
                }
            }
            Self {
                ranges: vec![result],
            }
        } else {
            Self { ranges }
        }
    }

    /// Create a VersionSet from a single range
    pub fn from_range(range: VersionRange) -> Self {
        Self {
            ranges: vec![range],
        }
    }

    /// Check if a version is in this set
    pub fn contains(&self, version: &PackedVersion) -> bool {
        self.ranges.iter().any(|range| range.contains(version))
    }

    /// Check if version string is in this set
    pub fn contains_str(&self, version_str: &str) -> bool {
        PackedVersion::parse(version_str)
            .map(|v| self.contains(&v))
            .unwrap_or(false)
    }

    /// Intersect two version sets
    pub fn intersect(&self, other: &Self) -> Self {
        let mut result = Vec::new();

        for r1 in &self.ranges {
            for r2 in &other.ranges {
                if let Some(intersection) = r1.intersect(r2) {
                    result.push(intersection);
                }
            }
        }

        Self { ranges: result }
    }

    /// Union of two version sets
    pub fn union(&self, other: &Self) -> Self {
        let mut ranges = self.ranges.clone();
        ranges.extend(other.ranges.iter().cloned());

        // Merge overlapping ranges
        ranges.sort_by(|a, b| a.min.packed.cmp(&b.min.packed));

        let mut merged: Vec<VersionRange> = Vec::new();
        for range in ranges {
            if let Some(last) = merged.last_mut() {
                if last.overlaps(&range) || last.touches(&range) {
                    *last = last.union(&range);
                } else {
                    merged.push(range);
                }
            } else {
                merged.push(range);
            }
        }

        Self { ranges: merged }
    }

    /// Difference between two version sets
    pub fn difference(&self, other: &Self) -> Self {
        let mut result = self.ranges.clone();

        for other_range in &other.ranges {
            let mut new_result = Vec::new();

            for range in result {
                // Simple case: no overlap
                if !range.overlaps(other_range) {
                    new_result.push(range);
                    continue;
                }

                // Complex case: subtract other_range from range
                // This is simplified - full implementation would handle all cases
                if range.min.packed < other_range.min.packed {
                    new_result.push(VersionRange {
                        min: range.min,
                        max: other_range.min,
                        min_inclusive: range.min_inclusive,
                        max_inclusive: !other_range.min_inclusive,
                    });
                }

                if range.max.packed > other_range.max.packed {
                    new_result.push(VersionRange {
                        min: other_range.max,
                        max: range.max,
                        min_inclusive: !other_range.max_inclusive,
                        max_inclusive: range.max_inclusive,
                    });
                }
            }

            result = new_result;
        }

        Self { ranges: result }
    }

    /// Check if this set is empty
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Get the highest version in this set from available versions
    pub fn highest(&self, available: &[PackedVersion]) -> Option<PackedVersion> {
        available
            .iter()
            .filter(|v| self.contains(v))
            .max_by(|a, b| a.cmp(b))
            .copied()
    }

    /// Get the lowest version in this set from available versions
    pub fn lowest(&self, available: &[PackedVersion]) -> Option<PackedVersion> {
        available
            .iter()
            .filter(|v| self.contains(v))
            .min_by(|a, b| a.cmp(b))
            .copied()
    }
}

impl From<VersionRange> for VersionSet {
    fn from(range: VersionRange) -> Self {
        Self::from_range(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packed_version_parse() {
        let v = PackedVersion::parse("1.2.3").unwrap();
        assert_eq!(v.packed, (1 << 40) | (2 << 20) | 3);

        let v = PackedVersion::parse("v10.20.30").unwrap();
        assert_eq!(v.packed, (10 << 40) | (20 << 20) | 30);
    }

    #[test]
    fn test_version_range_contains() {
        let range = VersionRange {
            min: PackedVersion::parse("1.0.0").unwrap(),
            max: PackedVersion::parse("2.0.0").unwrap(),
            min_inclusive: true,
            max_inclusive: false,
        };

        assert!(range.contains(&PackedVersion::parse("1.0.0").unwrap()));
        assert!(range.contains(&PackedVersion::parse("1.5.0").unwrap()));
        assert!(!range.contains(&PackedVersion::parse("2.0.0").unwrap()));
    }

    #[test]
    fn test_version_set_from_req() {
        let req: VersionReq = "^1.0.0".parse().unwrap();
        let set = VersionSet::from_req(&req);
        assert!(!set.is_empty());
        assert!(set.contains(&PackedVersion::parse("1.5.0").unwrap()));
        assert!(!set.contains(&PackedVersion::parse("2.0.0").unwrap()));
    }
}
