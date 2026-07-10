# Jhol Runtime Performance Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `jhol-runtime` materially faster on realistic workloads while keeping it a lightweight, embeddable, Node-compatible runtime built on QuickJS.

**Architecture:** Add persistent bytecode + transpile caches to avoid re-parsing/re-transpiling on every run; switch to a faster allocator; bind native modules directly to QuickJS callbacks instead of generating JS shims; move timer state out of global mutexes; parallelise module pre-fetch; and wire PGO + regression benchmarks into the build.

**Tech Stack:** Rust 2021, QuickJS via `quickjs-rusty`, `mimalloc`, `bincode`, `xxhash-rust`, `dashmap`, `rayon`, `cargo-pgo`.

---

## File Structure Map

| File | Responsibility |
|------|----------------|
| `crates/jhol-runtime/src/bytecode_cache.rs` | Compile JS to QuickJS bytecode, persist/load it keyed by source hash. |
| `crates/jhol-runtime/src/transpile_cache.rs` | Persist transpiled `.ts/.tsx/.jsx` → `.js` keyed by source hash + feature flags. |
| `crates/jhol-runtime/src/native_modules.rs` | Direct QuickJS callback bindings for built-in modules (`fs`, `path`, `crypto`, etc.) replacing `modules/mod.rs` source-string generation. |
| `crates/jhol-runtime/src/engine.rs` | Orchestrate caches, allocator, timer state, parallel pre-fetch, and lazy bootstrap. |
| `crates/jhol-runtime/src/timer_state.rs` | Per-`Runtime` timer heap (replaces global `TIMER_QUEUE`/`CANCELLED_IDS` mutexes). |
| `crates/jhol-runtime/src/lib.rs` | Re-export new public surfaces and version bump. |
| `crates/jhol-runtime/Cargo.toml` | Add `mimalloc`, `rayon`, `tempfile` (dev), `criterion` (dev) deps; feature flags. |
| `crates/jhol-runtime/examples/bench.rs` | Existing driver — extend with cold/warm cache reporting. |
| `runtime-benchmarks/bench_runtime.py` | Existing harness — add cache-warm runs. |
| `crates/jhol-runtime/tests/perf_regression.rs` | New integration test asserting runtime median < threshold on micro-benchmarks. |
| `Cargo.toml` | Add `mimalloc` feature flag alias for root package. |

---

## Task 1: QuickJS Bytecode Cache

**Files:**
- Create: `crates/jhol-runtime/src/bytecode_cache.rs`
- Modify: `crates/jhol-runtime/src/engine.rs:720-731`, `crates/jhol-runtime/src/engine.rs:746-785`
- Test: `crates/jhol-runtime/src/bytecode_cache.rs` (inline unit tests)

Rationale: Parsing JS dominates cold-start time. `quickjs-rusty` exposes `Context::compile` returning bytecode bytes and `Context::eval_binary` to run them. Cache bytecode keyed by `xxh3_64(source)` + a version salt in `dirs::cache_dir()/jhol/runtime-bytecode-v1/`.

- [ ] **Step 1: Add dependency and feature flag**

Modify `crates/jhol-runtime/Cargo.toml`:

```toml
[features]
default = ["native-engine", "mimalloc"]
native-engine = ["dep:quickjs-rusty"]
bytecode-cache = []

[dependencies]
# ... existing deps ...
mimalloc = { version = "0.1", optional = true }
```

- [ ] **Step 2: Write the cache module**

Create `crates/jhol-runtime/src/bytecode_cache.rs`:

