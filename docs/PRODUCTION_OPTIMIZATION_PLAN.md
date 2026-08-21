# Phase 3: Deep System Optimization Plan

Extends `docs/v2-optimization-plan.md` with 18 new bottlenecks found across
every subsystem — resolver, cache, storage, install pipeline, task queue,
registry, lockfile write, and experimental modules.

No quality loss. No copying other package managers. Every fix is a pure
refactor of jhol's own architecture.

---

## Recap: Phases 1–2 Already Deployed

| Fix | Area | Status |
|---|---|---|
| B1-B7 | Store index, bin linking, lattice, expansion cache | ✅ Done |
| L1-L8 | Lattice batch, stats lock, lifecycle, lockfile cache, store index reuse, BFS HashSet, packument cache, compact JSON | ✅ Done |
| L10 | packument_concurrency env var caching | ✅ Done |
| L12 | O(n²) merge in parallel_fetch_metadata | ⬜ Claimed done but STILL BROKEN in registry.rs:915–958 |

**Current bench:** 3-dep warm 9–14ms / 20-dep warm ~170ms / cold 2–10× faster
than bun.

---

## Phase 3: 20 New Bottlenecks (Ranked by Impact)

### Tier 0 — Correctness (bug, not perf)

#### C1: `offline_cache::get_cached_tarball()` returns WRONG tarball

**File:** `offline_cache.rs` — function scans `store/` directory and returns
the first `.tgz` file it finds, regardless of which package was requested.
This is a correctness bug: during offline install, it can silently return the
wrong package.

**Fix:** Use the store index (hash lookup) to find the correct tarball. If no
store index, fall back to name-matching the filename (not directory walk).

**Risk:** Data corruption — packages silently replaced with wrong content.
**Impact:** Critical correctness fix, not just optimization.

---

### Tier 1 — Hot path, every install

#### N1: Lockfile write does 3× full `to_string_pretty` + 76 `.clone()` calls

**File:** `lockfile_write.rs:1527,1558,1627`
**Current:** Writing `package-lock.json` serializes the entire lockfile
content 3 times with `serde_json::to_string_pretty` (30% slower than compact).
Plus 76 `.clone()` calls across the module for constructing `ResolvedPackage`
nodes — every field cloned from the source data even when unmodified.

**Fix:**
- Use `serde_json::to_string` for intermediate serialization, `to_string_pretty`
  only for the final user-facing write
- Replace `.clone()` chains with `Arc<str>` for version/resolved/integrity strings
- Batch writes: serialize once, write to all destinations from the same buffer

**Est. saving:** 3–8ms (3 dep) / 15–40ms (20 dep)

---

#### N2: GRM outer `RwLock` serializes all store/query + per-shard linear scan

**File:** `lock_free_grm.rs`
**Current:** Two-part bottleneck:
1. `parking_lot::RwLock<LockFreeGRM>` wraps the entire structure — every
   `store_global()` and `query_global()` acquires this write/read lock,
   serializing all concurrent access
2. Inside each `DashMap` shard (8 shards, keyed by first 2 chars), `store()`
   does linear scan of `Vec<ResolutionRecord>` to find duplicates, and
   `query()` does linear scan + max-by-key

**Fix:**
- Remove the outer RwLock — `DashMap` is already concurrent-safe
- Replace `Vec<ResolutionRecord>` per shard with `HashMap<String, ResolutionRecord>`
  for O(1) store/query

**Est. saving:** 1–3ms (3 dep) / 5–15ms (20 dep)

---

#### N3: Install BFS still has 72 `.clone()` + 129 string allocations per install

**File:** `install.rs` (BFS in `expand_with_transitive_dependencies`)
**Current:** Despite L6 (HashSet for deferred), the BFS queue still pushes
owned `String` tuples for every package. `constraints` map is `HashMap<String, String>`
cloned per iteration. `format!("{}@{}", name, version)` in hot loop.

**Fix:**
- Use `Arc<str>` for package names in queue entries
- Pre-allocate all `format!` buffers with `format_args!` + `write!` to reuse
- Cache parsed `(name, version)` pairs instead of raw specs

**Est. saving:** 2–5ms (3 dep) / 10–25ms (20 dep)

---

#### N4: `parallel_fetch_metadata` O(n²) merge via `.position()` — NOT fixed

**File:** `registry.rs:915–917, 956–958`
**Current:** Two sort comparators use `names.iter().position(|n| n == &a.0)`
inside each comparison. For n=20 packages, each sort does O(n²) linear scans.
For n=100+, this is visible.

**Fix:** Build `HashMap<&str, usize>` position index before sorting. Use
`pos_a.cmp(&pos_b)` instead of `position()`.

**Est. saving:** 1–3ms (20 dep)

---

#### N5: `extract_with_sheaf` buffers entire tarball in memory before any write

**File:** `selective_extract.rs`
**Current:** Reads all entries into `Vec<(String, Vec<u8>, bool)>` — the full
archive decompressed into memory — before writing any file. For a 10MB tarball,
this doubles memory: 10MB compressed + 10MB+ decompressed.

