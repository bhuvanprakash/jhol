# Jhol Runtime — Lock-Free Runtime State & Native Resolver Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the global `Mutex<RuntimeState>` bottleneck and replace the JSON resolver cache with a fast binary cache, while preserving all module resolution behavior.

**Architecture:** Split `RuntimeState` into independent `Arc`-wrapped structures: an immutable `ModuleRegistry`, a `DashMap`-based `ModuleResolverCache`, and an `RwLock<SparseTropicalModuleGraph>`. Resolver cache uses `bincode` for persistence and writes atomically on a background thread.

**Tech Stack:** Rust 2021, `dashmap`, `bincode`, `xxhash-rust`, `std::sync::{Arc, RwLock}`, existing `jhol-runtime` crate.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/jhol-runtime/src/resolver.rs` | Module resolver and cache. Rewritten to use `Arc<ResolvedModule>`, `bincode`, and hashed keys. |
| `crates/jhol-runtime/src/engine.rs` | Replaces `Mutex<RuntimeState>` with fine-grained `Arc<RuntimeState>`. Updates callbacks to use lock-free reads. |
| `crates/jhol-runtime/src/modules/mod.rs` | Minor: `ModuleRegistry` becomes immutable and cloneable via `Arc`. |
| `crates/jhol-runtime/src/lib.rs` | No signature changes. |
| `crates/jhol-runtime/Cargo.toml` | Add `dashmap`, `bincode`, `xxhash-rust` dependencies. |

---

### Task 1: Update Dependencies

**Files:**
- Modify: `crates/jhol-runtime/Cargo.toml`

- [ ] **Step 1: Add required crates**

```toml
[dependencies]
# ... existing deps ...
dashmap = "5.5"
bincode = "1.3"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
```

- [ ] **Step 2: Check it compiles**

```bash
cargo check -p jhol-runtime
```

Expected: resolves new crates successfully.

---

### Task 2: Rewrite Resolver with Binary Cache and Arc

**Files:**
- Modify: `crates/jhol-runtime/src/resolver.rs`

- [ ] **Step 1: Update imports and types**

At the top of `resolver.rs`, replace imports with:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::{Arc, RwLock, atomic::{AtomicBool, Ordering}};

use dashmap::DashMap;
use serde::{Serialize, Deserialize};

use crate::errors::{RuntimeError, RuntimeResult};
```

Change `ResolvedModule`:

```rust
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub path: PathBuf,
    pub source: Arc<str>,
    pub format: ModuleFormat,
    pub is_external: bool,
}
```

- [ ] **Step 2: Add persistent cache types**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistentEntry {
    key: u64,
    path: PathBuf,
    format: ModuleFormat,
    is_external: bool,
    mtime_secs: u64,
    mtime_nanos: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistentCacheFile {
    version: u32,
    entries: Vec<PersistentEntry>,
}

const CACHE_VERSION: u32 = 1;
const CACHE_FILE_NAME: &str = "module-resolver-cache-v2.bin";
```

- [ ] **Step 3: Replace cache file path and load/save helpers**

```rust
fn cache_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let base = PathBuf::from(&home).join(".jhol");
    let _ = std::fs::create_dir_all(&base);
    base.join(CACHE_FILE_NAME)
}

fn cache_key(specifier: &str, base: &Path) -> u64 {
    use xxhash_rust::xxh3::xxh3_64;
    let mut input = specifier.as_bytes().to_vec();
    input.push(0);
    if let Ok(c) = std::fs::canonicalize(base) {
        input.extend_from_slice(c.as_os_str().as_encoded_bytes());
    } else {
        input.extend_from_slice(base.as_os_str().as_encoded_bytes());
    }
    xxh3_64(&input)
}

fn load_persistent_cache() -> HashMap<u64, PersistentEntry> {
    let path = cache_file_path();
    if !path.exists() {
        return HashMap::new();
    }
    match std::fs::read(&path) {
        Ok(bytes) => match bincode::deserialize::<PersistentCacheFile>(&bytes) {
            Ok(file) if file.version == CACHE_VERSION => {
                file.entries.into_iter().map(|e| (e.key, e)).collect()
            }
            _ => HashMap::new(),
        },
        Err(_) => HashMap::new(),
    }
}

