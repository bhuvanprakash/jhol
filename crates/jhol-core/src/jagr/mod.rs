//! JAGR resolver — Layer 0 spectral pre-filter (Soul Path gate).
//! Full three-layer resolver available via feature flag.

pub mod betti_numbers;
pub mod conflict_explainer;
pub mod core_guided_lp;
pub mod core_guided_solver;
pub mod idea_benchmark;
pub mod io_benchmark;
pub mod jagr_resolver;
pub mod knot;
pub mod maxsat_solver;
pub mod mayer_vietoris;
pub mod ol3p_solver;
pub mod priority_benchmark;
pub mod resolution_differential;
pub mod simplicial_normal_form;
pub mod tropical_math;
pub mod tropical_max_atom;
pub mod tropical_nullstellensatz;
pub mod tropical_polyhedron;
pub mod tropical_sheaf;
pub mod tropical_sheaf_analyzer;

pub use jagr_resolver::JagrResolver;
pub use knot::{build_knot_graph, knot_solve, KnotRequest};
pub use resolution_differential::{ResolutionDifferential, ResolveRequest};