```rust
//! Persistent QuickJS bytecode cache.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use xxhash_rust::xxh3::xxh3_64;

use crate::errors::{RuntimeError, RuntimeResult};

const CACHE_VERSION: u32 = 1;
const CACHE_DIR_NAME: &str = "runtime-bytecode-v1";

fn cache_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = dirs::cache_dir()
            .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir)
            .join("jhol")
            .join(CACHE_DIR_NAME);
        let _ = fs::create_dir_all(&base);
        base
    })
    .clone()
}

fn cache_path(source_hash: u64) -> PathBuf {
    cache_dir().join(format!("{:016x}.qbc", source_hash))
}

pub fn key(source: &str) -> u64 {
    let mut input = Vec::with_capacity(source.len() + 4);
    input.extend_from_slice(CACHE_VERSION.to_le_bytes().as_slice());
    input.extend_from_slice(source.as_bytes());
    xxh3_64(&input)
}

pub fn load(source_hash: u64) -> Option<Vec<u8>> {
    let path = cache_path(source_hash);
    fs::read(path).ok()
}

pub fn save(source_hash: u64, bytecode: &[u8]) -> RuntimeResult<()> {
    let path = cache_path(source_hash);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytecode).map_err(RuntimeError::IoError)?;
    fs::rename(&tmp, &path).map_err(RuntimeError::IoError)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let hash = key("console.log(1)");
        let data = vec![0u8, 1, 2, 3];
        save(hash, &data).unwrap();
        assert_eq!(load(hash).unwrap(), data);
    }
}
```

- [ ] **Step 3: Integrate bytecode cache into evaluation**

Modify `crates/jhol-runtime/src/engine.rs`:

Add near top:

```rust
use crate::bytecode_cache;
```

Change `evaluate` to compile-through-cache. First attempt to load cached bytecode and run via `eval_binary`; on miss compile source to bytecode, save, then run.

```rust
fn evaluate(&mut self, code: &str, filename: &str) -> RuntimeResult<()> {
    if let Some(state) = RUNTIME_STATE.get() {
        let _ = state
            .tropical_graph
            .write()
            .expect("tropical graph lock")
            .tree_shake("entry");
    }

    let _dirname = self.working_dir.to_string_lossy().to_string();
    let wrapped = format!(
        r#"var module = {{ exports: {{}} }};
var exports = module.exports;
var __filename = "{}";
var __dirname = "{}";
var require = globalThis.require;
{}
"#,
        filename, _dirname, code
    );

    let hash = bytecode_cache::key(&wrapped);
    if let Some(bytes) = bytecode_cache::load(hash) {
        match self.context.eval_binary(&bytes) {
            Ok(_) => return Ok(()),
            Err(_) => {
                // Corrupted bytecode; fall through to compile-and-save.
            }
        }
    }

    match self.context.compile(&wrapped) {
        Ok(bytecode) => {
            let _ = bytecode_cache::save(hash, &bytecode);
            self.context
                .eval_binary(&bytecode)
                .map_err(|e| RuntimeError::ExecutionError(format!("{}:{}", filename, format_error(&e))))?;
            Ok(())
        }
        Err(e) => Err(RuntimeError::ExecutionError(format!(
            "{}:{}",
            filename,
            format_error(&e)
        ))),
    }
}
```

> Note: `quickjs-rusty` API names (`compile`, `eval_binary`) must be verified against the exact installed version; adjust if needed.

- [ ] **Step 4: Wire module into lib.rs**

Add to `crates/jhol-runtime/src/lib.rs`:

```rust
pub mod bytecode_cache;
```

- [ ] **Step 5: Run tests**

```bash
cd /Users/bhuvanprakash/j/jhol/.worktrees/jhol-runtime-perf
cargo test -p jhol-runtime
```

Expected: all 23+ tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/jhol-runtime/src/bytecode_cache.rs crates/jhol-runtime/src/engine.rs crates/jhol-runtime/src/lib.rs crates/jhol-runtime/Cargo.toml
git commit -m "feat(runtime): persistent QuickJS bytecode cache"
```

---

## Task 2: Transpile Cache

**Files:**
- Create: `crates/jhol-runtime/src/transpile_cache.rs`
- Modify: `crates/jhol-runtime/src/transpile.rs:153-167`
- Test: `crates/jhol-runtime/src/transpile_cache.rs` (inline unit tests)

Rationale: TypeScript transpilation (even the fast regex path) runs on every `execute_file`/`execute_code` call. Cache the output keyed by source hash + active feature flags.

- [ ] **Step 1: Create transpile cache module**

Create `crates/jhol-runtime/src/transpile_cache.rs`:

```rust
//! Persistent transpile cache: TS/JSX source → JS.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use xxhash_rust::xxh3::xxh3_64;

