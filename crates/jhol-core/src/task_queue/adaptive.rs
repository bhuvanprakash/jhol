//! Adaptive concurrency controller
//! 
//! Automatically adjusts concurrency based on latency and throughput.

use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::sync::{RwLock, Arc};
use std::time::Duration;
use std::collections::VecDeque;

/// Configuration for adaptive concurrency
#[derive(Clone, Debug)]
pub struct ConcurrencyConfig {
    /// Minimum concurrency level
    pub min_concurrency: usize,
    /// Maximum concurrency level
    pub max_concurrency: usize,
    /// Target latency (adjust to stay below this)
    pub target_latency: Duration,
    /// Number of samples to keep for latency tracking
    pub sample_count: usize,
    /// Adjustment step (how much to change concurrency)
    pub adjustment_step: usize,
    /// Cooldown between adjustments
    pub cooldown: Duration,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            min_concurrency: 1,
            max_concurrency: 64,
            target_latency: Duration::from_millis(100),
            sample_count: 100,
            adjustment_step: 2,
            cooldown: Duration::from_millis(500),
        }
    }
}

/// Adaptive concurrency controller
pub struct AdaptiveConcurrency {
    /// Current concurrency level
    current: AtomicUsize,
    /// Configuration
    config: ConcurrencyConfig,
    /// Recent latencies (circular buffer)
    latencies: RwLock<VecDeque<Duration>>,
    /// Last adjustment time
    last_adjustment: AtomicU64,
    /// Total requests processed
    total_requests: AtomicU64,
    /// Total latency (for average calculation)
    total_latency_ms: AtomicU64,
}

impl AdaptiveConcurrency {
    /// Create a new adaptive concurrency controller
    pub fn new(initial: usize, config: ConcurrencyConfig) -> Self {
        let initial = initial.clamp(config.min_concurrency, config.max_concurrency);
        
        Self {
            current: AtomicUsize::new(initial),
            config: config.clone(),
            latencies: RwLock::new(VecDeque::with_capacity(config.sample_count)),
            last_adjustment: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
        }
    }

    /// Create with defaults
    pub fn with_defaults(initial: usize) -> Self {
        Self::new(initial, ConcurrencyConfig::default())
    }

    /// Record a latency sample
    pub fn record_latency(&self, latency: Duration) {
        // Update totals
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency.as_millis() as u64, Ordering::Relaxed);
        
        // Add to recent latencies
        {
            let mut latencies = self.latencies.write().unwrap();
            latencies.push_back(latency);
            
            // Keep only recent samples
            while latencies.len() > self.config.sample_count {
                latencies.pop_front();
            }
        }
        
        // Try to adjust
        self.try_adjust();
    }

    /// Try to adjust concurrency based on recent latencies
    fn try_adjust(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let last = self.last_adjustment.load(Ordering::Relaxed);
        if now - last < self.config.cooldown.as_millis() as u64 {
            return;
        }
        
        // Try to acquire adjustment lock
        if self.last_adjustment.compare_exchange(
            last,
            now,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ).is_err() {
            return; // Another thread is adjusting
        }
        
        // Calculate average latency
        let latencies = self.latencies.read().unwrap();
        if latencies.len() < 10 {
            return; // Not enough samples
        }
        
        let avg_latency = latencies.iter()
            .map(|d| d.as_millis() as u64)
            .sum::<u64>() / latencies.len() as u64;
        
        let current = self.current.load(Ordering::Relaxed);
        let mut new_concurrency = current;
        
        // Adjust based on target latency
        if avg_latency > self.config.target_latency.as_millis() as u64 * 2 {
            // Latency too high, reduce concurrency
            new_concurrency = current.saturating_sub(self.config.adjustment_step);
        } else if avg_latency < self.config.target_latency.as_millis() as u64 / 2 {
            // Latency very low, can increase concurrency
            new_concurrency = current + self.config.adjustment_step;
        }
        
        // Clamp to valid range
        new_concurrency = new_concurrency.clamp(
            self.config.min_concurrency,
            self.config.max_concurrency,
        );
        
        // Update if changed
        if new_concurrency != current {
            self.current.store(new_concurrency, Ordering::Relaxed);
        }
    }

    /// Get current concurrency level
    pub fn get(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }

    /// Set concurrency level manually
    pub fn set(&self, value: usize) {
        let value = value.clamp(self.config.min_concurrency, self.config.max_concurrency);
        self.current.store(value, Ordering::Relaxed);
    }

    /// Get average latency
    pub fn average_latency(&self) -> Duration {
        let total_requests = self.total_requests.load(Ordering::Relaxed);
        let total_latency = self.total_latency_ms.load(Ordering::Relaxed);
        
        if total_requests == 0 {
            return Duration::ZERO;
        }
        
        Duration::from_millis(total_latency / total_requests)
    }

    /// Get statistics
    pub fn stats(&self) -> ConcurrencyStats {
        let latencies = self.latencies.read().unwrap();
        
        let latencies_vec: Vec<u64> = latencies.iter()
            .map(|d| d.as_millis() as u64)
            .collect();
        
        let p50 = percentile(&latencies_vec, 50);
        let p95 = percentile(&latencies_vec, 95);
        let p99 = percentile(&latencies_vec, 99);
        
        ConcurrencyStats {
            current_concurrency: self.current.load(Ordering::Relaxed),
            min_concurrency: self.config.min_concurrency,
            max_concurrency: self.config.max_concurrency,
            average_latency_ms: self.average_latency().as_millis() as u64,
            p50_latency_ms: p50,
            p95_latency_ms: p95,
            p99_latency_ms: p99,
            total_requests: self.total_requests.load(Ordering::Relaxed),
        }
    }

    /// Reset statistics
    pub fn reset(&self) {
        self.latencies.write().unwrap().clear();
        self.total_requests.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
        self.last_adjustment.store(0, Ordering::Relaxed);
    }
}

/// Statistics for adaptive concurrency
#[derive(Clone, Debug, Default)]
pub struct ConcurrencyStats {
    pub current_concurrency: usize,
    pub min_concurrency: usize,
    pub max_concurrency: usize,
    pub average_latency_ms: u64,
    pub p50_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub p99_latency_ms: u64,
    pub total_requests: u64,
}

/// Calculate percentile of sorted values
fn percentile(values: &[u64], p: u64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    
    let mut sorted: Vec<u64> = values.to_vec();
    sorted.sort_unstable();
    
    let idx = (sorted.len() as u64 * p / 100) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_concurrency_basic() {
        let config = ConcurrencyConfig {
            min_concurrency: 2,
            max_concurrency: 16,
            target_latency: Duration::from_millis(100),
            ..Default::default()
        };
        
        let adaptive = AdaptiveConcurrency::new(8, config);
        
        assert_eq!(adaptive.get(), 8);
    }

    #[test]
    fn test_percentile() {
        let values = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        
        assert_eq!(percentile(&values, 50), 5);
        assert_eq!(percentile(&values, 95), 9);
        assert_eq!(percentile(&values, 99), 10);
    }
}
