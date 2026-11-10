//! Command groups extracted from `main.rs`.
//!
//! Nothing here is new behaviour: each module is a straight move so the dispatch chain
//! stays readable, and the CLI tests plus both quality gates are the proof that the move
//! was a move.

pub mod attest;
pub mod cache;
pub mod env;