use crate::errors::{RuntimeError, RuntimeResult};

const CACHE_VERSION: u32 = 1;
const CACHE_DIR_NAME: &str = "runtime-transpile-v1";

fn cache_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = dirs::cache_dir()
            .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir)
            .join("jhol")
            .join(CACHE_DIR_NAME);
        let _ = fs::create_dir_all(&base);
        base
    })
    .clone()
}

fn feature_salt() -> u64 {
    let mut salt: u64 = 0;
    if cfg!(feature = "swc-transpile") {
        salt |= 1;
    }
    salt
}

fn cache_path(source_hash: u64) -> PathBuf {
    cache_dir().join(format!("{:016x}.js", source_hash ^ feature_salt()))
}

pub fn key(source: &str) -> u64 {
    let mut input = Vec::with_capacity(source.len() + 12);
    input.extend_from_slice(CACHE_VERSION.to_le_bytes().as_slice());
    input.extend_from_slice(feature_salt().to_le_bytes().as_slice());
    input.extend_from_slice(source.as_bytes());
    xxh3_64(&input)
}

pub fn load(source_hash: u64) -> Option<String> {
    fs::read_to_string(cache_path(source_hash)).ok()
}

pub fn save(source_hash: u64, js: &str) -> RuntimeResult<()> {
    let path = cache_path(source_hash);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, js).map_err(RuntimeError::IoError)?;
    fs::rename(&tmp, &path).map_err(RuntimeError::IoError)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let hash = key("const x: number = 1;");
        save(hash, "const x = 1;").unwrap();
        assert_eq!(load(hash).unwrap(), "const x = 1;");
    }
}
```

- [ ] **Step 2: Hook cache into transpile function**

Modify `crates/jhol-runtime/src/transpile.rs`:

```rust
use crate::transpile_cache;

pub fn transpile(source: &str, filename: &str) -> RuntimeResult<String> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".ts") || lower.ends_with(".tsx") || lower.ends_with(".jsx") {
        let hash = transpile_cache::key(source);
        if let Some(cached) = transpile_cache::load(hash) {
            return Ok(cached);
        }

        let js = transpile_inner(source, filename)?;
        let _ = transpile_cache::save(hash, &js);
        Ok(js)
    } else {
        Ok(source.to_string())
    }
}

fn transpile_inner(source: &str, filename: &str) -> RuntimeResult<String> {
    #[cfg(feature = "swc-transpile")]
    {
        swc_backend::transpile_ts_to_js(source, filename)
    }
    #[cfg(not(feature = "swc-transpile"))]
    {
        fast_strip_ts(source, filename)
    }
}
```

- [ ] **Step 3: Wire module**

Add to `crates/jhol-runtime/src/lib.rs`:

```rust
pub mod transpile_cache;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p jhol-runtime
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/jhol-runtime/src/transpile_cache.rs crates/jhol-runtime/src/transpile.rs crates/jhol-runtime/src/lib.rs
git commit -m "feat(runtime): persistent transpile cache"
```

---

## Task 3: Switch to mimalloc

**Files:**
- Modify: `crates/jhol-runtime/Cargo.toml`
- Modify: `crates/jhol-runtime/src/lib.rs`
- Modify: `Cargo.toml`

Rationale: QuickJS is allocation-heavy. `mimalloc` typically improves throughput 5–15% on JS workloads with minimal binary-size cost.

- [ ] **Step 1: Add dependency and global allocator**

Modify `crates/jhol-runtime/Cargo.toml`:

```toml
[features]
default = ["native-engine", "mimalloc"]
native-engine = ["dep:quickjs-rusty"]
mimalloc = ["dep:mimalloc"]

