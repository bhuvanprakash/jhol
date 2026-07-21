# Jhol Runtime Performance & Native Optimization Design

> Date: 2026-07-08  
> Scope: `crates/jhol-runtime`  
> Goal: Make `jhol run` / `jhol exec` / `jhol run --watch` dramatically faster and more native without losing functionality or Jhol's identity.

---

## 1. Goals & Non-Goals

### Goals

- Eliminate serialization points and algorithmic bottlenecks in the runtime.
- Replace stubbed or heavy dependencies with native, shared, or feature-gated implementations.
- Exploit OS-level primitives where safe (native file watching, fast randomness, connection reuse).
- Preserve the **tropical module graph** semantics — max-plus algebra, cycle detection, load ordering, and tree-shaking are part of Jhol's formal identity.
- Preserve every existing public API behavior:
  - `run_file`, `eval_code`, `compile_ts`, `is_supported_file`
  - `require`, built-in modules, `fetch`, timers, watch mode

### Non-Goals

- Do not change the JS engine (`quickjs-rusty`).
- Do not remove the tropical graph; only make it sparse and efficient.
- Do not break lockfile, resolver, or package-manager behavior in `jhol-core`.
- Do not add new user-facing commands; this is internal runtime optimization.

---

## 2. Bottlenecks Found

| # | Bottleneck | Location | Impact |
|---|---|---|---|
| 1 | Global `Mutex<RuntimeState>` serializes every `require()` | `engine.rs:RUNTIME_STATE` | All module loading inside QuickJS is single-threaded. |
| 2 | Dense tropical graph → `O(n³)` load order / tree-shake | `tropical_graph.rs` | 1,000 modules ≈ 1B float operations plus huge memory. |
| 3 | Resolver cache is JSON, keyed by long strings, saved on Drop | `resolver.rs` | Parse/write overhead; no cross-process sharing. |
| 4 | `setTimeout`/`setInterval` do not actually run | `engine.rs:__jhol_set_timeout` | The event loop is broken for async code. |
| 5 | Polling file watcher (200 ms) | `watch.rs` | High latency and CPU; misses rapid changes. |
| 6 | Regex-based TS stripper compiles regexes per call | `transpile.rs` | Redundant work and allocation. |
| 7 | `ureq` per `fetch()`, no connection pool | `engine.rs:__jhol_fetch` | Repeated TCP+TLS setup. |
| 8 | `/dev/urandom` opened per random call | `engine.rs` | Syscall overhead for UUID/bytes. |
| 9 | Stubbed `rusqlite`, `tiny_http`, `tungstenite` compiled in | `Cargo.toml` | Binary bloat and slower compile. |
| 10 | Module source returned as owned `String` per `require()` | `engine.rs:__jhol_require_module` | Copies large modules across the JS boundary. |

---

## 3. Architecture Changes

### 3.1 Lock-free Runtime State

#### Current

`RUNTIME_STATE` is a single `Mutex<RuntimeState>` holding:

- `resolver: ModuleResolver`
- `module_registry: ModuleRegistry`
- `tropical_graph: TropicalModuleGraph`

Every `require()` locks this mutex for the entire resolution + graph-update path.

#### New Design

Split `RuntimeState` into independent, contention-free structures:

```text
RUNTIME_STATE: OnceLock<Arc<RuntimeState>>

RuntimeState {
    registry: Arc<ModuleRegistry>,          // immutable after bootstrap
    resolver_cache: Arc<ModuleResolverCache>, // DashMap + RwLock for persistent batch
    tropical_graph: Arc<RwLock<SparseTropicalModuleGraph>>,
}
```

- `ModuleRegistry` is immutable after construction; reads need no lock.
- `ModuleResolverCache` uses `DashMap<String, Arc<ResolvedModule>>` for lock-free in-process hits.
- `TropicalModuleGraph` uses `RwLock`; reads are parallel, `add_edge` takes a short write lock.
- `__jhol_require_module` no longer holds one big mutex for the whole call.

**Preserves:** all behavior.  
**Gains:** module loading becomes parallel inside QuickJS.

---

### 3.2 Sparse Tropical Module Graph

#### Current

Dense adjacency matrix:

```rust
pub struct TropicalModuleGraph {
    names: Vec<String>,
    name_to_id: HashMap<String, ModuleId>,
    mat: Vec<Vec<TropicalEdge>>,
    capacity: usize,
}
```

`shortest_paths` runs Bellman-Ford over the matrix: `O(V³)`.  
`condense` runs Floyd-Warshall: `O(V³)`.  
Memory is `O(V²)`.

#### New Design

Keep the tropical algebra and public API exactly as-is:

