//! JAGR resolver — Layer 0 spectral pre-filter (Soul Path gate).
//! Full three-layer resolver available via feature flag.

pub mod knot;
pub mod resolution_differential;

pub use knot::{knot_solve, build_knot_graph, KnotRequest};
pub use resolution_differential::{ResolutionDifferential, ResolveRequest};

/// Compatibility stub for the full three-layer JAGR resolver.
pub struct JagrResolver;
impl JagrResolver {
    pub fn new() -> Self { Self }
    pub fn with_lp(self, _lp: bool) -> Self { self }
    pub fn with_timeout(self, _ms: u64) -> Self { self }
    pub fn with_skip_layer3(self, _skip: bool) -> Self { self }
    pub fn with_skip_layer2(self, _skip: bool) -> Self { self }
    pub fn with_run_all_layers(self, _all: bool) -> Self { self }
}