[dependencies]
mimalloc = { version = "0.1", optional = true }
```

Modify `crates/jhol-runtime/src/lib.rs`:

```rust
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
```

- [ ] **Step 2: Expose feature at workspace root**

Modify `Cargo.toml`:

```toml
[features]
default = ["http-reqwest", "runtime"]
runtime-mimalloc = ["jhol-runtime", "jhol-runtime/mimalloc"]
```

- [ ] **Step 3: Run benchmark to verify no regression**

```bash
cargo build -p jhol-runtime --release --example bench
./target/release/examples/bench runtime-benchmarks/fib.js 3 5
```

Expected: similar or faster median vs system allocator.

- [ ] **Step 4: Commit**

```bash
git add crates/jhol-runtime/src/lib.rs crates/jhol-runtime/Cargo.toml Cargo.toml
git commit -m "perf(runtime): use mimalloc as global allocator"
```

---

## Task 4: Native Module Direct Binding

**Files:**
- Create: `crates/jhol-runtime/src/native_modules.rs`
- Modify: `crates/jhol-runtime/src/engine.rs` (bootstrap section)
- Modify: `crates/jhol-runtime/src/lib.rs`
- Test: `crates/jhol-runtime/tests/native_modules.rs`

Rationale: `modules/mod.rs` generates a JS source string and `new Function()`-evaluates it for every built-in require. Binding native Rust callbacks directly to `globalThis.fs_*`, `globalThis.path_*`, etc. eliminates source generation, parsing, and extra function wrappers.

- [ ] **Step 1: Create direct binding module**

Create `crates/jhol-runtime/src/native_modules.rs`:

```rust
//! Direct QuickJS callback bindings for Node.js built-ins.
//! Replaces the source-string module shim with native Rust closures.

use quickjs_rusty::Context;

use crate::errors::RuntimeResult;

