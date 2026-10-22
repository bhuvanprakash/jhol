//! Global Resolution Memory — compatibility shim over [`crate::origin`].
//!
//! The resolution-record types and the memory facade now live in
//! `origin.rs`, which owns the metadata origin: packuments, resolution
//! records, and the append-only install log are one thing, and the
//! origin is the one place that should own the shape of that fact.
//!
//! This module keeps the `crate::grm::*` paths working for existing
//! callers (soul_path, lock_free_grm, resolution_mirror, lattice_cache)
//! and continues to host the GRM submodules.
//!
//! Nothing here does work any more. New code should call `crate::origin`.

pub mod distributed;
pub mod query;
pub mod store;
pub mod sync;

pub use crate::origin::{
    current_timestamp, garbage_collect, insert, lookup, GRMStats, GrmHandle, ResolutionRecord,
    ResolutionSource,
};