**Fix:** Stream extraction: process each entry as parsed. Write files
immediately, keep only metadata vector for sheaf filtering.

**Est. saving:** 5–15MB memory per extract, 1–3ms wall time

---

#### N6: `rebuild_bin_links` rescans full `node_modules` after install

**File:** `install.rs` + `bin_links.rs`
**Current:** Called as post-pass, walks every directory in `node_modules`
(including scoped subdirs), reads each `package.json`, parses JSON, then
generates wrapper scripts for any `.bin` entries.

**Fix:** Pass the list of installed packages (already known from install result).
Only read `package.json` for packages that have a `bin` field in their manifest.

**Est. saving:** 3–8ms (20 dep)

---

#### N7: Lattice cache `lookup_lattice` does O(k) scan + semver parse per entry per call

**File:** `lattice_cache.rs`
**Current:** For each package, scans all remembered entries and parses semver
on each to find best match. Semver `Version::parse()` allocates from string.

**Fix:**
- Store pre-parsed `semver::Version` in `LatticeEntry` (not just string)
- Sort entries by version on insertion, use binary search
- Cache best-match result per (name, constraint) pair

**Est. saving:** 1–3ms (3 dep) / 5–10ms (20 dep)

---

#### N8: HTTP client builds `format!("Bearer {}", token)` per request

**File:** `http_client.rs` + `registry.rs`
**Current:** Every authenticated registry request formats the Bearer header
string from scratch. Also builds ureq request chains with `.set()` per header.

**Fix:**
- Cache the formatted `format!("Bearer {}", token)` in an `OnceLock<String>`
- Cache the full ureq request builder (or at least the auth header)

**Est. saving:** <1ms per request, negligible but easy

---

### Tier 2 — Medium impact / cold path / experimental

#### N9: JAGR-S `probe()` is linear over entire capacity table

**File:** `jagrs.rs`
**Current:** The probe function scans the hash table linearly from start.
At high load factor, this degrades to O(capacity) per lookup.

**Fix:** Robin Hood hashing with backward-shift deletion. Or use `hashbrown`
(Hashbrown already uses Robin Hood internally — the custom implementation
should be replaced with hashbrown).

**Est. saving:** Not on default path, but measurable when JAGR-S is used.

---

#### N10: JAGR-S `read_alias_hash()` does file seek + read per lookup

**File:** `jagrs.rs`
**Current:** Alias lookups open/seek/read a file from the aliases directory.
Each separate `lookup_alias()` call pays a syscall.