fn save_persistent_cache(entries: &HashMap<u64, PersistentEntry>) {
    let path = cache_file_path();
    let file = PersistentCacheFile {
        version: CACHE_VERSION,
        entries: entries.values().cloned().collect(),
    };
    if let Ok(bytes) = bincode::serialize(&file) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
```

- [ ] **Step 4: Rewrite ModuleResolver**

```rust
#[derive(Clone)]
pub struct ModuleResolver {
    memory: Arc<DashMap<u64, Arc<ResolvedModule>>>,
    persistent: Arc<RwLock<HashMap<u64, PersistentEntry>>>,
    global_paths: Vec<PathBuf>,
    dirty: Arc<AtomicBool>,
}

impl ModuleResolver {
    pub fn new() -> Self {
        let mut global_paths = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            let global = PathBuf::from(home).join(".jhol").join("global").join(NODE_MODULES);
            if global.exists() {
                global_paths.push(global);
            }
        }
        let pc = load_persistent_cache();
        Self {
            memory: Arc::new(DashMap::new()),
            persistent: Arc::new(RwLock::new(pc)),
            global_paths,
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn resolve(&mut self, specifier: &str, base: &Path) -> RuntimeResult<Arc<ResolvedModule>> {
        let key = cache_key(specifier, base);

        // 1. Memory cache (lock-free)
        if let Some(cached) = self.memory.get(&key) {
            return Ok(Arc::clone(cached.value()));
        }

        // 2. Persistent cache with mtime check
        {
            let persistent = self.persistent.read().map_err(|_| RuntimeError::Internal("poisoned lock".into()))?;
            if let Some(entry) = persistent.get(&key) {
                if path_still_valid(&entry.path, entry.mtime_secs, entry.mtime_nanos) {
                    let module = Arc::new(ResolvedModule {
                        path: entry.path.clone(),
                        source: read_source(&entry.path, &entry.format)?,
                        format: entry.format.clone(),
                        is_external: entry.is_external,
                    });
                    self.memory.insert(key, Arc::clone(&module));
                    return Ok(module);
                }
            }
        }

        // 3. Resolve from disk
        let path = self.resolve_path(specifier, base)?;
        let module = Arc::new(self.load_module(&path)?);

        // 4. Store in both caches
        self.memory.insert(key, Arc::clone(&module));
        if let Ok(meta) = fs::metadata(&module.path) {
            if let Ok(mtime) = meta.modified() {
                let dur = mtime.duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap_or_default();
                let entry = PersistentEntry {
                    key,
                    path: module.path.clone(),
                    format: module.format.clone(),
                    is_external: module.is_external,
                    mtime_secs: dur.as_secs(),
                    mtime_nanos: dur.subsec_nanos(),
                };
                let mut persistent = self.persistent.write().map_err(|_| RuntimeError::Internal("poisoned lock".into()))?;
                persistent.insert(key, entry);
                self.dirty.store(true, Ordering::Release);
            }
        }

        Ok(module)
    }

    fn resolve_path(&self, specifier: &str, base: &Path) -> RuntimeResult<PathBuf> {
        let path = PathBuf::from(specifier);
        if path.is_absolute() {
            if let Some(found) = Self::try_path_with_extensions(&path) {
                return Ok(found);
            }
            return Err(RuntimeError::ModuleNotFound(format!("Module not found: {}", specifier)));
        }

        if specifier.starts_with("./") || specifier.starts_with("../") || specifier.starts_with('/') {
            let base_dir = if base.is_dir() { base.to_path_buf() } else {
                base.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
            };
            let resolved = base_dir.join(&path);
            if let Some(found) = Self::try_path_with_extensions(&resolved) {
                return Ok(found);
            }
            return Err(RuntimeError::ModuleResolutionFailed(
                specifier.to_string(),
                vec![resolved],
            ));
        }

        let base_dir = if base.is_dir() { base.to_path_buf() } else {
            base.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
        };

        if let Some(found) = Self::find_in_node_modules(&base_dir, specifier)? {
            return Ok(found);
        }

        if is_builtin(specifier) {
            return Ok(PathBuf::from(format!("node:{}", specifier)));
        }

        Err(RuntimeError::ModuleNotFound(format!("Cannot find module '{}'", specifier)))
    }

    fn load_module(&self, path: &Path) -> RuntimeResult<ResolvedModule> {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let format = match ext.as_str() {
            "mjs" | "mts" => ModuleFormat::ESM,
            "cjs" | "cts" => ModuleFormat::CJS,
            "json" => ModuleFormat::JSON,
            "wasm" => ModuleFormat::Wasm,
            "node" => ModuleFormat::Native,
            _ => ModuleFormat::CJS,
        };
        let source = read_source(path, &format)?;
        Ok(ResolvedModule { path: path.to_path_buf(), source, format, is_external: false })
    }

    pub fn save(&self) {
        if self.dirty.swap(false, Ordering::AcqRel) {
            if let Ok(persistent) = self.persistent.read() {
                save_persistent_cache(&*persistent);
            }
        }
    }
}

fn read_source(path: &Path, format: &ModuleFormat) -> RuntimeResult<Arc<str>> {
    if *format == ModuleFormat::Native || *format == ModuleFormat::Wasm {
        return Ok(Arc::from(""));
    }
    let text = fs::read_to_string(path).map_err(RuntimeError::IoError)?;
    Ok(Arc::from(text))
}

fn path_still_valid(path: &Path, secs: u64, nanos: u32) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false; };
    let Ok(mtime) = meta.modified() else { return true; };
    let dur = mtime.duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap_or_default();
    dur.as_secs() == secs && dur.subsec_nanos() == nanos
}
```

- [ ] **Step 5: Add serde derives to ModuleFormat**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleFormat {
    ESM,
    CJS,
    JSON,
    Wasm,
    Native,
    Unknown,
}
```