- `TropicalEdge`
- `Add` / `Mul` semiring operations
- `add_edge`
- `shortest_paths`
- `spectral_radius`
- `load_order_from`
- `tree_shake`
- `condense`

Internally replace the matrix with sparse adjacency lists:

```rust
pub struct SparseTropicalModuleGraph {
    names: Vec<String>,
    name_to_id: HashMap<String, ModuleId>,
    outgoing: Vec<Vec<(ModuleId, TropicalEdge)>>,
    incoming: Vec<Vec<(ModuleId, TropicalEdge)>>,
}
```

Algorithm replacements:

| Operation | Old | New |
|---|---|---|
| `load_order_from` | Dense Bellman-Ford `O(V³)` | Sparse topological sort + tropical longest-path DP `O(V + E)` |
| `shortest_paths` | Dense Bellman-Ford `O(V³)` | Sparse Bellman-Ford `O(V·E)` |
| `spectral_radius` | Dense power iteration `O(V³)` | Sparse power iteration `O(V + E)` per step |
| `tree_shake` | Calls `shortest_paths` | Same, but using sparse reachability |
| `condense` | Floyd-Warshall `O(V³)` | SCC (Tarjan/Kosaraju) + DAG reachability `O(V + E)` |

Add a safety cap: `JHOL_TROPICAL_MAX_NODES` (default 16,384) to prevent runaway memory.

**Preserves:** tropical algebra, cycle detection, formal grounding.  
**Gains:** orders of magnitude faster for real module graphs.

---

### 3.3 Native Module Resolver Cache

#### Current

- In-memory `HashMap<String, ResolvedModule>`
- Persistent JSON file `~/.jhol/module_cache.bin` (despite the `.bin` extension)
- Cache is written synchronously in `Drop`
- Cache key is a long string: `"{}::{}", specifier, base.display()`

#### New Design

In-memory cache:

```rust
use dashmap::DashMap;

pub struct ModuleResolverCache {
    memory: DashMap<u64, Arc<ResolvedModule>>,
    persistent: RwLock<HashMap<u64, PersistentEntry>>,
    dirty: AtomicBool,
}
```

- Key is `xxh3(specifier, canonical_base_dir)` as `u64`.
- Values are `Arc<ResolvedModule>` to share source across callers.

Persistent cache:

- File: `~/.jhol/module-resolver-cache-v2.bin`
- Format: `bincode`
- Entry: `{ key: u64, path: PathBuf, format: ModuleFormat, is_external: bool, mtime: u64 }`
- Load once at resolver creation.
- Write using temp file + atomic rename on a background thread when `dirty` flips.
- On hit, verify `mtime` with a single `stat`.

**Preserves:** cache semantics and correctness.  
**Gains:** faster lookup, smaller file, no Drop blocking, safe concurrent access.

---

### 3.4 Working Async Event Loop

#### Current

- `__jhol_set_timeout` callback ignores its arguments.
- `setTimeout`/`setInterval` store closures but they never execute.
- `run_event_loop()` returns immediately with the exit code.

#### New Design

Implement a real timer/event loop inside `Runtime`:

```rust
pub struct TimerHeap {
    heap: BinaryHeap<Reverse<TimerEntry>>,
    next_id: u32,
}

struct TimerEntry {
    deadline: Instant,
    id: u32,
    closure: String, // JS function source captured by setTimeout
    repeating: Option<Duration>,
}
```

- `__jhol_set_timeout(fn_str, ms)` pushes a `TimerEntry`.
- `Runtime::run_event_loop()` drains the heap:
  - Sleep until the next deadline using `crossbeam-channel` or `std::thread::park_timeout`.
  - Evaluate the JS closure via `context.eval` on the main thread (QuickJS is not thread-safe).
- `setImmediate` inserts at the front of the ready queue.
- `process.nextTick` polyfill added.
- `clearTimeout` / `clearInterval` mark entries cancelled without removing from heap (checked at pop).

**Preserves:** timer API.  
**Gains:** async code actually runs; enables real `fetch` promises.

---

### 3.5 Native File Watcher

#### Current

- 200 ms polling loop over all tracked files.
- Clones the tracked map every poll.

#### New Design

Add `notify` as an optional dependency behind a `native-watcher` feature:

```toml
[features]
default = ["native-engine"]
native-engine = ["dep:quickjs-rusty"]
native-watcher = ["dep:notify"]
```

- On macOS: FSEvents via `notify`.
- On Linux: inotify via `notify`.
- On Windows: `ReadDirectoryChangesW` via `notify`.
- Keep the polling watcher as a fallback when the feature is disabled or `JHOL_WATCH_POLL=1`.
- `WatchSession` abstracts both backends behind the same API.

**Preserves:** watch mode contract.  
**Gains:** near-instant change detection and lower CPU.

---

### 3.6 Faster Transpile

