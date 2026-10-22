# Bottlenecks (Beat Bun) + Production Readiness

This document lists **remaining performance bottlenecks** to beat Bun on cold installs, and **production-readiness issues** (security, stability, observability) that should be addressed before relying on Jhol in production.

---

# Part 1: Bottlenecks to Beat Bun

## Already fixed (reference)

- **Link instead of copy** for cache installs when `!must_materialize` (Tarball path in `install_cache_source`).
- **Register unpacked in global_extracted_store** after successful Tarball cache install so future installs use Unpacked (link) path.
- **Parallel extraction** for lockfile layout cache hits (`run_worker_pool` over `cache_hits`).
- **JAGR3**: Merged specs per package for prefetch, configurable batch size, graph cache cap.

## Remaining: Cold install (resolution + first fetch)

| Bottleneck | Where | Fix |
|------------|--------|-----|
| **Resolution time** | `resolve_full_graph` → JAGR3 / PubGrub; packument/summary fetches. | Abbreviated packument already used in `fetch_metadata`. Strengthen **persistent packument/summary cache** (disk) and **global resolution cache** keyed by root deps so “same deps, new project” skips resolution. |
| **First-time download** | One packument/metadata per package; then sequential or batched download. | Keep parallel download; ensure **parallel unpack** after download (materialize path already uses worker pool). Prefetch direct deps in parallel at start (JAGR3 does this). |
| **Store index I/O** | `read_store_index` / `write_store_index` full JSON. | Already only write when `!index_batch.is_empty()`. For very large stores, consider **binary or streaming-friendly format** or **mmap** for read path. |

## Benchmark targets

- **Warm:** Jhol already wins in recent runs (link-from-store + global_extracted_store).
- **Cold small/medium:** Close the gap by reducing resolution round-trips (cache, global resolution cache) and keeping parallel download + unpack.

---

# Part 2: Production Readiness

## 2.1 Security

### Path traversal in tarball extraction (high)

- **Where:** `global_store.rs` → `extract_tarball_full`: uses `path.strip_prefix("package").unwrap_or(&path)` then `dest.join(stripped)`. If a malicious tarball contains an entry like `package/../../../etc/passwd`, `stripped` can be `"/../../../etc/passwd"` and `dest.join(stripped)` may write outside `dest`.
- **Where:** `global_extracted_store.rs` and any path using `strip_prefix("package")` + `dest.join(stripped)` without normalizing or rejecting `..`.
- **Fix:** Normalize the relative path and **reject any segment that is `..`**, or resolve and then check that the canonical target is under `dest`. Prefer a shared helper (e.g. `normalize_tarball_rel_path` in `selective_extract`) that returns `None` if the path contains `..` or otherwise escapes the package root.

### Path traversal in selective_extract (medium)

- **Where:** `selective_extract.rs` → `normalize_tarball_rel_path`: builds `rel` from path segments under `package/` but does **not** filter out `..`. So `package/foo/../bar` yields `foo/../bar`; `package_root.join(rel)` can escape.
- **Fix:** When building `rel`, skip or reject segments equal to `..`, or canonicalize and verify the result is under `package_root`.

### Sanitize package names when building paths (low)

- **Where:** `utils.rs` → `sanitize_package_name` exists but is **never used**. Paths derived from user/registry package names (e.g. cache dirs, store paths) could in theory contain `..` or other unsafe characters.
- **Fix:** Use `sanitize_package_name` (or equivalent) wherever package/version strings are used to form filesystem paths; reject or sanitize so that `..` and path separators cannot escape the intended root.

### Integrity verification (medium)

- **Where:** Lockfile and registry paths support SRI/integrity; ensure **every** install path (including fallback and cache restores) verifies integrity when available and **fails on mismatch**.
- **Fix:** Audit all code paths that write to the store or extract from tarballs and ensure they call the same integrity-checking helper when `expected_integrity` is present; no silent ignore of mismatch.