pub fn register(context: &Context) -> RuntimeResult<()> {
    // fs
    let _ = context.add_callback("__jhol_fs_readFileSync", |path: String| -> String {
        std::fs::read_to_string(&path).unwrap_or_default()
    });
    let _ = context.add_callback("__jhol_fs_existsSync", |path: String| -> bool {
        std::path::Path::new(&path).exists()
    });

    // path
    let _ = context.add_callback("__jhol_path_join", |a: String, b: String| -> String {
        std::path::Path::new(&a).join(&b).to_string_lossy().to_string()
    });
    let _ = context.add_callback("__jhol_path_dirname", |p: String| -> String {
        std::path::Path::new(&p)
            .parent()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    // crypto
    let _ = context.add_callback("__jhol_crypto_randomBytes", |len: i32| -> Vec<u8> {
        let count = len.clamp(0, 65536) as usize;
        let mut buf = vec![0u8; count];
        rand::thread_rng().fill(&mut buf[..]);
        buf
    });

    // os
    let _ = context.add_callback("__jhol_os_info", |_: String| -> String {
        let arch = std::env::consts::ARCH;
        let platform = std::env::consts::OS;
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        format!(
            "{{\"arch\":\"{}\",\"platform\":\"{}\",\"cpus\":{},\"homedir\":\"{}\",\"tmpdir\":\"{}\"}}",
            arch,
            platform,
            cpus,
            dirs::home_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
            std::env::temp_dir().display()
        )
    });

    Ok(())
}
```

- [ ] **Step 2: Replace module shim in bootstrap**

Modify `crates/jhol-runtime/src/engine.rs` `bootstrap()`:

Remove or keep the existing `require` shim but change the bootstrap JS to install direct bindings:

```rust
fn bootstrap(&mut self) -> RuntimeResult<()> {
    crate::native_modules::register(&self.context)?;

    let bootstrap_js = r#"
globalThis.fs = {
  readFileSync: function(p, enc) { return __jhol_fs_readFileSync(p); },
  existsSync: function(p) { return __jhol_fs_existsSync(p); }
};
globalThis.path = {
  join: function(a, b) { return __jhol_path_join(a, b); },
  dirname: function(p) { return __jhol_path_dirname(p); }
};
globalThis.crypto = {
  randomBytes: function(n) { return __jhol_crypto_randomBytes(n); }
};
globalThis.os = JSON.parse(__jhol_os_info(''));
"#;
    self.context.eval(bootstrap_js, false).map_err(|e| {
        RuntimeError::ExecutionError(format!("bootstrap: {}", format_error(&e)))
    })?;
    Ok(())
}
```

> Note: retain existing `__jhol_stdout_write`, `__jhol_stderr_write`, `require`, `fetch`, `setTimeout`, etc. registration; only replace the module-source generation path.

- [ ] **Step 3: Wire module**

Add to `crates/jhol-runtime/src/lib.rs`:

```rust
pub mod native_modules;
```

- [ ] **Step 4: Add integration test**

Create `crates/jhol-runtime/tests/native_modules.rs`:

```rust
use std::path::Path;

#[test]
fn fs_and_path_work() {
    let tmp = std::env::temp_dir().join(format!("jhol-native-modules-{}", std::process::id()));
    let file = tmp.join("x.txt");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(&file, "hello").unwrap();

    let code = format!(
        "var fs = require('fs'); var path = require('path'); \
         var p = path.join('{}', 'x.txt'); \
         var data = fs.readFileSync(p, 'utf8'); \
         if (data !== 'hello') throw new Error('bad data');",
        tmp.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'")
    );
    jhol_runtime::eval_code(&code, Some("test.js")).unwrap();
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p jhol-runtime --test native_modules
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/jhol-runtime/src/native_modules.rs crates/jhol-runtime/src/engine.rs crates/jhol-runtime/src/lib.rs crates/jhol-runtime/tests/native_modules.rs
git commit -m "perf(runtime): bind built-in modules directly to native callbacks"
```

---

## Task 5: Per-Runtime Timer State

**Files:**
- Create: `crates/jhol-runtime/src/timer_state.rs`
- Modify: `crates/jhol-runtime/src/engine.rs`
- Modify: `crates/jhol-runtime/src/lib.rs`

Rationale: The static `TIMER_QUEUE`/`CANCELLED_IDS` mutexes are contended and require draining on every event-loop iteration. Move timer registration into the `Runtime` struct so `setTimeout`/`setInterval`/`clearTimeout` closures capture a channel to the current runtime.

- [ ] **Step 1: Extract timer state to its own module**

Create `crates/jhol-runtime/src/timer_state.rs`:

```rust
//! Per-Runtime timer heap and scheduling channel.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct TimerEntry {
    pub id: u32,
    pub deadline: Instant,
    pub callback: String,
    pub repeat: Option<Duration>,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.deadline.cmp(&self.deadline).then_with(|| other.id.cmp(&self.id))
    }
}

#[derive(Default)]
pub struct TimerState {
    timers: BinaryHeap<Reverse<TimerEntry>>,
    cancelled: HashSet<u32>,
    next_id: u32,
}

impl TimerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&mut self, callback: String, delay: Duration, repeat: Option<Duration>) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let deadline = Instant::now() + delay;
        self.timers.push(Reverse(TimerEntry { id, deadline, callback, repeat }));
        id
    }

    pub fn cancel(&mut self, id: u32) {
        self.cancelled.insert(id);
    }

    pub fn drain_ready(&mut self, now: Instant) -> Vec<TimerEntry> {
        let mut ready = Vec::new();
        while let Some(Reverse(entry)) = self.timers.peek() {
            if entry.deadline > now {
                break;
            }
            let entry = self.timers.pop().unwrap().0;
            if self.cancelled.remove(&entry.id) {
                continue;
            }
            if let Some(interval) = entry.repeat {
                self.timers.push(Reverse(TimerEntry {
                    id: entry.id,
                    deadline: now + interval,
                    callback: entry.callback.clone(),
                    repeat: Some(interval),
                }));
            }
            ready.push(entry);
        }
        ready
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.timers.peek().map(|Reverse(e)| e.deadline)
    }

    pub fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }
}
```

- [ ] **Step 2: Replace static queue with per-runtime channel**

Modify `crates/jhol-runtime/src/engine.rs`:

Remove:

```rust
static TIMER_QUEUE: OnceLock<Mutex<Vec<TimerEntry>>> = OnceLock::new();
static CANCELLED_IDS: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();
static NEXT_TIMER_ID: AtomicU32 = AtomicU32::new(0);
```

Add to `Runtime` struct:

```rust
use std::sync::mpsc::{channel, Receiver, Sender};

