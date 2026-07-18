//! GRM Query - Query engine for resolution memory

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::time::Duration;

/// GRM Query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GRMQuery {
    pub package_spec: String,
    pub platform: Option<String>,
    pub node_version: Option<String>,
    pub freshness: Option<FreshnessRequirement>,
}

impl GRMQuery {
    /// Create a new query
    pub fn new(package_spec: impl Into<String>) -> Self {
        Self {
            package_spec: package_spec.into(),
            platform: None,
            node_version: None,
            freshness: None,
        }
    }

    /// Set platform
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    /// Set node version
    pub fn with_node_version(mut self, node_version: impl Into<String>) -> Self {
        self.node_version = Some(node_version.into());
        self
    }

    /// Set freshness requirement
    pub fn with_freshness(mut self, freshness: FreshnessRequirement) -> Self {
        self.freshness = Some(freshness);
        self
    }
}

impl Default for GRMQuery {
    fn default() -> Self {
        Self {
            package_spec: String::new(),
            platform: None,
            node_version: None,
            freshness: None,
        }
    }
}

/// Freshness requirement
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FreshnessRequirement {
    /// Within this duration
    Within(Duration),
    /// At least this many recent hits
    RecentHits(u64),
    /// Either fresh by time OR by hits
    Either(Duration, u64),
    /// Both fresh by time AND by hits
    Both(Duration, u64),
}

impl FreshnessRequirement {
    /// Check if age satisfies this requirement
    pub fn is_fresh(&self, age_ms: u64) -> bool {
        match self {
            FreshnessRequirement::Within(d) => Duration::from_millis(age_ms) <= *d,
            _ => true, // For hit-based, we check separately
        }
    }

    /// Check if hit count satisfies this requirement
    #[allow(dead_code)]
    pub fn has_hits(&self, hits: u64) -> bool {
        match self {
            FreshnessRequirement::Within(_) => true,
            FreshnessRequirement::RecentHits(min) => hits >= *min,
            FreshnessRequirement::Either(d, min) => Duration::from_millis(0) <= *d || hits >= *min,
            FreshnessRequirement::Both(d, min) => Duration::from_millis(0) <= *d && hits >= *min,
        }
    }
}

/// Query result with scoring
#[derive(Debug, Clone)]
pub struct ScoredQueryResult {
    pub record: crate::grm::ResolutionRecord,
    pub score: f32,
    pub freshness_score: f32,
    pub hit_score: f32,
}

impl ScoredQueryResult {
    /// Compare for sorting (higher score first)
    #[allow(dead_code)]
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        // Handle NaN: treat as greater than all finite values
        if self.score.is_nan() && other.score.is_nan() {
            Ordering::Equal
        } else if self.score.is_nan() {
            Ordering::Greater // NaN is "greater", but we reverse below
        } else if other.score.is_nan() {
            Ordering::Less
        } else {
            self.score
                .partial_cmp(&other.score)
                .unwrap_or(Ordering::Equal)
        }
        .reverse()
    }
}

/// Query builder for fluent API
#[derive(Debug, Clone, Default)]
pub struct QueryBuilder {
    package_spec: String,
    platform: Option<String>,
    node_version: Option<String>,
    freshness: Option<FreshnessRequirement>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl QueryBuilder {
    /// Create new builder for package
    pub fn for_package(package: impl Into<String>) -> Self {
        Self {
            package_spec: package.into(),
            ..Default::default()
        }
    }

    /// Set platform constraint
    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    /// Set node version constraint
    pub fn node_version(mut self, version: impl Into<String>) -> Self {
        self.node_version = Some(version.into());
        self
    }

    /// Require freshness within duration
    pub fn fresh_within(mut self, duration: Duration) -> Self {
        self.freshness = Some(FreshnessRequirement::Within(duration));
        self
    }

    /// Require minimum hit count
    pub fn min_hits(mut self, hits: u64) -> Self {
        self.freshness = Some(FreshnessRequirement::RecentHits(hits));
        self
    }

    /// Set limit
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set offset
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Build query
    pub fn build(self) -> GRMQuery {
        GRMQuery {
            package_spec: self.package_spec,
            platform: self.platform,
            node_version: self.node_version,
            freshness: self.freshness,
        }
    }
}
