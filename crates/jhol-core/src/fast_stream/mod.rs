//! Jhol FastStream Engine
//!
//! Integrates:
//! 1. `ssdp`: Stream-to-Store Direct Projection (Single-pass in-memory decompression and direct linking)
//! 2. `twm`: Topological Wavefront Multiplexing (JAGR Sheaf cohomology scheduling)
//! 3. `vrb`: Vectorized Lock-Free Ring Buffer (Zero-allocation slab arena)

pub mod ssdp;
pub mod twm;
pub mod vrb;

pub use ssdp::{ProjectionResult, StreamDirectProjector};
pub use twm::{StrandCategory, TopologicalWavefrontScheduler, WavefrontDispatchItem};
pub use vrb::{AlignedSlab, SlabArena, VrbStreamChannel, DEFAULT_ARENA_SLABS, SLAB_SIZE};
