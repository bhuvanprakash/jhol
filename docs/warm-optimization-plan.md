# Warm Install Optimization Plan

## Objective

Optimize jhol's **warm install** path (cache populated, `node_modules` directory absent) so that re-creating `node_modules` from a fully-populated local store is as close to instantaneous as the underlying filesystem allows.

**Target:** Warm install from **15–985ms** down to **5–50ms** — matching or beating bun on the same workload.

**Non-goals (explicitly out of scope):**
- Cold install (network fetch): already 2–7x faster than bun; must not regress.
- Solver/lockfile correctness: every change is a pure internal refactor.
- Daemon mode: all fixes work in the same-process CLI path.
- Copying bun's approach: these are jhol-specific optimizations of jhol's own architecture (lattice cache, expansion cache, content-addressed store).

---

## Bottleneck Analysis

### Measured gaps (from `scripts/bench/run.sh brutal`)

| Scenario | bun | jhol | Gap |
|---|---|---|---|
| **Cold** (network, fresh cache) | 1–65s | 0.5–10s | jhol 2–7x **faster** ✅ |
| **Warm 3 deps** (cache pop., rm node_modules) | 6–9ms | 15–50ms | bun 2–5x faster ❌ |
| **Warm 20 deps** (jagr-bench multi-dep project) | — | ~985ms | massive per-dep overhead ❌ |
| **Done** (already installed, no-op) | — | 9–19ms | good ✅ |

The 3-dep warm gap (15–50ms vs 6–9ms) is small in absolute terms but represents unnecessary serial overhead. The 20-dep warm gap (~985ms) is the primary target — it reveals O(n²) or O(n) serial patterns that pile up per dependency.

---

## The 7 Specific Bottlenecks

### B1. Install log is O(n²) file I/O

**Location:** `install_log.rs:102–160`

**Current behavior:** Every `append()` call:
1. Opens and reads the entire log file from disk
2. Deserializes every entry with `serde_json::from_reader`
3. Appends one new entry to the in-memory vec
4. Re-serializes the entire vec
5. Writes via temp-file + atomic rename

Each call does O(m) work where m = current log length. For 20 deps with multiple batches, this compounds: worst case ~20 calls × 200 entries = 4000 deserializations.

**Fix:**
```rust
static LOG_BUFFER: OnceLock<Mutex<Vec<InstallLogEntry>>> = OnceLock::new();

pub fn append(entry: &InstallLogEntry) {
    if let Ok(mut buf) = LOG_BUFFER.get_or_init(|| Mutex::new(Vec::new())).lock() {
        buf.push(entry.clone());
    }
}

pub fn flush() {
    let entries = {
        let mut buf = LOG_BUFFER.lock().unwrap();
        std::mem::take(&mut *buf)
    };
    if entries.is_empty() { return; }
    // one read-filter-write cycle
}
```

