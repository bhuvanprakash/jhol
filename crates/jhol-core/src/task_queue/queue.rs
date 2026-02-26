//! JAGR-2: Work-Stealing Task Queue
//! 
//! High-performance task queue with work-stealing for maximum parallelism.

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crossbeam::deque::{Stealer, Worker as DequeWorker, Steal};
use crossbeam::channel::{bounded, Sender, Receiver};

/// Task queue with work-stealing
pub struct TaskQueue<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    /// Worker deques (one per worker)
    workers: Arc<Vec<DequeWorker<T>>>,
    /// Stealers (for work-stealing)
    stealers: Vec<Stealer<T>>,
    /// Result sender
    result_tx: Sender<R>,
    /// Result receiver
    result_rx: Receiver<R>,
    /// Number of workers
    worker_count: usize,
    /// Shutdown flag
    shutdown: Arc<AtomicBool>,
    /// Worker handles
    handles: Vec<JoinHandle<()>>,
}

/// Builder for TaskQueue
pub struct TaskQueueBuilder<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    worker_count: usize,
    queue_capacity: usize,
    _phantom: std::marker::PhantomData<(T, R)>,
}

impl<T, R> TaskQueueBuilder<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            worker_count: num_cpus::get().max(1),
            queue_capacity: 4096,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set number of workers
    pub fn workers(mut self, count: usize) -> Self {
        self.worker_count = count.max(1);
        self
    }

    /// Build the task queue
    pub fn build<F>(self, task_fn: Arc<F>) -> TaskQueue<T, R>
    where
        F: Fn(T) -> R + Send + Sync + 'static,
    {
        TaskQueue::new(self.worker_count, task_fn)
    }
}

impl<T, R> Default for TaskQueueBuilder<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, R> TaskQueue<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    /// Create a new task queue
    pub fn new<F>(worker_count: usize, task_fn: Arc<F>) -> Self
    where
        F: Fn(T) -> R + Send + Sync + 'static,
    {
        let (result_tx, result_rx) = bounded(worker_count * 4);

        let mut stealers = Vec::with_capacity(worker_count);
        let mut workers_vec = Vec::with_capacity(worker_count);

        // Create worker deques
        for _ in 0..worker_count {
            let worker = DequeWorker::new_fifo();
            let stealer = worker.stealer();
            stealers.push(stealer);
            workers_vec.push(worker);
        }
        
        let workers = Arc::new(workers_vec);
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(worker_count);

        // Spawn workers
        for worker_id in 0..worker_count {
            let stealers_clone = stealers.clone();
            let result_tx_clone = result_tx.clone();
            let shutdown_clone = shutdown.clone();
            let task_fn_clone = task_fn.clone();
            let workers_clone = Arc::clone(&workers);

            let handle = thread::spawn(move || {
                Self::worker_loop(
                    worker_id,
                    workers_clone,
                    stealers_clone,
                    result_tx_clone,
                    shutdown_clone,
                    task_fn_clone,
                );
            });

            handles.push(handle);
        }

        Self {
            workers,
            stealers,
            result_tx,
            result_rx,
            worker_count,
            shutdown,
            handles,
        }
    }

    /// Worker main loop
    fn worker_loop<F>(
        worker_id: usize,
        workers: Arc<Vec<DequeWorker<T>>>,
        stealers: Vec<Stealer<T>>,
        result_tx: Sender<R>,
        shutdown: Arc<AtomicBool>,
        task_fn: Arc<F>,
    )
    where
        F: Fn(T) -> R + Send + Sync + 'static,
    {
        while !shutdown.load(Ordering::Relaxed) {
            // Try to get task from local deque first
            match workers[worker_id].pop() {
                Steal::Success(task) => {
                    let result = (task_fn)(task);
                    let _ = result_tx.send(result);
                }
                Steal::Empty | Steal::Retry => {
                    // Local deque empty, try to steal
                    match Self::try_steal(&stealers, worker_id) {
                        Steal::Success(task) => {
                            let result = (task_fn)(task);
                            let _ = result_tx.send(result);
                        }
                        Steal::Empty | Steal::Retry => {
                            thread::yield_now();
                        }
                    }
                }
            }
        }
    }

    /// Try to steal work from other workers
    fn try_steal(stealers: &[Stealer<T>], worker_id: usize) -> Steal<T> {
        let mut indices: Vec<usize> = (0..stealers.len()).filter(|&i| i != worker_id).collect();
        
        // Simple shuffle
        for i in (1..indices.len()).rev() {
            let j = rand::random::<usize>() % (i + 1);
            indices.swap(i, j);
        }
        
        for &idx in &indices {
            match stealers[idx].steal() {
                Steal::Success(task) => return Steal::Success(task),
                Steal::Empty | Steal::Retry => continue,
            }
        }
        
        Steal::Empty
    }

    /// Submit a task to the queue
    pub fn submit(&self, task: T) {
        let worker_idx = rand::random::<usize>() % self.worker_count;
        self.workers[worker_idx].push(task);
    }

    /// Submit multiple tasks
    pub fn submit_batch(&self, tasks: Vec<T>) {
        for (i, task) in tasks.into_iter().enumerate() {
            let worker_idx = i % self.worker_count;
            self.workers[worker_idx].push(task);
        }
    }

    /// Get result from queue
    pub fn recv(&self) -> Result<R, crossbeam::channel::RecvError> {
        self.result_rx.recv()
    }

    /// Get result with timeout
    pub fn recv_timeout(&self, timeout: Duration) -> Result<R, crossbeam::channel::RecvTimeoutError> {
        self.result_rx.recv_timeout(timeout)
    }

    /// Try to get result (non-blocking)
    pub fn try_recv(&self) -> Result<R, crossbeam::channel::TryRecvError> {
        self.result_rx.try_recv()
    }

    /// Get number of workers
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Shutdown the queue
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl<T, R> Drop for TaskQueue<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Simple parallel map using work-stealing queue
pub fn parallel_map<I, O, F>(items: Vec<I>, f: F, worker_count: usize) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    let task_fn = Arc::new(f);
    let queue = TaskQueue::new(worker_count, task_fn);
    
    let item_count = items.len();
    queue.submit_batch(items);
    
    let mut results = Vec::with_capacity(item_count);
    for _ in 0..item_count {
        if let Ok(result) = queue.recv_timeout(Duration::from_secs(300)) {
            results.push(result);
        }
    }
    
    queue.shutdown();
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_queue_basic() {
        let task_fn = Arc::new(|x: i32| x * 2);
        let queue = TaskQueue::new(4, task_fn);
        
        for i in 0..10 {
            queue.submit(i);
        }
        
        let mut results = Vec::new();
        for _ in 0..10 {
            if let Ok(result) = queue.recv_timeout(Duration::from_secs(5)) {
                results.push(result);
            }
        }
        
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_parallel_map() {
        let items: Vec<i32> = (0..100).collect();
        let results = parallel_map(items, |x| x * 2, 4);
        
        assert_eq!(results.len(), 100);
        assert!(results.iter().all(|&r| r % 2 == 0));
    }
}
