use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::errors::{BundleError, BundleResult};

pub struct BundlerWatcher {
    watcher: RecommendedWatcher,
    rx: mpsc::Receiver<Event>,
    watched_paths: HashSet<PathBuf>,
    debounce: Duration,
}

impl BundlerWatcher {
    pub fn new() -> BundleResult<Self> {
        let (tx, rx) = mpsc::channel();
        let watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            Config::default(),
        )
        .map_err(|e| BundleError::Internal(format!("Failed to create watcher: {}", e)))?;

        Ok(Self {
            watcher,
            rx,
            watched_paths: HashSet::new(),
            debounce: Duration::from_millis(100),
        })
    }

    pub fn watch_files(&mut self, files: &[PathBuf]) -> BundleResult<()> {
        for file in files {
            let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
            if let Some(parent) = canonical.parent() {
                if !self.watched_paths.contains(parent) {
                    self.watcher.watch(parent, RecursiveMode::NonRecursive).ok();
                    self.watched_paths.insert(parent.to_path_buf());
                }
            }
        }
        Ok(())
    }

    pub fn watch_directory(&mut self, dir: &Path) -> BundleResult<()> {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());

        if !self.watched_paths.contains(&canonical) {
            self.watcher
                .watch(&canonical, RecursiveMode::Recursive)
                .map_err(|e| BundleError::Internal(format!("Watch error: {}", e)))?;
            self.watched_paths.insert(canonical);
        }
        Ok(())
    }

    pub fn wait_for_changes(&mut self) -> Vec<PathBuf> {
        let mut changes = Vec::new();
        let mut last_event = std::time::Instant::now();

        loop {
            match self.rx.recv_timeout(self.debounce) {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    ) {
                        for path in event.paths {
                            if !path.components().any(|c| c.as_os_str() == "node_modules") {
                                changes.push(path);
                            }
                        }
                    }
                    last_event = std::time::Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !changes.is_empty() && last_event.elapsed() >= self.debounce {
                        return changes;
                    }
                    if changes.is_empty() {
                        continue;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return changes,
            }
        }
    }
}

pub struct WatchBuild {
    pub watcher: BundlerWatcher,
    builder: Box<dyn FnMut() -> BundleResult<()> + Send>,
}

impl WatchBuild {
    pub fn new<F>(builder: F) -> BundleResult<Self>
    where
        F: FnMut() -> BundleResult<()> + Send + 'static,
    {
        Ok(Self {
            watcher: BundlerWatcher::new()?,
            builder: Box::new(builder),
        })
    }

    pub fn run(&mut self) -> BundleResult<()> {
        println!("Building...");
        let start = std::time::Instant::now();
        (self.builder)()?;
        let elapsed = start.elapsed();
        println!(
            "Build complete in {}.{}s",
            elapsed.as_secs(),
            elapsed.subsec_millis()
        );

        if self.watcher.watched_paths.is_empty() {
            println!("No files to watch. Exiting.");
            return Ok(());
        }

        println!("Watching for changes...");
        loop {
            let changes = self.watcher.wait_for_changes();
            if changes.is_empty() {
                continue;
            }

            println!("\n── Change detected, rebuilding ──");

            std::thread::sleep(Duration::from_millis(50));

            let start = std::time::Instant::now();
            match (self.builder)() {
                Ok(()) => {
                    let elapsed = start.elapsed();
                    println!(
                        "Rebuilt in {}.{}s",
                        elapsed.as_secs(),
                        elapsed.subsec_millis()
                    );
                    println!("Watching for changes...");
                }
                Err(e) => {
                    eprintln!("Build error: {}", e);
                    println!("Watching for changes...");
                }
            }
        }
    }
}
