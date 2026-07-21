# Jhol Runtime — Event Loop & Native File Watcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `setTimeout`/`setInterval`/`setImmediate` actually execute, and replace the polling file watcher with native OS events.

**Architecture:** Add a timer heap inside `Runtime`. `run_event_loop()` drains timers and evaluates their JS callbacks on the main QuickJS thread. Abstract the file watcher behind a trait with two backends: native `notify` and the existing polling fallback.

**Tech Stack:** Rust 2021, `std::collections::BinaryHeap`, `crossbeam-channel` (or `std::sync::mpsc`), `notify` crate behind `native-watcher` feature.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/jhol-runtime/src/engine.rs` | Adds `TimerHeap`, `Runtime::run_event_loop()` implementation, timer callbacks. |
| `crates/jhol-runtime/src/watch.rs` | Adds `Watcher` trait + `PollingWatcher` and `NativeWatcher` implementations. |
| `crates/jhol-runtime/src/lib.rs` | No signature changes. |
| `crates/jhol-runtime/Cargo.toml` | Adds `notify` under `native-watcher` feature. |

---

### Task 1: Add notify Dependency

**Files:**
- Modify: `crates/jhol-runtime/Cargo.toml`

- [ ] **Step 1: Add native-watcher feature and notify**

```toml
[features]
default = ["native-engine"]
native-engine = ["dep:quickjs-rusty"]
native-watcher = ["dep:notify"]

[dependencies]
# ... existing ...
notify = { version = "6.1", optional = true }
```

- [ ] **Step 2: Check it compiles**

```bash
cargo check -p jhol-runtime --features native-watcher
```

Expected: resolves new crate.

---

### Task 2: Implement Timer Heap and Event Loop

**Files:**
- Modify: `crates/jhol-runtime/src/engine.rs`

- [ ] **Step 1: Add timer types**

Add near the top of `engine.rs`:

```rust
use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct TimerEntry {
    id: u32,
    deadline: Instant,
    callback: String,
    repeat: Option<Duration>,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool { self.deadline == other.deadline && self.id == other.id }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap by deadline, then by id for determinism
        other.deadline.cmp(&self.deadline).then_with(|| other.id.cmp(&self.id))
    }
}

struct TimerState {
    next_id: u32,
    timers: BinaryHeap<Reverse<TimerEntry>>,
    cancelled: std::collections::HashSet<u32>,
    ready: Vec<TimerEntry>,
}

impl TimerState {
    fn new() -> Self {
        Self {
            next_id: 0,
            timers: BinaryHeap::new(),
            cancelled: std::collections::HashSet::new(),
            ready: Vec::new(),
        }
    }

