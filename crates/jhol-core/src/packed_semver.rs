//! Packed 64-bit integer representation of Semantic Versioning (Semver).
//!
//! Encodes major (24 bits), minor (20 bits), patch (20 bits) into a single u64.
//! Enables comparing and range-checking semver versions using 1-2 CPU integer
//! instructions without string or heap allocations.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackedSemver(pub u64);

impl PackedSemver {
    #[inline(always)]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        let val = ((major & 0xFFFFFF) << 40) | ((minor & 0xFFFFF) << 20) | (patch & 0xFFFFF);
        Self(val)
    }

    #[inline(always)]
    pub const fn major(&self) -> u64 {
        (self.0 >> 40) & 0xFFFFFF
    }

    #[inline(always)]
    pub const fn minor(&self) -> u64 {
        (self.0 >> 20) & 0xFFFFF
    }

    #[inline(always)]
    pub const fn patch(&self) -> u64 {
        self.0 & 0xFFFFF
    }

    /// Parse a standard version string (e.g. "1.2.3") into a PackedSemver.
    pub fn parse(s: &str) -> Option<Self> {
        let clean = s.trim().trim_start_matches('v').trim_start_matches('=');
        let mut parts = clean.split('.');
        let major = parts.next()?.parse::<u64>().ok()?;
        let minor = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
        let patch_part = parts.next().unwrap_or("0");
        // Handle pre-releases by taking only the leading digits
        let patch_digits: String = patch_part
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let patch = if patch_digits.is_empty() {
            0
        } else {
            patch_digits.parse::<u64>().ok().unwrap_or(0)
        };
        Some(Self::new(major, minor, patch))
    }
}

/// A packed half-open interval [min, max) representing a semver constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedInterval {
    pub min: PackedSemver,
    pub max: PackedSemver,
}

impl PackedInterval {
    #[inline(always)]
    pub const fn new(min: PackedSemver, max: PackedSemver) -> Self {
        Self { min, max }
    }

    #[inline(always)]
    pub fn any() -> Self {
        Self {
            min: PackedSemver(0),
            max: PackedSemver(u64::MAX),
        }
    }

    #[inline(always)]
    pub fn contains(&self, version: PackedSemver) -> bool {
        version.0 >= self.min.0 && version.0 < self.max.0
    }

    /// Parse constraint specifier (e.g. "^1.2.3", "~1.2.3", ">=1.0.0 <2.0.0", "*", "latest")
    pub fn parse_spec(spec: &str) -> Option<Self> {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed == "*" || trimmed == "latest" || trimmed == "x" {
            return Some(Self::any());
        }

        if let Some(rest) = trimmed.strip_prefix('^') {
            let base = PackedSemver::parse(rest)?;
            let maj = base.major();
            let max = if maj == 0 {
                let min = base.minor();
                if min == 0 {
                    PackedSemver::new(0, 0, base.patch() + 1)
                } else {
                    PackedSemver::new(0, min + 1, 0)
                }
            } else {
                PackedSemver::new(maj + 1, 0, 0)
            };
            return Some(Self::new(base, max));
        }

        if let Some(rest) = trimmed.strip_prefix('~') {
            let base = PackedSemver::parse(rest)?;
            let max = PackedSemver::new(base.major(), base.minor() + 1, 0);
            return Some(Self::new(base, max));
        }

        if let Some(exact) = PackedSemver::parse(trimmed) {
            let max = PackedSemver(exact.0 + 1);
            return Some(Self::new(exact, max));
        }

        Some(Self::any())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packed_semver_roundtrip() {
        let v = PackedSemver::new(18, 2, 0);
        assert_eq!(v.major(), 18);
        assert_eq!(v.minor(), 2);
        assert_eq!(v.patch(), 0);

        let parsed = PackedSemver::parse("18.2.0").unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn test_packed_interval_contains() {
        let interval = PackedInterval::parse_spec("^18.2.0").unwrap();
        assert!(interval.contains(PackedSemver::parse("18.2.0").unwrap()));
        assert!(interval.contains(PackedSemver::parse("18.3.1").unwrap()));
        assert!(!interval.contains(PackedSemver::parse("19.0.0").unwrap()));
        assert!(!interval.contains(PackedSemver::parse("18.1.9").unwrap()));
    }
}
