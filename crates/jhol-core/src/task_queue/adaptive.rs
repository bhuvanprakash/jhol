//! Adaptive concurrency controller
//!
//! Automatically adjusts concurrency based on latency and throughput.
//! Uses running EWMA estimate — no per-sample storage, no sort.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

/// Configuration for adaptive concurrency
#[derive(Clone, Debug)]
pub struct ConcurrencyConfig {
    /// Minimum concurrency level
    pub min_concurrency: usize,
    /// Maximum concurrency level
    pub max_concurrency: usize,
    /// Target latency (adjust to stay below this)
    pub target_latency: Duration,
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
            adjustment_step: 2,
            cooldown: Duration::from_millis(500),
        }
    }
}

/// Adaptive concurrency controller (EWMA-based, no per-sample storage)
pub struct AdaptiveConcurrency {
    current: AtomicUsize,
    config: ConcurrencyConfig,
    /// EWMA of recent latency in millis (Q5.10 fixed-point: value * 1024)
    ewma_latency: AtomicU64,
    last_adjustment: AtomicU64,
    total_requests: AtomicU64,
    total_latency_ms: AtomicU64,
}

const EWMA_ALPHA: u64 = 64; // alpha = 64/128 = 0.5
const EWMA_SHIFT: u64 = 7; // 128 = 1 << 7

impl AdaptiveConcurrency {
    pub fn new(initial: usize, config: ConcurrencyConfig) -> Self {
        let initial = initial.clamp(config.min_concurrency, config.max_concurrency);
        Self {
            current: AtomicUsize::new(initial),
            config,
            ewma_latency: AtomicU64::new(0),
            last_adjustment: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
        }
    }

    pub fn with_defaults(initial: usize) -> Self {
        Self::new(initial, ConcurrencyConfig::default())
    }

    /// Record a latency sample using EWMA (no allocation, no lock)
    pub fn record_latency(&self, latency: Duration) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms
            .fetch_add(latency.as_millis() as u64, Ordering::Relaxed);

        let sample = latency.as_millis() as u64;
        let prev = self.ewma_latency.load(Ordering::Relaxed);
        let ewma = if prev == 0 {
            sample << EWMA_SHIFT
        } else {
            // ewma = alpha * sample + (1 - alpha) * prev
            // fixed-point: ewma = (EWMA_ALPHA * sample + (128-EWMA_ALPHA) * prev) >> 7
            let alpha = EWMA_ALPHA;
            ((alpha * sample) + ((128 - alpha) * prev)) >> 7
        };
        self.ewma_latency.store(ewma, Ordering::Relaxed);

        self.try_adjust();
    }

    fn try_adjust(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = self.last_adjustment.load(Ordering::Relaxed);
        if now - last < self.config.cooldown.as_millis() as u64 {
            return;
        }
        if self
            .last_adjustment
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        if self.total_requests.load(Ordering::Relaxed) < 10 {
            return;
        }

        let ewma = self.ewma_latency.load(Ordering::Relaxed) >> EWMA_SHIFT;
        let target = self.config.target_latency.as_millis() as u64;
        let current = self.current.load(Ordering::Relaxed);
        let mut new_concurrency = current;

        if ewma > target * 2 {
            new_concurrency = current.saturating_sub(self.config.adjustment_step);
        } else if ewma < target / 2 {
            new_concurrency = current + self.config.adjustment_step;
        }

        new_concurrency =
            new_concurrency.clamp(self.config.min_concurrency, self.config.max_concurrency);
        if new_concurrency != current {
            self.current.store(new_concurrency, Ordering::Relaxed);
        }
    }

    pub fn get(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }

    pub fn set(&self, value: usize) {
        let value = value.clamp(self.config.min_concurrency, self.config.max_concurrency);
        self.current.store(value, Ordering::Relaxed);
    }

    pub fn average_latency(&self) -> Duration {
        let total_requests = self.total_requests.load(Ordering::Relaxed);
        let total_latency = self.total_latency_ms.load(Ordering::Relaxed);
        if total_requests == 0 {
            return Duration::ZERO;
        }
        Duration::from_millis(total_latency / total_requests)
    }

    pub fn stats(&self) -> ConcurrencyStats {
        ConcurrencyStats {
            current_concurrency: self.current.load(Ordering::Relaxed),
            min_concurrency: self.config.min_concurrency,
            max_concurrency: self.config.max_concurrency,
            average_latency_ms: self.average_latency().as_millis() as u64,
            p50_latency_ms: self.ewma_latency.load(Ordering::Relaxed) >> EWMA_SHIFT,
            p95_latency_ms: 0,
            p99_latency_ms: 0,
            total_requests: self.total_requests.load(Ordering::Relaxed),
        }
    }

    pub fn reset(&self) {
        self.ewma_latency.store(0, Ordering::Relaxed);
        self.total_requests.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
        self.last_adjustment.store(0, Ordering::Relaxed);
    }
}

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