---

## 2.2 Stability and correctness

### Unwrap / expect / panic

- **Where:** Many `.unwrap()`, `.expect()`, and a few `panic!` remain across the crate (see grep counts). Under bad inputs or I/O errors these can crash the process.
- **Fix:** Replace with `?` and structured `Result`/`JholError` where possible; reserve `expect` only for “programmer bug” invariants with a clear message.

### Race conditions and concurrency

- **Where:** Cache and store index: multiple processes (e.g. parallel `jhol install` in different dirs) can read/write the same store index or cache files.
- **Fix:** Document “single-process or serialized install per store” if that’s the intended model; otherwise add file locking (e.g. `fs2` or `fd-lock`) or atomic write (temp + rename) and retry/backoff for store index and critical cache metadata.

### Lockfile and determinism

- **Where:** Documented in `SCOPE_AND_STABILITY.md`: same inputs and registry state should yield same resolution and lockfile.
- **Fix:** Add tests that assert deterministic resolution and lockfile output for a fixed set of deps and a fixed (or mocked) registry; avoid time- or order-dependent behavior in resolver and lockfile write.

---

## 2.3 Observability and operations

### Logging and errors

- **Where:** Ensure all failure paths (network, resolution, extract, link, store write) produce a **clear, actionable error** (e.g. derivation tree for resolution, package + URL for network, path for I/O).
- **Fix:** Review `JholError` variants and `eprintln!`/log sites; ensure no silent failures and that key operations are identifiable in logs.

### Timeouts and resource limits

- **Where:** Timeouts (fetch, resolve, install) and peer loop limit are configurable; document defaults and env vars in README and `SCOPE_AND_STABILITY.md`.
- **Fix:** Ensure no unbounded loops (e.g. resolution, prefetch) without a timeout or iteration cap; document how to tune for large monorepos or slow networks.

### Disk and cache growth

- **Where:** Store and global caches (e.g. global_extracted_store, packument cache) can grow without bound.
- **Fix:** Provide **cache prune** (and document it); consider size or age limits for packument and extracted-store caches so production systems can bound disk usage.

---

## 2.4 Documentation and contract

### Public API and lockfile format

- **Where:** Formal semantics in `JAGR2_SEMANTICS.md` and lockfile design in `LOCKFILE_DESIGN.md`.
- **Fix:** Keep these in sync with implementation; add a short “Production checklist” (e.g. integrity on, path safety, timeout tuning, cache prune) to README or a dedicated OPERATIONS.md.

### Backward compatibility

- **Where:** Lockfile format and store layout may evolve.
- **Fix:** Document versioning and migration (e.g. “lockfile v2” or “store layout v2”) and avoid breaking existing installs without a clear migration path or tool.

---

# Part 3: Suggested priority order

1. **Path traversal (security):** Fix `extract_tarball_full` and `normalize_tarball_rel_path` (reject or normalize `..`); use `sanitize_package_name` where package/version form paths.
2. **Integrity:** Audit and enforce integrity on every install path when expected_integrity is present.
3. **Cold resolution:** Strengthen resolution cache and global resolution cache to close cold-install gap vs Bun.
4. **Unwrap/panic reduction:** Replace unwraps in hot and error paths with proper `Result`/`JholError`.
5. **Store index and cache growth:** Document and implement prune; consider format/read optimization for very large stores.
6. **Observability and docs:** Production checklist, timeout/cache docs, and deterministic resolution tests.

---

# References

- `docs/engineering/BOTTLENECKS_JHOL_VS_BUN.md` — Performance and identity (link vs copy, resolution, store).
- `docs/user/SCOPE_AND_STABILITY.md` — In-scope features and stability priorities.
- `docs/engineering/JAGR2_SEMANTICS.md` — Resolution and lockfile semantics.
- `docs/engineering/LOCKFILE_DESIGN.md` — Lockfile format and guarantees.