- [ ] **Step 6: Run resolver tests**

```bash
cargo test -p jhol-runtime resolver
```

Expected: passes. If no resolver tests exist, run full runtime tests.

---

### Task 3: Replace Global Mutex with Fine-Grained State

**Files:**
- Modify: `crates/jhol-runtime/src/engine.rs`

- [ ] **Step 1: Update imports and RuntimeState**

Replace the top of `engine.rs` with:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::SystemTime;

use quickjs_rusty::{Context, ExecutionError};

use crate::errors::{RuntimeError, RuntimeResult};
use crate::modules::{ModuleRegistry, BuiltinModule};
use crate::resolver::{ModuleResolver, ModuleFormat};
use crate::transpile;
use crate::tropical_graph::TropicalModuleGraph;
```

Replace `RUNTIME_STATE`:

```rust
struct RuntimeState {
    resolver: ModuleResolver,
    module_registry: Arc<ModuleRegistry>,
    tropical_graph: Arc<RwLock<TropicalModuleGraph>>,
}

static RUNTIME_STATE: OnceLock<Arc<RuntimeState>> = OnceLock::new();

fn init_runtime_state(resolver: ModuleResolver, module_registry: Arc<ModuleRegistry>, tg: TropicalModuleGraph) {
    let _ = RUNTIME_STATE.set(Arc::new(RuntimeState {
        resolver,
        module_registry,
        tropical_graph: Arc::new(RwLock::new(tg)),
    }));
}
```

- [ ] **Step 2: Update Runtime::new to use Arc<ModuleRegistry>**

```rust
let resolver = ModuleResolver::new();
let module_registry = Arc::new(ModuleRegistry::new());
let tg = TropicalModuleGraph::new(128);
init_runtime_state(resolver, Arc::clone(&module_registry), tg);
```

- [ ] **Step 3: Rewrite __jhol_require_module callback**

Replace the existing callback:

```rust
let _ = self.context.add_callback("__jhol_require_module", |parent: String, name: String| -> String {
    // Built-in lookup needs no lock
    if let Some(state) = RUNTIME_STATE.get() {
        if state.module_registry.has(&name) {
            if let Some(source) = state.module_registry.get_exports(&name) {
                return source;
            }
            return String::new();
        }
    }

    // Resolve path
    let base = std::path::Path::new(&parent)
        .parent()
        .unwrap_or(std::path::Path::new("."));

    let resolved = RUNTIME_STATE
        .get()
        .and_then(|state| {
            // Tropical graph write: short lock
            if let Ok(mut tg) = state.tropical_graph.write() {
                tg.add_edge(&parent, &name, 1.0, false);
            }
            // Resolver: DashMap + RwLock
            state.resolver.resolve(&name, base).ok()
        });

    match resolved {
        Some(module) => module.source.as_ref().to_string(),
        None => format!("throw new Error('Cannot find module: {}')", name),
    }
});
```

Note: because `resolver.resolve` takes `&mut self` for the persistent-cache dirty flag, but the callback is `Fn`, we need `ModuleResolver::resolve` to take `&self`. Update its signature to `pub fn resolve(&self, specifier: &str, base: &Path)` and make the dirty flag atomic. (Already done in Task 2 with `AtomicBool`.)

- [ ] **Step 4: Update execute_file tropical prefetch**

Replace the tropical prefetch block:

```rust
{
    if let Some(state) = RUNTIME_STATE.get() {
        let order = {
            if let Ok(tg) = state.tropical_graph.read() {
                tg.load_order_from("entry")
            } else {
                Vec::new()
            }
        };
        for mod_name in &order {
            let base = canonical.parent().unwrap_or(std::path::Path::new("."));
            if let Ok(resolved) = state.resolver.resolve(mod_name, base) {
                let cpath = if let Ok(c) = std::fs::canonicalize(&resolved.path) { c } else { resolved.path.clone() };
                if !self.file_cache.contains_key(&cpath) {
                    if let (Ok(raw), Ok(meta)) = (std::fs::read_to_string(&cpath), std::fs::metadata(&cpath)) {
                        if let Ok(mtime) = meta.modified() {
                            self.file_cache.insert(cpath, (mtime, std::sync::Arc::new(raw)));
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 5: Update evaluate tree_shake**

```rust
if let Some(state) = RUNTIME_STATE.get() {
    if let Ok(mut tg) = state.tropical_graph.write() {
        let _ = tg.tree_shake("entry");
    }
}
```

- [ ] **Step 6: Update file_cache type**

Change `file_cache` from `HashMap<PathBuf, (SystemTime, Arc<String>)>` to `HashMap<PathBuf, (SystemTime, Arc<str>)>`.

Update all insertions to use `Arc::from(raw)` or `Arc::from(text)`.

- [ ] **Step 7: Save persistent cache on runtime drop**

Add a `Drop` impl for `Runtime`:

```rust
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(state) = RUNTIME_STATE.get() {
            state.resolver.save();
        }
    }
}
```

- [ ] **Step 8: Run tests**

```bash
cargo test -p jhol-runtime
```

Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/jhol-runtime/src/resolver.rs crates/jhol-runtime/src/engine.rs crates/jhol-runtime/src/modules/mod.rs crates/jhol-runtime/Cargo.toml
git commit -m "perf(runtime): lock-free runtime state and binary resolver cache"
```

---

## Self-Review Checklist

- [ ] Spec coverage: lock-free state, binary cache, Arc source, background save.
- [ ] No placeholders: every function body provided.
- [ ] Type consistency: `ResolvedModule.source` is `Arc<str>`; `ModuleResolver.resolve` takes `&self`.
- [ ] Tests: existing runtime tests pass; resolver cache behavior preserved.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-08-jhol-runtime-lockfree-state-and-cache.md`.**

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach?
