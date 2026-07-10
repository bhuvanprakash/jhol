//! File watching for `jhol run --watch`.
//!
//! Abstracts two backends behind the `Watcher` trait:
//! - `NativeWatcher` (when the `native-watcher` feature is enabled and
//!   `JHOL_WATCH_POLL` is not set): uses OS-specific events via `notify`.
//! - `PollingWatcher` (fallback): mtime polling every 200 ms.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::errors::RuntimeResult;

const POLL_INTERVAL_MS: u64 = 200;

/// Unified watcher interface.
pub trait Watcher {
    fn watch(&mut self, path: &Path) -> RuntimeResult<()>;
    fn watch_dir(&mut self, dir: &Path) -> RuntimeResult<()>;
    fn unwatch(&mut self, path: &Path);
    fn wait_for_changes(&mut self) -> RuntimeResult<Vec<PathBuf>>;
}

/// Poll-based fallback watcher.
pub struct PollingWatcher {
    tracked: HashMap<PathBuf, (std::time::SystemTime, u64)>,
    poll_interval: Duration,
}

impl PollingWatcher {
    pub fn new() -> RuntimeResult<Self> {
        Ok(Self {
            tracked: HashMap::new(),
            poll_interval: Duration::from_millis(POLL_INTERVAL_MS),
        })
    }

    pub fn with_interval(interval: Duration) -> RuntimeResult<Self> {
        Ok(Self {
            tracked: HashMap::new(),
            poll_interval: interval,
        })
    }

    fn record_mtime(&mut self, path: &Path) {
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(mtime) = meta.modified() {
                self.tracked.insert(path.to_path_buf(), (mtime, meta.len()));
            }
        }
    }

    fn poll_once(&mut self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        for (path, (old_mtime, old_size)) in &self.tracked.clone() {
            if let Ok(meta) = std::fs::metadata(path) {
                let new_mtime = meta.modified().ok();
                let new_size = meta.len();
                if new_mtime.map(|m| m != *old_mtime).unwrap_or(false) || new_size != *old_size {
                    changed.push(path.clone());
                    if let Some((mtime, size)) = self.tracked.get_mut(path) {
                        if let Some(m) = new_mtime {
                            *mtime = m;
                        }
                        *size = new_size;
                    }
                }
            }
        }
        changed
    }
}

impl Watcher for PollingWatcher {
    fn watch(&mut self, path: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.record_mtime(&canonical);
        Ok(())
    }

    fn watch_dir(&mut self, dir: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Ok(entries) = std::fs::read_dir(&canonical) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    self.record_mtime(&path);
                }
            }
        }
        Ok(())
    }

    fn unwatch(&mut self, path: &Path) {
        self.tracked.remove(path);
    }

    fn wait_for_changes(&mut self) -> RuntimeResult<Vec<PathBuf>> {
        loop {
            std::thread::sleep(self.poll_interval);
            let changes = self.poll_once();
            if !changes.is_empty() {
                return Ok(changes);
            }
        }
    }
}

#[cfg(feature = "native-watcher")]
pub struct NativeWatcher {
    watcher: notify::RecommendedWatcher,
    rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    watched: std::collections::HashSet<PathBuf>,
}

#[cfg(feature = "native-watcher")]
impl NativeWatcher {
    pub fn new() -> RuntimeResult<Box<dyn Watcher>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|e| RuntimeError::Internal(format!("notify error: {}", e)))?;
        Ok(Box::new(Self {
            watcher,
            rx,
            watched: std::collections::HashSet::new(),
        }))
    }
}

#[cfg(feature = "native-watcher")]
impl Watcher for NativeWatcher {
    fn watch(&mut self, path: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.watched.insert(canonical.clone()) {
            self.watcher
                .watch(&canonical, notify::RecursiveMode::NonRecursive)
                .map_err(|e| RuntimeError::Internal(format!("watch error: {}", e)))?;
        }
        Ok(())
    }

    fn watch_dir(&mut self, dir: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if self.watched.insert(canonical.clone()) {
            self.watcher
                .watch(&canonical, notify::RecursiveMode::Recursive)
                .map_err(|e| RuntimeError::Internal(format!("watch dir error: {}", e)))?;
        }
        Ok(())
    }

    fn unwatch(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.watched.remove(&canonical) {
            let _ = self.watcher.unwatch(&canonical);
        }
    }

    fn wait_for_changes(&mut self) -> RuntimeResult<Vec<PathBuf>> {
        loop {
            match self.rx.recv() {
                Ok(Ok(event)) => {
                    let paths: Vec<PathBuf> = event
                        .paths
                        .into_iter()
                        .filter(|p| self.watched.iter().any(|w| p.starts_with(w)))
                        .collect();
                    if !paths.is_empty() {
                        return Ok(paths);
                    }
                }
                Ok(Err(e)) => return Err(RuntimeError::Internal(format!("watch error: {}", e))),
                Err(_) => return Err(RuntimeError::Internal("watch channel closed".into())),
            }
        }
    }
}

/// Create the best available watcher for the current platform/features.
pub fn create_watcher() -> RuntimeResult<Box<dyn Watcher>> {
    #[cfg(feature = "native-watcher")]
    {
        if std::env::var("JHOL_WATCH_POLL").is_err() {
            return NativeWatcher::new();
        }
    }
    Ok(Box::new(PollingWatcher::new()?))
}

/// Callback type for watch sessions.
pub type WatchRunner = Box<dyn FnMut(&Path) -> RuntimeResult<()> + 'static>;

/// Watch session that runs a callback whenever the watched entry changes.
pub struct WatchSession {
    watcher: Box<dyn Watcher>,
    entry: PathBuf,
    runner: WatchRunner,
}

impl WatchSession {
    pub fn new<F>(entry: &Path, runner: F) -> RuntimeResult<Self>
    where
        F: FnMut(&Path) -> RuntimeResult<()> + 'static,
    {
        let mut watcher = create_watcher()?;
        watcher.watch(entry)?;
        watcher.watch_dir(entry.parent().unwrap_or_else(|| Path::new(".")))?;

        Ok(Self {
            watcher,
            entry: entry.to_path_buf(),
            runner: Box::new(runner),
        })
    }

    pub fn run(&mut self) -> RuntimeResult<()> {
        loop {
            let changes = self.watcher.wait_for_changes()?;
            if changes.iter().any(|p| p == &self.entry) {
                (self.runner)(&self.entry)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn polling_watcher_detects_change() {
        let tmp = std::env::temp_dir().join(format!("jhol-watcher-test-{}", std::process::id()));
        let file = tmp.join("foo.txt");
        std::fs::create_dir_all(&tmp).unwrap();
        {
            let mut f = std::fs::File::create(&file).unwrap();
            writeln!(f, "hello").unwrap();
        }

        let mut w = PollingWatcher::with_interval(Duration::from_millis(10)).unwrap();
        w.watch(&file).unwrap();

        // Wait a bit, then modify
        std::thread::sleep(Duration::from_millis(50));
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&file)
                .unwrap();
            writeln!(f, "world").unwrap();
        }

        let changes = w.wait_for_changes().unwrap();
        assert!(changes.iter().any(|p| p.ends_with("foo.txt")));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
