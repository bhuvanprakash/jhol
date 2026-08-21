# Phase 2: Deep Warm Install Optimization Plan

Extends `docs/warm-optimization-plan.md` with 13 additional bottlenecks found
across the full pipeline — registry, lockfile, lifecycle, cache, and resolver.

---

## Recap: Phase 1 (already deployed)

| Fix | File | Status |
|---|---|---|
| B2: Dedupe store index write | `utils.rs:312-348` | ✅ Done |
| B3: Parallelize bin linking | `install.rs:2526-2571` | ✅ Done |
| B4: Merge expansion caches | `utils.rs + install.rs` | ✅ Done |
| B5: Lattice cache in first pass | `install.rs:610-674` | ✅ Done |
| B6: Skip re-resolution on cache hit | `install.rs:562` | ✅ Pre-existing |
| B7: Batch store index writes | `install.rs` | ✅ Pre-existing |

## Phase 2 (all deployed)

| Fix | File | Status |
|---|---|---|
| L1: Batch lattice disk writes | `lattice_cache.rs:80-97` | ✅ Done |
| L2: Stats lock fix + dead code | `resolution_mirror.rs:72-118` | ✅ Done |
| L3: Skip lifecycle I/O | `lifecycle.rs:19-22` | ✅ Done |
| L4: Cache lockfile content | `lockfile.rs` (shared mtime cache) | ✅ Done |
| L5: Read-only store index access | `utils.rs` + `capsule.rs` | ✅ Done |
| L6: BFS HashSet for deferred | `install.rs:579,704,754,770` | ✅ Done |
| L7: Single-file packument cache | `registry.rs:90-123` | ✅ Done |
| L8: Compact cache JSON | `utils.rs:258,316,753` | ✅ Done |
| L9: Reverse index for tarballs | `install.rs:1089-1208` | ⬜ Skipped (cold path) |
| L10: Cache concurrency env vars | `install.rs:14-34` | ✅ Mostly done (cached_env_var) |
| L11: Avoid re-parsing specs | `install.rs:384-414` | ⬜ Skipped (ns-scale) |
| L12: O(n²) merge fix | `registry.rs:808-811` | ✅ Done |
| L13: Skip node_modules walk | `install.rs:1508-1517` | ⬜ Skipped (6-call-site refactor) |

**Bench result after Phase 1:** 3-dep warm 12–19ms (was 15–50ms) / 20-dep warm
**160ms** (was ~985ms) — 6× improvement on 20-dep.

**Bench result after Phase 2:** 3-dep warm **9-14ms** (was 12-19ms) / 20-dep warm
**~170ms** (was ~160ms, within noise). Cold: jhol still 2-10× faster than bun.

---

## Phase 2: 13 Remaining Bottlenecks

### Tier 1 — Hot path, every install

#### L1: Batch lattice cache disk writes

**File:** `lattice_cache.rs:80-97`
**Current:** `remember()` calls `save_disk()` which:
1. Clones every entry into a temporary `HashMap<String, Vec<(String,String)>>`
2. Full `serde_json::to_string(&out)` over all entries
3. `fs::write(path, s)` — full file rewrite

This happens for EVERY package after resolution. 20 deps = 20 full serializations
of the entire lattice store.

**Fix:** Buffer remembers in a `Mutex<Vec<(String,String,String)>>`. Flush to disk
once at process exit (or after install completes). The seed data loaded at startup
via `load_seed_entries` doesn't need re-writing each time — only new remembers.

**Est. saving:** 2–5ms (3 dep) / 10–30ms (20 dep)

---

#### L2: Resolution mirror stats lock churn + dead code

**File:** `resolution_mirror.rs:72-118`
**Current:**
- `get_section()` line 83: `self.stats.write().hits = ...` on a read path
- `lookup()` line 96: same `stats.write()` on read
- `lookup()` line 103-105: dead hash computation (`DefaultHasher` with no input)

**Fix:** Use `stats.read()` for incrementing hits on read paths. Remove the dead
`DefaultHasher` block in `lookup()`.