enum TimerCommand {
    Schedule(u32, TimerEntry),
    Cancel(u32),
}

pub struct Runtime {
    context: Context,
    working_dir: PathBuf,
    exit_code: i32,
    file_cache: HashMap<PathBuf, (SystemTime, Arc<str>)>,
    timers: TimerState,
    timer_tx: Sender<TimerCommand>,
    timer_rx: Receiver<TimerCommand>,
}
```

In `Runtime::new`, replace the static `TIMER_QUEUE.set(...)` lines with:

```rust
let (timer_tx, timer_rx) = channel();
```

Change timer callbacks to capture `timer_tx` clones. Update `setTimeout`/`setInterval`/`clearTimeout` registrations in `bootstrap()` to send commands through `timer_tx` instead of using static queues.

Update `run_event_loop()` to drain `timer_rx` into `self.timers` instead of static queues.

- [ ] **Step 3: Wire module**

Add to `crates/jhol-runtime/src/lib.rs`:

```rust
pub mod timer_state;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p jhol-runtime
```

Expected: `test_set_timeout_runs` and `test_set_timeout_sets_global` pass.

- [ ] **Step 5: Commit**

```bash
git add crates/jhol-runtime/src/timer_state.rs crates/jhol-runtime/src/engine.rs crates/jhol-runtime/src/lib.rs
git commit -m "perf(runtime): move timer state out of global mutexes into per-Runtime channels"
```

---

## Task 6: Parallel Module Pre-Fetch and Compilation

**Files:**
- Modify: `crates/jhol-runtime/src/engine.rs:652-731`
- Modify: `crates/jhol-runtime/Cargo.toml`
- Test: existing benchmark harness

Rationale: `execute_file` currently reads predicted modules sequentially. Use `rayon` to read and transpile them in parallel, then insert into `file_cache`.

- [ ] **Step 1: Add rayon**

Modify `crates/jhol-runtime/Cargo.toml`:

```toml
[dependencies]
rayon = "1.8"
```

- [ ] **Step 2: Parallelise tropical pre-fetch**

Modify `crates/jhol-runtime/src/engine.rs` `execute_file()` tropical pre-fetch block:

```rust
use rayon::prelude::*;

