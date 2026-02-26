//! Worker configuration and handles

use std::time::Duration;

/// Configuration for a worker
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Worker ID
    pub id: usize,
    /// Stack size for worker thread
    pub stack_size: usize,
    /// Whether to use real-time priority
    pub real_time_priority: bool,
    /// Idle timeout before worker yields
    pub idle_timeout: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            id: 0,
            stack_size: 2 * 1024 * 1024, // 2 MB
            real_time_priority: false,
            idle_timeout: Duration::from_micros(100),
        }
    }
}

/// Handle to a worker thread
pub struct WorkerHandle {
    /// Worker ID
    pub id: usize,
    /// Whether worker is running
    pub running: bool,
}

/// Worker state
pub struct Worker {
    /// Configuration
    pub config: WorkerConfig,
    /// Whether worker should stop
    pub should_stop: bool,
}

impl Worker {
    /// Create a new worker
    pub fn new(config: WorkerConfig) -> Self {
        Self {
            config,
            should_stop: false,
        }
    }

    /// Start the worker
    pub fn start(&mut self) -> WorkerHandle {
        WorkerHandle {
            id: self.config.id,
            running: true,
        }
    }

    /// Stop the worker
    pub fn stop(&mut self) {
        self.should_stop = true;
    }
}