**Fix:** Cache the alias file in memory (it's small — mmap it). Also batch
bulk lookups into a single read.

**Est. saving:** Cold path, but each lookup jumps from ~2µs to ~50ns.

---

#### N11: JAGR Layer 2 sheaf cohomology does O(n³) Gaussian elimination repeatedly

**File:** `jagr/tropical_sheaf.rs`
**Current:** `compute_sheaf_cohomology()` builds dense matrices and does
RREF elimination. `build_simplicial_complex()` does quadratic dedup with
`position()` + `contains()`.

**Fix:** Cache boundary matrix computations. Use sparse matrix representation
for the (typically sparse) simplicial complex. Reuse row operations across
cohomology dimensions.

**Est. saving:** Not on default path, but when `--jagr` is used, could cut
Layer 2 time from seconds to milliseconds.

---

#### N12: `global_cache::add()` rewrites full `index.json` on every package

**File:** `global_cache.rs`
**Current:** After each package add, the entire `index.json` is written with
`serde_json::to_string_pretty`.

**Fix:** Batch writes — accumulate N packages, write once. Or use append-only
log + periodic compaction.

**Est. saving:** Not hot (cold/cache-path), but each write is O(entries).

---

#### N13: `global_cache::get()` mmaps then immediately calls `.to_vec()`

**File:** `global_cache.rs`
**Current:** The mmap path maps the file then copies the entire content into
an owned `Vec<u8>`, losing all zero-copy benefit.

**Fix:** Return `Bytes` (from bytes crate) or `Arc<Mmap>` that shares the
underlying mmap. Only copy when caller needs to mutate.

**Est. saving:** Eliminates one full copy per cache hit. 1–5MB saved per install.

---

#### N14: `task_queue::try_steal()` allocates + shuffles `Vec<usize>` per idle check

**File:** `task_queue/queue.rs`
**Current:** When a worker has no work, `try_steal` builds a new `Vec<usize>`
of all worker indices, shuffles it, and iterates. Workers spin with
`thread::yield_now()`.

**Fix:** Pre-allocate steal order at pool creation. Use `compare_exchange` for
steal attempts without allocation.

**Est. saving:** Near-zero for small pools, but idle workers burn CPU.

---

#### N15: `adaptive` task queue records latency with `SystemTime` + sorts all samples per adjust

**File:** `task_queue/adaptive.rs`
**Current:** `record_latency` takes write lock, pushes sample, prunes. `try_adjust`
reads all samples, sorts for percentile calculation. Both use `SystemTime::now()`
per sample.

**Fix:** Use running P² quantile estimation (no storage of all samples). Use
`Instant` instead of `SystemTime` (faster on Linux).

**Est. saving:** Negligible unless adaptive scheduling is enabled.

---

#### N16: `resolution_mirror stats()` and `prune()` scan all entries

**File:** `resolution_mirror.rs`
**Current:** `stats()` iterates all sections/entries to compute totals. `prune()`
scans + removes expired entries.

**Fix:** Maintain running counters for stats (atomic increments/decrements).
Use segment-based eviction (delete oldest segment) instead of per-entry scan.

**Est. saving:** Not hot, but O(entries) per call.

---

#### N17: 6 independent in-memory caches duplicate data with different serialization

**Files:** `grm/`, `lock_free_grm.rs`, `lattice_cache.rs`, `resolution_mirror.rs`,
`global_cache.rs`, `global_cache_2.rs`, `offline_cache.rs`
**Current:** Each caches the same class of data (package→version resolution
results) with its own format, eviction, and persistence. GRM stores
`ResolutionRecord`. Lattice caches `(constraint, version, hits)` tuples.
Resolution mirror caches packed stats.

**Fix:** Unify into a single tiered cache with a common `CacheEntry` type:
- L1: `HashMap<String, Arc<CacheEntry>>` (fast, no serialization)
- L2: `HashMap<String, Vec<u8>>` mmap-backed (compact binary)
- L3: JSON on disk (compatibility)
All write-through to L1, background sync to L2/L3.

**Est. saving:** Reduces total memory by deduplication. Simplifies invalidation.

---

#### N18: `cas::store()` calls `sync_all()` on every tarball write

**File:** `cas/cas.rs`
**Current:** Every `store()` writes the file then calls `file.sync_all()`, which
forces a disk flush. On macOS with APFS, `sync_all` takes 1–10ms.

**Fix:** Remove `sync_all()` from the hot path. Only sync once at install
completion via explicit `sync_store()` call.

**Est. saving:** 1–10ms per package write × 20 packages = 20–200ms saved on
cold install. Not a warm-path issue.

---

### Tier 3 — Quick hygiene wins

#### Q1: `jagrs::daemon_stats()` scans entire manifest

Fix with running counter.

#### Q2: `mayer_vietoris::build_cover()` repeatedly computes overlaps

Use spatial index or bounding box pre-filter.

#### Q3: `tropical_max_atom::floyd_warshall()` classic O(n³)

Skip if graph is small (<50 nodes). Use Johnson's algorithm for sparse graphs.

#### Q4: `knot::graph_capsule_fp()` hashes entire graph

Cache the fingerprint instead of recomputing every call.

#### Q5: `offline_cache::cached_packages()` double-scans memory + disk

Use set semantics to avoid duplicate listing.

#### Q6: `global_cache_2::update_access()` linear search in `lru_queue`

Use `HashMap<Hash, DequePosition>` for O(1) access update.

---

## Estimated Combined Impact

| Tier | Items | 3 dep warm | 20 dep warm |
|------|-------|-----------|-------------|
| Current (Phases 1–2) | L1–L8, B1–B7 | 9–14ms | ~170ms |
| + N1: Lockfile write 3× serialize | `lockfile_write.rs` | 6–10ms | 130–155ms |
| + N2: GRM outer lock + linear scan | `lock_free_grm.rs` | 5–8ms | 120–140ms |
| + N3: BFS clone churn | `install.rs` | 4–7ms | 105–125ms |
| + N4: O(n²) merge fix | `registry.rs` | 4–6ms | 100–120ms |
| + N6: Skip bin link rescan | `install.rs` + `bin_links.rs` | 3–5ms | 90–110ms |
| + N7: Lattice lookup scan | `lattice_cache.rs` | 3–4ms | 85–100ms |
| + N5+N8+N18 | extraction/HW/sync | 3–4ms | 80–95ms |
| **Target** | | **~3–4ms** | **~80–95ms** |

C1 is a correctness fix (wrong tarball returned) — must fix regardless of perf
impact.

---

## Implementation Order

1. **C1: Fix offline_cache tarball lookup** — correctness bug fixes first
2. **N4: Fix O(n²) `.position()` merge** — 5-line fix, listed as "done" but isn't
3. **N1: Dedupe lockfile writes** — highest impact on warm 20-dep
4. **N2: Fix GRM outer lock + shard linear scan** — high impact, unlocks concurrency
5. **N6: Skip bin link rescan** — pass known package list
6. **N7: Lattice cache semver pre-parse + binary search**
7. **N3: BFS Arc<str> + buffer reuse**
8. **N5: Streaming tarball extraction**
9. **N8 + N18: HTTP header cache + remove sync_all**
10. **N9–N17: Tier 2 items** — JAGR-S probe, alias cache, global_cache batch, etc.
11. **Q1–Q6: Hygiene items**