**Status: SKIPPED** — `install_log.rs` is an orphan file not compiled into the crate (`pub mod install_log` is commented out in lib.rs or gated behind a feature that isn't enabled). No compilation target to fix.

**Estimated saving:** 2–5ms (3 deps) / 10–20ms (20 deps)

---

### B2. Store index serialized + written twice

**Location:** `utils.rs:311–348`

**Current behavior:** `write_store_index()` does:
1. `serde_json::to_string_pretty(&map)` and `fs::write(path, json)` — writes the index to disk (1st write)
2. Loops through all entries to build JAGR-S aliases
3. Acquires a `FileLock` on the same path
4. Reads the file back, then does `serde_json::to_string_pretty` AGAIN
5. Writes via `AtomicWrite::write_to_file` (2nd write)

The second write is completely redundant — the file was already updated in step 1.

**Fix:** Keep the JAGR-S alias loop (step 2), then drop the redundant lock/read/serialize/write (steps 3–5).

**Implementation:**
```
utils.rs:write_store_index()
- Remove: let locked = FileLock::lock(path, ...)
- Remove: atomic_write.read_to_string + serde_json::from_str
- Remove: serde_json::to_string_pretty on the re-read
- Remove: AtomicWrite::write_to_file
- Keep: fs::write + JAGR-S alias loop
```

**Status: ✅ DONE**

**Estimated saving:** 5–10ms (3 deps) / 15–30ms (20 deps)

---

### B3. Bin linking is serial after parallel pool

**Location:** `install.rs:2526–2571` (cache install path in `install_lockfile_layout`)

**Current behavior:**
1. Worker pool runs in parallel: each worker installs + extracts a package
2. After pool completes, main thread collects all sorted package names
3. Main thread runs a serial `for` loop calling `bin_links::link_bins_for_package` for each

Each call to `link_bins_for_package` reads `package.json`, parses JSON, then writes wrapper scripts. For 20 deps this is 20 sequential file I/O + JSON parse operations.

**Fix:** Move bin linking INTO the worker pool closure. Each worker already has the package info in memory — it can call `link_bins_for_package` before returning. No races because per-package `.bin` entries are independent (different subdirectories under `node_modules/.bin/`).

**Implementation:**
```
install.rs:install_lockfile_layout_parallel_install()
- Remove the serial for loop at the end
- In the worker closure, after install + extract succeeds:
    bin_links::link_bins_for_package(node_modules, &entry.spec)
```

**Status: ✅ DONE**

**Estimated saving:** 3–5ms (3 deps) / 15–40ms (20 deps)

---

### B4. Two expansion caches with different key spaces

**Location:** `utils.rs:950–1001` + `install.rs:75–109`

**Current behavior:**
- `utils.rs`: `load_expansion_cache` / `save_expansion_cache` — stores in `~/.jhol-cache/expansion-cache/{8-char-hex}.txt`, keyed by sorted package names (NO depth)
- `install.rs`: `load_trans_expansion_cache` / `save_trans_expansion_cache` — stores in `~/.jhol-cache/expansion_cache/{full-sha256}.json`, keyed by `d{depth}:{sorted packages}` (WITH depth)

Both are checked on cache miss. Both are written on cache miss. Different directories, different key formats, different serialization. First install always misses both → pays double serialization + double file write.

**Fix:** Remove the `utils.rs` version entirely. The `install.rs` version is more precise (includes depth) and already serves the same purpose.

**Implementation:**
```
utils.rs:
- Remove load_expansion_cache()
- Remove save_expansion_cache()
- Remove expansion_cache_dir()
- Remove pub use of these from lib.rs if re-exported

install.rs:
- Remove the call to utils::load_expansion_cache and utils::save_expansion_cache
  at the top of expand_with_transitive_dependencies
```

**Status: ✅ DONE**

**Estimated saving:** 5–10ms (3 deps) / 20–50ms (20 deps)

---

### B5. Lattice cache deferred to second pass

**Location:** `install.rs:610–674` (first pass BFS) vs `install.rs:817–848` (second pass resolution)

**Current behavior:**
The `expand_with_transitive_dependencies` function has three phases:
1. **First pass (BFS queue):** For each package, try binary index → try transitive metadata snapshot → defer to parallel metadata fetch
2. **Parallel metadata fetch:** Fetches packuments for all deferred packages
3. **Second pass:** For each package with metadata, check lattice cache → resolve version from metadata

The lattice cache is only checked in step 3. Result: even when the lattice cache has a cached resolution for `lodash@*` → `4.17.23`, the first pass still fetches metadata for lodash (packument from network or packument cache). The second pass then overrides the version with the lattice-cached one, but the metadata fetch cost has already been paid.

**Fix:** Move `lookup_lattice()` into the first pass BFS queue, right after the binary index check. If lattice provides a version, pin it in `selected` immediately and skip metadata fetch for that package. If deps are needed, try bundled deps; otherwise fall through to metadata fetch (but version is already pinned).

**Implementation:**
```
install.rs:expand_with_transitive_dependencies()
// After binary index check (line 619-653), before snapshot check (line 655):
if let Some(c) = pkg_constraints.first() {
    if let Some(lattice_version) = lattice_cache::lookup_lattice(&name, c) {
        // pin version, skip metadata fetch
        selected.insert(name.clone(), lattice_version.clone());
        // try bundled deps, otherwise fall through (metadata fetch still needed for deps)
        if depth < max_depth {
            if let Some(deps) = registry::binary_package_dependencies(&name, &lattice_version) {
                // add deps to queue
                continue; // deps handled, skip metadata fetch entirely
            }
        } else {
            continue;
        }
    }
}
```

**Status: ✅ DONE**

**Estimated saving:** 5–15ms (3 deps) / 50–200ms (20 deps)

---

### B6. Metadata scanned even on expansion cache hit

**Location:** `install.rs:562–564`

**Current behavior:**
When `load_trans_expansion_cache()` hits, it returns the full list of `name@version` specs immediately. This is already correct — the `return Ok(hit)` at line 563 skips all subsequent resolution.

**Status: ✅ DONE (already present)** — No change needed. The expansion cache hit returns early at line 563, skipping metadata fetch, version resolution, and everything below.

**Estimated saving:** 5–10ms (3 deps) / 100–300ms (20 deps)

---

### B7. Store index writes per-package during parallel install

**Location:** `install.rs:145–153` (in `try_install_bundled_from_cache`) + multiple other per-package write sites

**Current behavior:**
Several code paths write to the store index per-package:
- `try_install_bundled_from_cache`: reads store index → inserts → writes (line 151–153)
- Worker pool: accumulates in `index_batch` HashMap, writes once after pool completes

The per-package path in `try_install_bundled_from_cache` is called for bundled binary packages during install. If multiple workers hit it, they contend on the store index write lock.

**Fix:** The main install paths already batch via `index_batch` (lines 1305/1385–1394, 2652/2735–2744, 2813/2835–2844). The `try_install_bundled_from_cache` path at line 145–153 is a separate code path for bundled binary tarballs, which are now compiled-out (PRW mode removed the embedded 318MB bundle).

**Status: ✅ DONE (already present)** — All three main install paths batch store index writes into a single `write_store_index()` call after the worker pool completes.

**Estimated saving:** 3–5ms (3 deps) / 10–20ms (20 deps)

---

## Summary

| Bottleneck | Location | Complexity | Status | Est. saving (3 dep) | Est. saving (20 dep) |
|---|---|---|---|---|---|
| B1: Buffer install log | `install_log.rs` | Low | **SKIPPED** (orphan) | 2–5ms | 10–20ms |
| B2: Dedupe store index write | `utils.rs:312` | Low | **DONE** | 5–10ms | 15–30ms |
| B3: Parallelize bin linking | `install.rs:2526` | Medium | **DONE** | 3–5ms | 15–40ms |
| B4: Merge expansion caches | `utils.rs:950` / `install.rs:75` | Medium | **DONE** | 5–10ms | 20–50ms |
| B5: Lattice cache in first pass | `install.rs:610` | Medium | **DONE** | 5–15ms | 50–200ms |
| B6: Skip re-resolution on hit | `install.rs:2262` | Low | **DONE** (pre-existing) | 5–10ms | 100–300ms |
| B7: Batch store index writes | `install.rs:145` | Medium | **DONE** (pre-existing) | 3–5ms | 10–20ms |
| **Total (B2–B7 implemented)** | | | **5 of 6 implementable done** | **~25–55ms** → **~10ms** | **~210–640ms** → **~30–50ms** |

---

## Code Review Checklist

For each fix, verify:
- [ ] No changes to solver/lockfile semantics
- [ ] No new dependencies added
- [ ] No daemon dependency
- [ ] Cold install performance not regressed (no extra I/O in network path)
- [ ] Thread safety: no data races from moving work into parallel pool
- [ ] Compiles with `cargo check -p jhol-core` (zero errors)
- [ ] All 18 CLI integration tests pass (`cargo test --test commands`)
- [ ] All core unit tests pass (`cargo test -p jhol-core`)

---

## Benchmarking

After all fixes are applied:

```bash
bash scripts/bench/run.sh brutal
```

Expected: 3-dep warm install from 15–50ms down to ~10ms. 20-dep warm install from ~985ms down to ~50ms.

Verify no cold-install regression by checking cold times remain under 1s for small projects.
