//! JAGR-2: PubGrub-inspired dependency resolution algorithm
//! 
//! This module implements a fast, conflict-driven dependency resolver
//! with excellent error messages and 10-100x faster resolution than SAT.
//! 
//! Based on the PubGrub algorithm used by uv, Swift PM, Bundler, and Dart.

mod version_set;
mod term;
mod incompatibility;
mod partial_solution;
mod solver;
mod vsids;
mod minimal;  // JAGR-3: Minimal version selection

pub use version_set::{VersionSet, VersionRange, PackedVersion};
pub use term::Term;
pub use incompatibility::{Incompatibility, Cause, DerivationTree};
pub use partial_solution::PartialSolution;
pub use solver::{PubGrubSolver, PubGrubError, PubGrubResult, Solution};
pub use vsids::AdaptiveHeuristic;  // JAGR-3: Adaptive heuristic (replaces VSIDS)
pub use minimal::{MinimalVersionSelector, resolve_minimal, can_use_minimal_selection, ResolutionError, detect_early_conflicts};

#[cfg(test)]
mod tests;
