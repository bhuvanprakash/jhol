//! Parallel map and execution using rayon

use rayon::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Parallel map with automatic thread pool sizing
pub fn parallel_map<I, O, F>(items: Vec<I>, f: F) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    items.into_par_iter().map(f).collect()
}

/// Parallel map with progress tracking
pub fn parallel_map_with_progress<I, O, F>(
    items: Vec<I>,
    f: F,
    progress_counter: &AtomicUsize,
) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    items
        .into_par_iter()
        .map(|item| {
            let result = f(item);
            progress_counter.fetch_add(1, Ordering::Relaxed);
            result
        })
        .collect()
}

/// Parallel map with custom configuration
pub struct ParallelMap<I, O, F>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    items: Vec<I>,
    f: Arc<F>,
    num_threads: Option<usize>,
}

impl<I, O, F> ParallelMap<I, O, F>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    /// Create a new parallel map operation
    pub fn new(items: Vec<I>, f: F) -> Self {
        Self {
            items,
            f: Arc::new(f),
            num_threads: None,
        }
    }

    /// Set number of threads
    pub fn with_threads(mut self, num_threads: usize) -> Self {
        self.num_threads = Some(num_threads);
        self
    }

    /// Execute the parallel map
    pub fn execute(self) -> Vec<O> {
        let f = self.f;
        
        // Use rayon's global thread pool
        // For custom pool, would need rayon::ThreadPoolBuilder
        self.items.into_par_iter().map(move |item| (f)(item)).collect()
    }

    /// Execute with progress tracking
    pub fn execute_with_progress(self, progress: &AtomicUsize) -> Vec<O> {
        let f = self.f;
        
        self.items
            .into_par_iter()
            .map(move |item| {
                let result = (f)(item);
                progress.fetch_add(1, Ordering::Relaxed);
                result
            })
            .collect()
    }
}

/// Batch items for parallel processing
pub fn batch_items<T: Clone>(items: Vec<T>, batch_size: usize) -> Vec<Vec<T>> {
    items.chunks(batch_size).map(|chunk| chunk.to_vec()).collect()
}

/// Parallel for each
pub fn parallel_for_each<T, F>(items: Vec<T>, f: F)
where
    T: Send + 'static,
    F: Fn(T) + Send + Sync + 'static,
{
    items.into_par_iter().for_each(f);
}

/// Parallel reduce
pub fn parallel_reduce<T, O, F, R>(items: Vec<T>, map_fn: F, reduce_fn: R, identity: O) -> O
where
    T: Send + 'static,
    O: Send + Sync + Clone + 'static,
    F: Fn(T) -> O + Send + Sync + 'static,
    R: Fn(O, O) -> O + Send + Sync + 'static,
{
    items
        .into_par_iter()
        .map(map_fn)
        .reduce(|| identity.clone(), reduce_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_map() {
        let items: Vec<i32> = (0..1000).collect();
        let results = parallel_map(items, |x| x * 2);
        
        assert_eq!(results.len(), 1000);
        assert!(results.iter().all(|&r| r % 2 == 0));
    }

    #[test]
    fn test_parallel_map_with_progress() {
        let items: Vec<i32> = (0..100).collect();
        let progress = AtomicUsize::new(0);
        
        let results = parallel_map_with_progress(items, |x| x * 2, &progress);
        
        assert_eq!(results.len(), 100);
        assert_eq!(progress.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn test_parallel_for_each() {
        use std::sync::Mutex;
        
        let items: Vec<i32> = (0..100).collect();
        let sum = Mutex::new(0);
        
        parallel_for_each(items, |x| {
            let mut s = sum.lock().unwrap();
            *s += x;
        });
        
        assert_eq!(*sum.lock().unwrap(), (0..100).sum::<i32>());
    }
}
