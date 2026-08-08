//! JAGR-2: PubGrub-inspired dependency resolution algorithm
//!
//! This module implements a fast, conflict-driven dependency resolver
//! with excellent error messages and 10-100x faster resolution than SAT.
//!
//! Based on the PubGrub algorithm used by uv, Swift PM, Bundler, and Dart.

mod incompatibility;
mod minimal;
mod partial_solution;
mod solver;
mod term;
mod version_set;
mod vsids; // JAGR-3: Minimal version selection

pub use incompatibility::{Cause, DerivationTree, Incompatibility};
pub use minimal::{
    can_use_minimal_selection, detect_early_conflicts, resolve_minimal, MinimalVersionSelector,
    ResolutionError,
};
pub use partial_solution::PartialSolution;
pub use solver::{PubGrubError, PubGrubResult, PubGrubSolver, Solution};
pub use term::Term;
pub use version_set::{PackedVersion, VersionRange, VersionSet};
pub use vsids::AdaptiveHeuristic; // JAGR-3: Adaptive heuristic (replaces VSIDS)

#[cfg(test)]
mod tests;