#### Current

- `fast_strip_ts` compiles some regexes inside the function.
- Line-by-line allocation into a new `Vec<String>`.
- Always reprocesses even files with no TypeScript syntax.

#### New Design

- Pre-compile all regexes in `OnceLock`.
- Add a fast rejection scan: if the file has no `:` type annotations, `interface`, `type`, `enum`, or `<` JSX, return source unchanged.
- Strip type-only imports/exports and declarations in a single pass using a brace-depth scanner over `&str` slices, minimizing allocation.
- Keep the SWC backend behind `swc-transpile` for complex code.

**Preserves:** output for supported TS/JSX syntax.  
**Gains:** lower latency and less allocation.

---

### 3.7 Native / Shared HTTP, Randomness, and Crypto

#### Current

- `fetch()` creates a new `ureq` request per call.
- `crypto.randomBytes` / `randomUUID` open `/dev/urandom` every call.

#### New Design

- `fetch()`: reuse the shared `jhol-core::http_client` when the crate is available; otherwise keep one `ureq::Agent` inside `Runtime`.
- Randomness: use `rand::thread_rng()` (or a thread-local ChaCha8Rng) instead of `/dev/urandom`.
- `crypto.getRandomValues` copies directly into the JS TypedArray.

**Preserves:** API.  
**Gains:** connection reuse, fewer syscalls.

---

### 3.8 Dependency Diet

#### Current

These are always compiled:

- `rusqlite` (DB callbacks are stubs)
- `tiny_http` (only a toy `__jhol_http_listen` callback)
- `tungstenite` (only a toy `__jhol_ws_connect` callback)
- `ureq` (will be replaced or shared)

#### New Design

Gate each behind a feature:

```toml
[features]
default = ["native-engine"]
native-engine = ["dep:quickjs-rusty"]
http-server = ["dep:tiny_http"]
websocket = ["dep:tungstenite"]
sqlite = ["dep:rusqlite"]
native-watcher = ["dep:notify"]
```

Root `Cargo.toml` enables `runtime` but not `http-server`, `websocket`, or `sqlite` by default.

**Preserves:** all functionality when features are enabled.  
**Gains:** smaller default binary and faster compile.

---

### 3.9 String / Arc Optimization

#### Current

- `__jhol_require_module` returns an owned `String`.
- `ResolvedModule.source` is `String`.
- `file_cache` stores `Arc<String>`.

#### New Design

- `ResolvedModule.source` becomes `Arc<str>`.
- `file_cache` stores `Arc<str>`.
- Return `Arc<str>` to QuickJS if the binding supports zero-copy; otherwise clone once.

**Preserves:** behavior.  
**Gains:** less copying, shared memory for repeated requires.

---

## 4. Backwards Compatibility

- All public functions keep identical signatures.
- Built-in module APIs stay the same.
- Cache file path changes from `~/.jhol/module_cache.bin` to `~/.jhol/module-resolver-cache-v2.bin`; old file is ignored.
- New env vars are additive:
  - `JHOL_TROPICAL_MAX_NODES`
  - `JHOL_WATCH_POLL`
  - `JHOL_DISABLE_NATIVE_WATCHER`
- Server/WebSocket/SQLite features must be explicitly enabled in downstream builds.

---

## 5. Testing Strategy

- Unit tests for sparse tropical graph parity with dense matrix on small graphs.
- Resolver cache round-trip, stale-entry, and concurrent-access tests.
- Timer/event-loop tests for `setTimeout`, `setInterval`, `setImmediate`, `clearTimeout`.
- File watcher tests for both polling and native backends.
- Transpile parity tests: same output for TS/JSX samples before and after.
- `cargo test -p jhol-runtime --all-features` passes.
- Compile-size check: compare `cargo build --release` binary size before and after.

---

## 6. Expected Gains

| Area | Expected Improvement |
|---|---|
| Module load parallelism | ~N× on N-core for require-heavy startup |
| Tropical graph (1k modules) | ~100–1000× faster load-order/tree-shake |
| Resolver cache | ~5–10× faster cache hits |
| Watch mode latency | 200 ms → ~10 ms with native watcher |
| Default binary size | -5 to -15 MB depending on target |
| Cold startup | noticeable from smaller binary + fewer locks |

---

## 7. Implementation Order

1. Sparse tropical graph (biggest algorithmic win, preserves identity).
2. Lock-free runtime state (removes main serialization point).
3. Native resolver cache (improves every run).
4. Working event loop (enables async correctness).
5. Native file watcher (improves dev UX).
6. Transpile optimization (faster TS/JSX startup).
7. HTTP/randomness/crypto improvements (fewer syscalls).
8. Dependency diet and feature gating (binary size).
9. String/Arc optimization (memory/copy reduction).