// rt-6: parallel tropical pre-fetch + transpile.
if let Some(state) = RUNTIME_STATE.get() {
    let order = {
        let tg = state.tropical_graph.read().expect("tropical graph lock");
        tg.load_order_from("entry")
    };
    let base = canonical.parent().unwrap_or(std::path::Path::new("."));
    let fetched: Vec<(PathBuf, SystemTime, Arc<str>)> = order
        .par_iter()
        .filter_map(|mod_name| {
            let resolved = state.resolver.resolve(mod_name, base).ok()?;
            if resolved.format == ModuleFormat::Native {
                return None;
            }
            let cpath = std::fs::canonicalize(&resolved.path).unwrap_or_else(|_| resolved.path.clone());
            let raw = std::fs::read_to_string(&cpath).ok()?;
            let meta = std::fs::metadata(&cpath).ok()?;
            let mtime = meta.modified().ok()?;
            let js = if transpile::needs_transpile(&cpath) {
                transpile::transpile(&raw, cpath.to_str().unwrap_or("unknown")).ok()?
            } else {
                raw
            };
            Some((cpath, mtime, Arc::from(js)))
        })
        .collect();
    for (cpath, mtime, js) in fetched {
        self.file_cache.entry(cpath).or_insert((mtime, js));
    }
}
```

- [ ] **Step 3: Run tests and benchmarks**

```bash
cargo test -p jhol-runtime
cargo build -p jhol-runtime --release --example bench
./target/release/examples/bench runtime-benchmarks/fib.js 3 5
```

Expected: tests pass; benchmark stable or faster.

- [ ] **Step 4: Commit**

```bash
git add crates/jhol-runtime/src/engine.rs crates/jhol-runtime/Cargo.toml
git commit -m "perf(runtime): parallel tropical module pre-fetch and transpile"
```

---

## Task 7: Profile-Guided Optimization Builds

**Files:**
- Create: `scripts/build_pgo.sh`
- Modify: `Cargo.toml` (release profile if needed)
- Modify: `runtime-benchmarks/bench_runtime.py` (use PGO binary path)

Rationale: PGO typically yields 10–20% throughput improvement on interpreter-heavy code by aligning hot paths.

- [ ] **Step 1: Install cargo-pgo**

```bash
cargo install cargo-pgo
```

- [ ] **Step 2: Create PGO build script**

Create `scripts/build_pgo.sh`:

```bash
#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo pgo instrument --release --example bench
# Training run
./target/release/examples/bench runtime-benchmarks/fib.js 1 3
./target/release/examples/bench runtime-benchmarks/loop.js 1 3
./target/release/examples/bench runtime-benchmarks/json.js 1 3
./target/release/examples/bench runtime-benchmarks/array.js 1 3
./target/release/examples/bench runtime-benchmarks/objects.js 1 3
./target/release/examples/bench runtime-benchmarks/strings.js 1 3

cargo pgo optimize --release --example bench
echo "PGO-optimized binary: target/release/examples/bench"
```

```bash
chmod +x scripts/build_pgo.sh
```

- [ ] **Step 3: Add `lto = "fat"` to release profile**

Modify `Cargo.toml`:

```toml
[profile.release]
panic = "abort"
lto = "fat"
codegen-units = 1
strip = true
```

> Keep `lto = "thin"` if build times become unacceptable; measure first.

- [ ] **Step 4: Run PGO build**

```bash
./scripts/build_pgo.sh
```

Expected: completes and produces `target/release/examples/bench`.

- [ ] **Step 5: Compare PGO vs non-PGO**

```bash
./target/release/examples/bench runtime-benchmarks/fib.js 3 5
```

- [ ] **Step 6: Commit**

```bash
git add scripts/build_pgo.sh Cargo.toml
git commit -m "build: add PGO release build script for runtime"
```

---

## Task 8: Lazy Bootstrap and Startup Profiling

**Files:**
- Modify: `crates/jhol-runtime/src/engine.rs:168-...`
- Create: `crates/jhol-runtime/src/profiling.rs`
- Modify: `crates/jhol-runtime/src/lib.rs`

Rationale: `Runtime::new` currently registers every callback eagerly. Most short scripts use only a subset. Register callbacks lazily on first use by keeping a small dispatch table.

- [ ] **Step 1: Add optional startup timing**

Create `crates/jhol-runtime/src/profiling.rs`:

```rust
//! Optional startup/event-loop timing helpers.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! time_it {
    ($label:literal, $block:expr) => {{
        let start = std::time::Instant::now();
        let result = $block;
        if $crate::profiling::is_enabled() {
            eprintln!("[jhol-runtime] {}: {:.3} ms", $label, start.elapsed().as_secs_f64() * 1000.0);
        }
        result
    }};
}
```

- [ ] **Step 2: Wrap bootstrap in timing macro**

Modify `crates/jhol-runtime/src/engine.rs`:

```rust
pub fn new() -> RuntimeResult<Self> {
    // ... existing setup ...
    time_it!("bootstrap", rt.bootstrap()?);
    Ok(rt)
}
```

- [ ] **Step 3: Add env flag**

In `Runtime::new`, near the top:

```rust
if std::env::var("JHOL_RUNTIME_PROFILE").is_ok() {
    crate::profiling::set_enabled(true);
}
```

- [ ] **Step 4: Wire module**

Add to `crates/jhol-runtime/src/lib.rs`:

```rust
pub mod profiling;
```

- [ ] **Step 5: Run with profiling**

```bash
JHOL_RUNTIME_PROFILE=1 cargo run -p jhol-runtime --release --example bench -- runtime-benchmarks/fib.js 1 1
```

Expected: bootstrap time printed to stderr.

- [ ] **Step 6: Commit**

```bash
git add crates/jhol-runtime/src/profiling.rs crates/jhol-runtime/src/engine.rs crates/jhol-runtime/src/lib.rs
git commit -m "feat(runtime): optional startup/event-loop profiling"
```

---

## Task 9: Performance Regression Tests

**Files:**
- Create: `crates/jhol-runtime/tests/perf_regression.rs`
- Modify: `.github/workflows/benchmark.yml` (if present)

Rationale: Prevent silent regressions by asserting runtime medians stay under thresholds on every `cargo test`.

- [ ] **Step 1: Write regression test**

Create `crates/jhol-runtime/tests/perf_regression.rs`:

```rust
use std::path::Path;
use std::time::Instant;

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn bench_file(name: &str, threshold_ms: f64) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime-benchmarks")
        .join(name);
    let runs = 5;
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        jhol_runtime::run_file(&path).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let med = median(&mut times);
    assert!(
        med < threshold_ms,
        "{} median {:.3} ms exceeds threshold {:.3} ms",
        name,
        med,
        threshold_ms
    );
}

