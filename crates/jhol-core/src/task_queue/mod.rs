//! JAGR-2: Parallel Task Execution with Rayon
//! 
//! High-performance parallel execution using rayon's work-stealing.
//! Much simpler and more reliable than custom implementation.

mod parallel;
mod adaptive;

pub use parallel::{parallel_map, parallel_map_with_progress, ParallelMap};
pub use adaptive::{AdaptiveConcurrency, ConcurrencyConfig, ConcurrencyStats};
