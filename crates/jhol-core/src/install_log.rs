//! Install event log — a rolling JSONL file at ~/.cache/jhol/install-log.jsonl
//!
//! Every `jhol install` (or `add`) writes one entry after completion.
//! The TUI reads these entries for the "Recent Installs" panel.
//! Entries older than MAX_ENTRY_AGE_HOURS are pruned on each write.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// How many hours of history to keep.
pub const MAX_ENTRY_AGE_HOURS: u64 = 6;
/// Maximum entries to keep in the log file (avoids unbounded growth).
const MAX_LOG_ENTRIES: usize = 200;

/// A single install operation recorded in the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallLogEntry {
    /// Unix timestamp (seconds) when the install finished.
    pub timestamp: u64,
    /// Working directory where the install ran.
    pub project_dir: String,
    /// Packages that were installed (name@version).
    pub packages: Vec<String>,
    /// Total elapsed milliseconds.
    pub elapsed_ms: u64,
    /// Number of packages served from cache (not network).
    pub cache_hits: u64,
    /// Number of network requests made.
    pub network_requests: u64,
    /// Whether the install succeeded.
    pub success: bool,
    /// Brief error summary if not successful.
    pub error: Option<String>,
}

impl InstallLogEntry {
    /// Human-readable elapsed time string (e.g. "⚡ 11ms", "1.23s").
    pub fn elapsed_label(&self) -> String {
        match self.elapsed_ms {
            0..=9 => "⚡ instant".to_string(),
            10..=99 => format!("⚡ {}ms", self.elapsed_ms),
            100..=999 => format!("{}ms", self.elapsed_ms),
            _ => format!("{:.2}s", self.elapsed_ms as f64 / 1000.0),
        }
    }

    /// Cache hit percentage (0.0–100.0).
    pub fn cache_rate(&self) -> f64 {
        let total = self.cache_hits + self.network_requests;
        if total == 0 {
            100.0
        } else {
            self.cache_hits as f64 / total as f64 * 100.0
        }
    }

    /// Short source label: "cache" | "network" | "mixed".
    pub fn source_label(&self) -> &'static str {
        if self.network_requests == 0 {
            "cache"
        } else if self.cache_hits == 0 {
            "network"
        } else {
            "mixed"
        }
    }

    /// Age in seconds relative to `now`.
    pub fn age_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.timestamp)
    }
}

/// Path to the install log file.
pub fn log_path() -> PathBuf {
    // Reuse the same cache directory as the rest of jhol.
    let base = if cfg!(target_os = "macos") {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("jhol")
            .join("global-store")
    } else if cfg!(target_os = "linux") {
        std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")))
            .join("jhol")
            .join("global-store")
    } else {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("jhol")
            .join("global-store")
    };
    base.join("install-log.jsonl")
}

/// Append one entry to the log, pruning old entries if needed.
///
/// Failures are silently ignored — the log is best-effort and must
/// never block or crash the install.
pub fn append(entry: &InstallLogEntry) {
    let path = log_path();

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Read existing entries to prune stale ones.
    let cutoff_secs = entry.timestamp.saturating_sub(MAX_ENTRY_AGE_HOURS * 3600);

    let mut kept: Vec<String> = Vec::new();
    if path.exists() {
        if let Ok(file) = std::fs::File::open(&path) {
            for line in BufReader::new(file).lines().flatten() {
                if line.trim().is_empty() {
                    continue;
                }
                // Keep the line if it's recent enough (parse timestamp cheaply).
                let is_recent = serde_json::from_str::<InstallLogEntry>(&line)
                    .map(|e| e.timestamp >= cutoff_secs)
                    .unwrap_or(false);
                if is_recent {
                    kept.push(line);
                }
            }
        }
    }

    // Append the new entry.
    if let Ok(new_line) = serde_json::to_string(entry) {
        kept.push(new_line);
    }

    // Trim to MAX_LOG_ENTRIES (keep the most recent).
    if kept.len() > MAX_LOG_ENTRIES {
        let drain_count = kept.len() - MAX_LOG_ENTRIES;
        kept.drain(0..drain_count);
    }

    // Atomic write: tmp + rename.
    // Use PID+timestamp to avoid collision when two concurrent jhol installs
    // write the log simultaneously (same-named `.tmp` would cause one to overwrite
    // the other's work before rename, silently losing an entry).
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp_path = path.with_file_name(format!("install-log.{}.{}.tmp", std::process::id(), ts));
    if let Ok(mut f) = std::fs::File::create(&tmp_path) {
        for line in &kept {
            let _ = writeln!(f, "{}", line);
        }
        let _ = f.flush();
        let _ = std::fs::rename(&tmp_path, &path);
        // If rename failed (cross-filesystem), clean up orphaned tmp
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Read all log entries, from oldest to newest, no older than MAX_ENTRY_AGE_HOURS.
pub fn read_recent() -> Vec<InstallLogEntry> {
    let path = log_path();
    if !path.exists() {
        return Vec::new();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cutoff = now.saturating_sub(MAX_ENTRY_AGE_HOURS * 3600);

    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };

    BufReader::new(file)
        .lines()
        .flatten()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<InstallLogEntry>(&l).ok())
        .filter(|e| e.timestamp >= cutoff)
        .collect()
}

/// Read entries sorted newest-first (for display in the TUI).
pub fn read_recent_newest_first() -> Vec<InstallLogEntry> {
    let mut entries = read_recent();
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_entry(timestamp: u64, packages: Vec<&str>, elapsed_ms: u64) -> InstallLogEntry {
        InstallLogEntry {
            timestamp,
            project_dir: "/tmp/test-project".to_string(),
            packages: packages.into_iter().map(|s| s.to_string()).collect(),
            elapsed_ms,
            cache_hits: 1,
            network_requests: 0,
            success: true,
            error: None,
        }
    }

    #[test]
    fn test_elapsed_label() {
        assert_eq!(mock_entry(0, vec![], 5).elapsed_label(), "⚡ instant");
        assert_eq!(mock_entry(0, vec![], 42).elapsed_label(), "⚡ 42ms");
        assert_eq!(mock_entry(0, vec![], 500).elapsed_label(), "500ms");
        assert_eq!(mock_entry(0, vec![], 1234).elapsed_label(), "1.23s");
    }

    #[test]
    fn test_cache_rate() {
        let mut e = mock_entry(0, vec![], 10);
        e.cache_hits = 3;
        e.network_requests = 1;
        assert!((e.cache_rate() - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_source_label() {
        let mut e = mock_entry(0, vec![], 10);
        e.cache_hits = 1;
        e.network_requests = 0;
        assert_eq!(e.source_label(), "cache");
        e.network_requests = 1;
        assert_eq!(e.source_label(), "mixed");
        e.cache_hits = 0;
        assert_eq!(e.source_label(), "network");
    }
}

// ── Public aliases used by lib.rs re-exports ──────────────────────────────────

/// Write an install log entry. Alias for lib.rs re-export.
pub fn append_install_log(entry: &InstallLogEntry) {
    append(entry);
}

/// Read recent entries, newest first. Alias for lib.rs re-export.
pub fn read_install_log_newest_first() -> Vec<InstallLogEntry> {
    read_recent_newest_first()
}