    fn schedule(&mut self, callback: String, delay: Duration, repeat: Option<Duration>) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        let entry = TimerEntry {
            id,
            deadline: Instant::now() + delay,
            callback,
            repeat,
        };
        self.timers.push(Reverse(entry));
        id
    }

    fn cancel(&mut self, id: u32) {
        self.cancelled.insert(id);
    }

    fn collect_ready(&mut self) {
        let now = Instant::now();
        while let Some(Reverse(entry)) = self.timers.peek() {
            if entry.deadline > now { break; }
            let entry = self.timers.pop().unwrap().0;
            if !self.cancelled.remove(&entry.id) {
                if let Some(interval) = entry.repeat {
                    self.timers.push(Reverse(TimerEntry {
                        id: entry.id,
                        deadline: now + interval,
                        callback: entry.callback.clone(),
                        repeat: Some(interval),
                    }));
                }
                self.ready.push(entry);
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.timers.peek().map(|Reverse(e)| e.deadline)
    }
}
```

- [ ] **Step 2: Add timer state to Runtime**

```rust
pub struct Runtime {
    context: Context,
    working_dir: PathBuf,
    exit_code: i32,
    file_cache: HashMap<PathBuf, (SystemTime, Arc<str>)>,
    timers: TimerState,
}
```

Initialize in `Runtime::new`:

```rust
let mut rt = Self {
    context,
    working_dir: cwd,
    exit_code: 0,
    file_cache: HashMap::new(),
    timers: TimerState::new(),
};
```

- [ ] **Step 3: Rewrite __jhol_set_timeout callback**

```rust
let _ = self.context.add_callback("__jhol_set_timeout", |fn_str: String, ms: i32| -> i32 {
    if let Some(state) = RUNTIME_STATE.get() {
        // We cannot directly mutate Runtime from here because it's not 'static.
        // Use a thread-local or static queue to schedule timers.
        schedule_timer(fn_str, Duration::from_millis(ms.max(0) as u64), None)
    } else {
        0
    }
});
```

Because callbacks are `Fn` and `Runtime` is not `'static`, we need a static channel for timer scheduling. Add:

```rust
use std::sync::Mutex;

static TIMER_QUEUE: OnceLock<Mutex<Vec<TimerEntry>>> = OnceLock::new();

fn init_timer_queue() {
    let _ = TIMER_QUEUE.set(Mutex::new(Vec::new()));
}

fn schedule_timer(callback: String, delay: Duration, repeat: Option<Duration>) -> u32 {
    static NEXT_ID: AtomicU32 = AtomicU32::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            q.push(TimerEntry {
                id,
                deadline: Instant::now() + delay,
                callback,
                repeat,
            });
        }
    }
    id
}
```

Call `init_timer_queue()` in `Runtime::new`.

- [ ] **Step 4: Drain static timer queue into Runtime**

Add a method on `Runtime`:

```rust
fn ingest_external_timers(&mut self) {
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            for entry in q.drain(..) {
                self.timers.timers.push(Reverse(entry));
            }
        }
    }
}
```

- [ ] **Step 5: Implement run_event_loop**

```rust
pub fn run_event_loop(&mut self) -> RuntimeResult<i32> {
    loop {
        self.ingest_external_timers();
        self.timers.collect_ready();

        // Execute all ready timers
        while let Some(entry) = self.timers.ready.pop() {
            let wrapped = format!(
                "(function() {{ var __filename = '{}'; var __dirname = '{}'; ({}).call(globalThis); }})();",
                self.working_dir.join("<timer>").to_string_lossy().replace("\\", "/"),
                self.working_dir.to_string_lossy().replace("\\", "/"),
                entry.callback
            );
            let _ = self.context.eval(&wrapped, false);
            // Re-ingest in case the timer scheduled more timers
            self.ingest_external_timers();
        }

        // Determine next wait
        self.ingest_external_timers();
        if let Some(next) = self.timers.next_deadline() {
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
        } else {
            // No more timers; exit event loop
            break;
        }
    }
    Ok(self.exit_code)
}
```

- [ ] **Step 6: Update JS timer polyfills**

In the bootstrap JS, update `setTimeout`/`setInterval`/`setImmediate` to pass the function source:

```javascript
globalThis.setTimeout = function(fn, ms) {
    var id = ++globalThis._jhol_timer_id;
    var fn_str = fn.toString();
    __jhol_set_timeout(fn_str, ms || 0);
    return id;
};
globalThis.clearTimeout = function(id) { delete globalThis._jhol_timers[id]; __jhol_cancel_timer(id); };
globalThis.setInterval = function(fn, ms) {
    var id = ++globalThis._jhol_timer_id;
    var fn_str = fn.toString();
    __jhol_set_interval(fn_str, ms || 0);
    return id;
};
globalThis.clearInterval = function(id) { delete globalThis._jhol_intervals[id]; __jhol_cancel_timer(id); };
globalThis.setImmediate = function(fn) {
    var args = Array.prototype.slice.call(arguments, 1);
    var id = ++globalThis._jhol_timer_id;
    var fn_str = 'function() { (' + fn.toString() + ').apply(null, ' + JSON.stringify(args) + '); }';
    __jhol_set_timeout(fn_str, 0);
    return id;
};
```

- [ ] **Step 7: Add interval and cancel callbacks**

```rust
let _ = self.context.add_callback("__jhol_set_interval", |fn_str: String, ms: i32| -> i32 {
    schedule_timer(fn_str, Duration::from_millis(ms.max(0) as u64), Some(Duration::from_millis(ms.max(0) as u64)))
});

let _ = self.context.add_callback("__jhol_cancel_timer", |id: i32| -> bool {
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            q.retain(|e| e.id != id as u32);
        }
    }
    true
});
```

- [ ] **Step 8: Run tests**

```bash
cargo test -p jhol-runtime
```

Expected: existing tests pass.

---

### Task 3: Implement Native File Watcher

**Files:**
- Modify: `crates/jhol-runtime/src/watch.rs`

- [ ] **Step 1: Define Watcher trait and polling backend**

Rewrite `watch.rs`:

```rust
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::errors::{RuntimeError, RuntimeResult};

pub trait Watcher: Send {
    fn watch(&mut self, path: &Path) -> RuntimeResult<()>;
    fn watch_dir(&mut self, dir: &Path) -> RuntimeResult<()>;
    fn unwatch(&mut self, path: &Path);
    fn wait_for_changes(&mut self) -> RuntimeResult<Vec<PathBuf>>;
}

pub struct PollingWatcher {
    tracked: std::collections::HashMap<PathBuf, (std::time::SystemTime, u64)>,
    poll_interval: Duration,
}

impl PollingWatcher {
    pub fn new() -> Self {
        Self {
            tracked: std::collections::HashMap::new(),
            poll_interval: Duration::from_millis(200),
        }
    }

    pub fn with_interval(interval: Duration) -> Self {
        Self {
            tracked: std::collections::HashMap::new(),
            poll_interval: interval,
        }
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
                    if let Some((ref mut mtime, ref mut size)) = self.tracked.get_mut(path) {
                        if let Some(m) = new_mtime { *mtime = m; }
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

pub fn create_watcher() -> RuntimeResult<Box<dyn Watcher>> {
    #[cfg(feature = "native-watcher")]
    {
        if std::env::var("JHOL_WATCH_POLL").is_err() {
            return NativeWatcher::new();
        }
    }
    Ok(Box::new(PollingWatcher::new()))
}
```

- [ ] **Step 2: Implement NativeWatcher with notify**

Add under `#[cfg(feature = "native-watcher")]`:

```rust
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
        }).map_err(|e| RuntimeError::Internal(format!("notify error: {}", e)))?;
        Ok(Box::new(Self { watcher, rx, watched: std::collections::HashSet::new() }))
    }
}

#[cfg(feature = "native-watcher")]
impl Watcher for NativeWatcher {
    fn watch(&mut self, path: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.watched.insert(canonical.clone()) {
            self.watcher.watch(&canonical, notify::RecursiveMode::NonRecursive)
                .map_err(|e| RuntimeError::Internal(format!("watch error: {}", e)))?;
        }
        Ok(())
    }

    fn watch_dir(&mut self, dir: &Path) -> RuntimeResult<()> {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if self.watched.insert(canonical.clone()) {
            self.watcher.watch(&canonical, notify::RecursiveMode::Recursive)
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
                    let paths: Vec<PathBuf> = event.paths.into_iter()
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
```

- [ ] **Step 3: Update WatchSession to use trait**

```rust
pub struct WatchSession {
    watcher: Box<dyn Watcher>,
    entry: PathBuf,
    runner: Box<dyn FnMut(&Path) -> RuntimeResult<()> + 'static>,
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
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p jhol-runtime --features native-watcher
```

Expected: passes.

Also run without feature:

```bash
cargo test -p jhol-runtime
```

Expected: passes (uses polling watcher).

- [ ] **Step 5: Commit**

```bash
git add crates/jhol-runtime/src/engine.rs crates/jhol-runtime/src/watch.rs crates/jhol-runtime/Cargo.toml
git commit -m "feat(runtime): real timers and native file watcher"
```

---

## Self-Review Checklist

- [ ] Spec coverage: working timers, native watcher, polling fallback.
- [ ] No placeholders: every callback and watcher method provided.
- [ ] Type consistency: `Runtime` owns `TimerState`; `Watcher` trait used uniformly.
- [ ] Tests: existing runtime tests pass with and without `native-watcher`.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-08-jhol-runtime-event-loop-and-watcher.md`.**

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach?