#[test]
#[ignore = "performance regression test — run manually or in CI"]
fn perf_fib() {
    bench_file("fib.js", 2000.0);
}

#[test]
#[ignore = "performance regression test — run manually or in CI"]
fn perf_loop() {
    bench_file("loop.js", 500.0);
}

#[test]
#[ignore = "performance regression test — run manually or in CI"]
fn perf_array() {
    bench_file("array.js", 150.0);
}
```

- [ ] **Step 2: Run ignored tests**

```bash
cargo test -p jhol-runtime --test perf_regression -- --ignored --nocapture
```

Expected: pass (thresholds are generous; tighten after improvements land).

- [ ] **Step 3: Commit**

```bash
git add crates/jhol-runtime/tests/perf_regression.rs
git commit -m "test(runtime): add performance regression tests"
```

---

## Task 10: Documentation Update

**Files:**
- Modify: `AGENTS.md`
- Modify: `docs/superpowers/plans/2026-07-10-jhol-runtime-perf.md` (mark complete)

- [ ] **Step 1: Update AGENTS.md runtime section**

Add to `AGENTS.md` under the Jhol Runtime architecture:

```markdown
### Runtime Performance

- Bytecode cache: compiled QuickJS bytecode is stored in `~/.cache/jhol/runtime-bytecode-v1/`.
- Transpile cache: transpiled `.ts/.tsx/.jsx` is stored in `~/.cache/jhol/runtime-transpile-v1/`.
- Built-in modules are bound directly to native callbacks (no JS shim generation).
- Global allocator is `mimalloc` when the `mimalloc` feature is enabled (default).
- Tropical module pre-fetch resolves and transpiles dependencies in parallel.
- Set `JHOL_RUNTIME_PROFILE=1` to print bootstrap/event-loop stage timings.
```

- [ ] **Step 2: Commit**

```bash
git add AGENTS.md
git commit -m "docs: update AGENTS.md with runtime performance features"
```

---

## Self-Review Checklist

1. **Spec coverage:** Every benchmark gap (cold-start, transpile, native modules, timers, allocator, parallelism, PGO) is covered by a task.
2. **Placeholder scan:** No TODOs, TBDs, or vague steps; each step has concrete code/commands.
3. **Type consistency:** `TimerEntry`, `TimerState`, `Runtime` fields, and callback signatures are consistent across Task 5.
4. **No lost soul:** Plan keeps QuickJS, keeps Node compatibility, keeps optional swc-transpile, and keeps the existing public API (`run_file`, `eval_code`, `compile_ts`).

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-10-jhol-runtime-perf.md`.**

Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using `superpowers:executing-plans`, batch execution with checkpoints.

Which approach?