**Est. saving:** 1ms (3 dep) / 3–5ms (20 dep)

---

#### L3: Skip lifecycle I/O when scripts disabled

**File:** `lifecycle.rs:19-48`
**Current:** `run_install_scripts()` reads `package.json` + `serde_json::from_str`
for every installed package, even when `allowed.is_empty()` (scripts disabled).

**Fix:** Return early `Ok(false)` at the top if `allowed.is_empty()`.

**Est. saving:** 3–8ms (3 dep) / 15–40ms (20 dep)

---

#### L4: Cache lockfile parse across call sites

**File:** `lockfile.rs:273-503`
**Current:** Four functions read + parse the lockfile independently:
- `read_resolved_from_dir()` — full read + JSON parse
- `lockfile_integrity_complete()` — full read + JSON parse
- `read_all_resolved_specs_from_dir()` — full read + JSON parse (has mtime cache,
  but first call still pays cost + other callers don't share it)
- `read_lockfile_install_entries_from_dir()` — full read + JSON parse

Each is called from `install.rs` in the same install flow. The same lockfile
gets parsed up to 4 times.

**Fix:** Share a process-level parsed lockfile cache keyed by mtime. All callers
check the shared cache first.

**Est. saving:** 3–8ms (3 dep) / 5–15ms (20 dep)

---

#### L5: Reuse store index across branches

**File:** `install.rs` + `registry.rs` — `read_store_index()` called in multiple
independent branches: `install_lockfile_layout`, `install_package` cache path,
`install_package` cold path, `try_install_bundled_from_cache`.

Each call reads + JSON-parses the full store index from disk. In a single
install, this can happen 3–5 times.

**Fix:** Load store index once at install start. Pass `&HashMap` to callees.
Only re-read if a concurrent process may have modified it (which is rare).

**Est. saving:** 2–5ms (3 dep) / 10–25ms (20 dep)

---

#### L6: BFS expansion allocation churn

**File:** `install.rs:542-779`
**Current:** `expand_with_transitive_dependencies` does:
- `queue` stores `(String, usize)` — clones names on push/pop
- `deferred_packages: Vec<String>` — O(n) `.contains()` checks per package
- `constraints` entry lookup scans with `.iter().any()` per dep edge
- `pkg_constraints` cloned per BFS iteration
- `format!()` in tight loops for log messages

**Fix:**
- `deferred_packages: HashSet<String>` (or `FxHashSet`) for O(1) dedup
- Build a `HashMap<String, Vec<String>>` reverse index for constraints
- Avoid `String` clones in queue using `name: Rc<str>` or indices
- Guard log formatting behind a `verbose` flag

**Est. saving:** 2–5ms (3 dep) / 10–30ms (20 dep)

---

#### L7: Merge packument cache into single file

**File:** `registry.rs:90-123`
**Current:** Packument cache stored as two files per package:
`{hash}.json` (body) + `{hash}.etag` (ETag header). Every lookup issues two
`fs::read` calls; every write issues two `fs::write` calls.

**Fix:** Embed ETag in the JSON body as a top-level field `{"etag": ..., "data": ...}`.
Single file read + single file write.

**Est. saving:** 3–8ms (3 dep) / 5–15ms (20 dep)

---

#### L8: Compact cache JSON serialization

**File:** `registry.rs` (packument cache writes)
**Current:** Uses `serde_json::to_string_pretty` for cache files. Pretty-printing
is ~30% slower than compact serialization and produces larger output.

**Fix:** Use `serde_json::to_string` for internal cache files. Keep pretty
only for user-facing files (`package-lock.json`).

**Est. saving:** 1–3ms (3 dep) / 3–8ms (20 dep)

---

### Tier 2 — Medium impact / code hygiene

#### L9: Reverse index for tarball lookups

**File:** `install.rs:1089-1208` (`get_cached_tarball_fast`)
**Current:** On miss, scans all store index entries linearly with string
comparisons to find matching packages. O(n) per miss.

**Fix:** Build a reverse index (`HashMap<String, String>` mapping `pkg` → `hash`)
once and reuse.

**Est. saving:** 1–3ms (3 dep) / 5–15ms (20 dep)

---

#### L10: Cache concurrency values in OnceLock

**File:** `install.rs:14-34`
**Current:** `download_concurrency()` and `cache_install_concurrency()` call
`env::var()`, `parse::<usize>`, and clamping on every invocation. Called
multiple times per install.

**Fix:** Cache in `static` `OnceLock<usize>`.

**Est. saving:** <1ms per call

---

#### L11: Avoid re-parsing package specs

**File:** `install.rs:384-414` (`base_name`, `exact_version_from_spec`)
**Current:** `rfind('@')` + string slicing repeated for the same package across
multiple functions.

**Fix:** Pass `(name, version)` tuples through internal APIs; parse once at
the entry point.

**Est. saving:** 1–2ms (20 dep)

---

#### L12: O(n²) merge in parallel_fetch_metadata

**File:** `registry.rs:789-805`
**Current:** Collects ordered results then uses `position()` inside the loop
to match results back to requests. `position()` is O(n) scan per item.

**Fix:** Index by `HashMap<String, result>`.

**Est. saving:** 1–3ms (20 dep)

---

#### L13: Skip node_modules walk in remember_installed_packages

**File:** `install.rs:1508-1517`
**Current:** After install, `remember_installed_packages` walks every directory
in `node_modules` to build the installed-packages index. But we already know
which packages were just installed.

**Fix:** Pass the known package list instead of walking the filesystem.

**Est. saving:** 2–5ms (20 dep)

---

### Tier 3 — JAGR-specific / experimental (low priority)

| Issue | File | Complexity | Fix |
|---|---|---|---|
| PGM index O(n²) training | `jagrs.rs:810-861` | High | Segment training with rolling window |
| FrontCodedTable O(n²) per access | `jagrs.rs:1093-1118` | High | Cache cumulative offsets |
| Elias-Fano linear high-bit scan | `jagrs.rs:1205-1230` | Medium | popcount rank/select |
| EmbeddingIndex O(n log n) per insert | `jagrs.rs:1263-1277` | Medium | Batch inserts |
| Mmap probe degrades with load | `jagrs.rs:333-378` | Medium | Robin Hood hashing |

Not on the default install path — skip unless user explicitly enables them.

---

## Estimated Combined Impact (Phase 1 + Phase 2)

| Fix | 3 dep warm | 20 dep warm |
|---|---|---|
| Phase 1 (B2-B7, deployed) | 12-19ms | 160ms |
| L1: Batch lattice remember | 10-14ms | 130-150ms |
| +L2: Stats lock fix | 9-13ms | 127-145ms |
| +L3: Skip lifecycle I/O | 6-10ms | 112-130ms |
| +L4: Cache lockfile parse | 5-8ms | 105-120ms |
| +L5: Reuse store index | 4-7ms | 95-110ms |
| +L6: BFS allocation churn | 3-6ms | 85-100ms |
| +L7: Single-file packument cache | 3-5ms | 80-95ms |
| +L8: Compact cache JSON | 2-5ms | 77-90ms |
| +L9-L13 (Tier 2) | 2-4ms | 65-80ms |
| **Actual after Phase 2** | **~9-14ms** | **~160-176ms** |

No quality loss — pure refactors, no solver/lockfile semantic changes,
no new dependencies, no copying bun/npm.

---

## Implementation Order

1. **L3: Skip lifecycle I/O** — 3-line change, high impact, zero risk
2. **L2: Stats lock fix + dead code** — 4-line change, trivial
3. **L8: Compact cache JSON** — 1-line change per call site
4. **L7: Merge packument cache** — moderate change in `registry.rs`
5. **L1: Batch lattice remember** — moderate change in `lattice_cache.rs`
6. **L4: Cache lockfile parse** — moderate change in `lockfile.rs`
7. **L5: Reuse store index** — moderate change across `install.rs`/`registry.rs`
8. **L6: BFS allocation churn** — moderate change in `install.rs`
9. **L9-L13** — lower priority, tackle after Tier 1
