//! Live install progress — lock-free atomics.

use std::sync::atomic::{AtomicU64, Ordering};

static DOWNLOADED: AtomicU64 = AtomicU64::new(0);
static FROM_CACHE: AtomicU64 = AtomicU64::new(0);
static FROM_NETWORK: AtomicU64 = AtomicU64::new(0);
static LINKED: AtomicU64 = AtomicU64::new(0);
static EXTRACTED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);
static BYTES_DOWNLOADED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default)]
pub struct ProgressSnapshot {
    pub downloaded: u64,
    pub from_cache: u64,
    pub from_network: u64,
    pub linked: u64,
    pub extracted: u64,
    pub failed: u64,
    pub total: u64,
    pub bytes_downloaded: u64,
}

impl ProgressSnapshot {
    pub fn cache_rate(&self) -> f64 {
        let done = self.from_cache + self.from_network;
        if done == 0 {
            100.0
        } else {
            (self.from_cache as f64 / done as f64) * 100.0
        }
    }
    pub fn downloaded_label(&self) -> String {
        format_download_size(self.bytes_downloaded)
    }
}

fn format_download_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Set by the CLI while a live progress bar owns the terminal. Log lines are then
/// written with a clear-to-end-of-line prefix, so a warning while the bar is ticking
/// does not leave a mangled half-line on screen.
static BAR_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_bar_active(active: bool) {
    BAR_ACTIVE.store(active, std::sync::atomic::Ordering::Relaxed);
}

pub fn bar_active() -> bool {
    BAR_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn snapshot() -> ProgressSnapshot {
    ProgressSnapshot {
        downloaded: DOWNLOADED.load(Ordering::Relaxed),
        from_cache: FROM_CACHE.load(Ordering::Relaxed),
        from_network: FROM_NETWORK.load(Ordering::Relaxed),
        linked: LINKED.load(Ordering::Relaxed),
        extracted: EXTRACTED.load(Ordering::Relaxed),
        failed: FAILED.load(Ordering::Relaxed),
        total: TOTAL.load(Ordering::Relaxed),
        bytes_downloaded: BYTES_DOWNLOADED.load(Ordering::Relaxed),
    }
}

pub fn mark_downloaded() {
    DOWNLOADED.fetch_add(1, Ordering::Relaxed);
}
pub fn mark_from_cache() {
    FROM_CACHE.fetch_add(1, Ordering::Relaxed);
}
pub fn mark_from_network() {
    FROM_NETWORK.fetch_add(1, Ordering::Relaxed);
}
pub fn mark_linked() {
    LINKED.fetch_add(1, Ordering::Relaxed);
}
pub fn mark_extracted() {
    EXTRACTED.fetch_add(1, Ordering::Relaxed);
}
pub fn mark_failed() {
    FAILED.fetch_add(1, Ordering::Relaxed);
}
pub fn set_total(n: u64) {
    TOTAL.store(n, Ordering::Relaxed);
}

/// Reset all counters at the start of a new install.
pub fn reset() {
    DOWNLOADED.store(0, Ordering::Relaxed);
    FROM_CACHE.store(0, Ordering::Relaxed);
    FROM_NETWORK.store(0, Ordering::Relaxed);
    LINKED.store(0, Ordering::Relaxed);
    EXTRACTED.store(0, Ordering::Relaxed);
    FAILED.store(0, Ordering::Relaxed);
    TOTAL.store(0, Ordering::Relaxed);
    BYTES_DOWNLOADED.store(0, Ordering::Relaxed);
}
pub fn add_bytes(n: u64) {
    BYTES_DOWNLOADED.fetch_add(n, Ordering::Relaxed);
}
