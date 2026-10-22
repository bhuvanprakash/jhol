//! Jhol global thread pool — persistent rayon worker pool for all parallel work.
//!
//! Uses OnceLock so the pool is created once and reused across all install calls.
//! No pool construction per operation. Pool size is bounded by `io_pool_threads`. Jobs smaller than
//! `sequential_threshold` run on the main thread to avoid pool contention.
//!
//! All module-local parallelism flows through this pool:
//!   - install.rs: tarball downloads, package installation
//!   - registry.rs: packument metadata fetching
//!   - prefetch.rs: prefetch downloads
//!   - capsule.rs: hash computation

use rayon::ThreadPool;
use std::sync::OnceLock;

/// Shared bounded IO workers; default Rayon metadata discovery remains separate.
/// Created on first use and reused for the lifetime of the process.

/// Thread count for the shared IO pool: `4 x cores`, clamped to `2..=32`.
///
/// NOTE: lowering this (e.g. to 8) together with the HTTP bounds deadlocked
/// plain installs — the fetcher parks with the pool saturated — so it is left at
/// the original sizing. A concurrency reduction here must be validated with a
/// real `jhol install` before it ships.
pub fn io_pool_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() * 4).clamp(2, 32))
        .unwrap_or(8)
}

pub(crate) fn global_pool() -> &'static ThreadPool {
    static POOL: OnceLock<ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            // Bound by machine size: a hard 64 threads times several concurrent
            // installs oversubscribes a small laptop (10 cores / 6 GB here) and
            // makes installs stall instead of finish. 4x cores, capped at 32,
            // keeps the IO pool useful for latency-bound work without thrashing.
            .num_threads(io_pool_threads())
            .thread_name(|i| format!("jhol-worker-{}", i))
            .build()
            .expect("jhol global thread pool: build failed")
    })
}

/// Run parallel map over items using the global thread pool.
/// Jobs smaller than `sequential_threshold` run sequentially on the main thread.
pub fn parallel_map<I, O, F>(
    items: Vec<I>,
    concurrency: usize,
    sequential_threshold: usize,
    job: F,
) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    if items.is_empty() {
        return Vec::new();
    }

    // Fast path: tiny batches run on main thread (no pool overhead)
    if concurrency <= 1 || items.len() <= sequential_threshold.clamp(1, 64) {
        return items.into_iter().map(job).collect();
    }

    let worker_count = concurrency.clamp(1, 64).min(items.len());
    let total_items = items.len();

    global_pool().install(|| {
        use rayon::prelude::*;
        items
            .into_par_iter()
            .with_max_len(std::cmp::max(1, total_items / worker_count))
            .map(job)
            .collect()
    })
}

/// Run parallel map over items using the global thread pool (simple API).
/// Uses default concurrency of 64 and sequential threshold of 8.
pub fn par_map<I, O, F>(items: Vec<I>, job: F) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    parallel_map(items, 64, 8, job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_map_order() {
        let input: Vec<i32> = (0..100).collect();
        let results = parallel_map(input.clone(), 8, 1, |n| n * 2);
        let mut sorted = results.clone();
        sorted.sort();
        assert_eq!(sorted, input.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn test_sequential_threshold() {
        let input: Vec<i32> = vec![1, 2, 3];
        // sequential_threshold=100 forces sequential
        let results = parallel_map(input.clone(), 64, 100, |n| n * 2);
        assert_eq!(results, vec![2, 4, 6]);
    }

    #[test]
    fn test_empty_input() {
        let results: Vec<i32> = parallel_map(Vec::<i32>::new(), 64, 8, |n| n);
        assert!(results.is_empty());
    }

    #[test]
    fn test_concurrent_correctness() {
        // Parallel computation must produce same result as sequential
        let input: Vec<i32> = (0..1000).collect();
        let results = parallel_map(input.clone(), 64, 1, |n| n.wrapping_mul(3).wrapping_add(7));
        let expected: Vec<i32> = input
            .iter()
            .map(|n| n.wrapping_mul(3).wrapping_add(7))
            .collect();
        let mut sorted_results = results.clone();
        sorted_results.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(sorted_results, sorted_expected);
    }
}
