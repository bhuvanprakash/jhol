# Jhol: Be Fast and Unique — Performance + Identity

This document is about making Jhol **the fastest and most reliable** npm-compatible package manager on its own terms: by fixing real bottlenecks and leaning into what only Jhol does. The goal is to **beat the rest on speed and reliability** without being a clone of any one tool.

---

## Part A: How Jhol Is Already Unique (Don’t Copy; Double Down)

Jhol’s differentiators are **reliability, clarity, and formal grounding**. These are product identity, not “Bun features.”

| Jhol differentiator | Why it matters |
|--------------------|----------------|
| **No silent hangs** | Configurable timeouts (fetch, resolve, install); fail-fast with clear errors. Bun/npm often get blamed for “hangs at Resolving…” — Jhol doesn’t. |
| **Derivation trees for conflicts** | When resolution fails, you get a human-readable explanation (which requirement conflicted with which), not a wall of ERESOLVE. |
| **PeerSpin-style loop detection** | Peer dependency infinite loops are detected and reported instead of spinning. |
| **Formal semantics** | `JAGR2_SEMANTICS.md` and lockfile design are documented; resolution and lockfile behavior are specified. |
| **Proxy + Docker** | Explicit proxy support and timeouts; “works behind corporate proxy / in CI” is a stated goal. |
| **Narrow scope, stability-first** | Install, resolve, run, exec, lockfile, audit — no bundler/runtime scope creep. Fewer moving parts, fewer surprises. |
| **Rust + native** | No JS runtime required to install; single binary. |

**Positioning:** Jhol is the **predictable, formally specified, “it never hangs and errors are explainable”** package manager that also aims to be **fast**. Speed comes from correct store semantics and efficient I/O, not from mimicking one competitor.

---

## Part B: Why “Linking from Store” Isn’t Copying Bun

Linking from a content-addressable store into `node_modules` is **standard package-manager design**: npm (with `node_modules`), pnpm (symlinks from store), Yarn (PnP or classic), and Bun all use some form of “one copy in store, reference from project.” Jhol already has a store and `store_unpacked`; today the hot path **copies** from that into `node_modules` instead of referencing it. Fixing that is **correct store semantics**, not “Bun copycat.” The unique part is *how* Jhol gets there: with formal semantics, clear errors, and no silent hangs.

---

## Part C: Bottlenecks (Neutral, Jhol-Centric)

### Where time goes (from `JHOL_PROFILE_INSTALL=1` and benchmarks)

| Phase | Typical time | Main cause |
|-------|--------------|------------|
| **Resolution** | ~5–15 s | Packument/summary fetches; demand-driven discovery. |
| **Download + unpack + install** | ~13–43 s | Network + extract; then **copy** into `node_modules` instead of referencing store. |
| **Warm install (cache hit)** | 0.5–1.7 s | Still **copying** from store to project; no reference/link path for Tarball source. |

---

### Bottleneck 1: Copy instead of reference (warm + cold)

**Location:** `install.rs` → `install_cache_source()` for `CacheInstallSource::Tarball`.

**Current behavior:** Cache hit → unpack to `store_unpacked/<hash>/` (or reuse) → **copy** that directory into `node_modules`. So every install from cache does a full copy.

**Jhol’s fix (store semantics):** Use **reference/link** from store when the layout allows it (i.e. when `must_materialize` is false). We already have `link_or_copy_package_dir_to_path` and `must_materialize` for the Unpacked path; use the same rule for the Tarball path. If nested dependency placement breaks with linking, fix the placement logic so linking is safe instead of forcing copy for everyone.

**Differentiation:** We keep our layout and resolution semantics; we only fix “install from store” to use references where correct. No change to lockfile format, resolver, or error reporting.

---

### Bottleneck 2: Resolution (cold)

**Location:** `resolve_full_graph` (JAGR3 / PubGrub v2); `metadata_summary` and registry fetches.

**Current behavior:** Demand-driven metadata fetches; N round-trips for N packages. Batched prefetch helps but resolution is still the dominant cost.

**Jhol’s fix (efficient metadata):**
- Use **abbreviated packument** where the registry supports it (smaller payload, less parse).
- Use **persistent packument/summary cache** aggressively (we have disk cache; tune TTL and keying).
- **Parallel prefetch** for direct deps and larger batches for transitive (JAGR3 already has `prefetch_summaries`; tune batch size/concurrency).
- **Global resolution cache** keyed by root deps (e.g. hash of `package.json` deps) so “same deps, new project” can skip resolution. We have `global_resolution_cache`; ensure it’s keyed and usable across projects.

**Differentiation:** We don’t introduce a separate “global registry” or binary format; we make the existing resolver and caches as efficient as possible and document behavior in `JAGR2_SEMANTICS.md`.

---

### Bottleneck 3: Extract-then-copy (cold + warm)

**Location:** Same as Bottleneck 1; `ensure_unpacked_in_store` + copy.

**Jhol’s fix:** Same as Bottleneck 1 — **reference from store when safe**. Optionally align with `global_extracted_store`: first unpack registers there (by package@version when we have it), and install-from-cache = reference from that store. One unpack, many projects; no re-extract on warm path.

---

### Bottleneck 4: Store index

**Location:** `read_store_index` / `write_store_index`.

**Current behavior:** Full JSON read/write; we have in-memory cache (mtime+size). For very large stores, first read and every write can be heavy.

**Jhol’s fix:** Keep in-memory cache; avoid redundant writes when store didn’t change (e.g. link-only install). If needed, consider a more efficient format or mmap for read-heavy workloads. No need to match another tool’s format.

---

### Bottleneck 5: Extraction

**Location:** `selective_extract` (gzip); `ensure_unpacked_in_store`.

**Jhol’s fix:** **Parallelize** extraction across packages (we already parallelize download). Keep “unpack once, then reference” so warm path doesn’t re-extract. Optional: more efficient internal store format (e.g. zstd) if we want smaller/faster decompress; registry remains gzip.

---

## Part D: Ways to “Beat Bun” Without Imitating It

- **Speed:** Fix store semantics (reference instead of copy) and make resolution/metadata efficient (abbreviated packument, caches, batching). That’s generic good engineering.
- **Reliability:** We already win on timeouts, proxy, no silent hangs, derivation trees, PeerSpin detection. Keep highlighting these.
- **Correctness:** Formal semantics and lockfile guarantees (“same tree” from lockfile) are a different kind of “win” — fewer surprises in production.
- **Ecosystem:** Stay npm-compatible (lockfiles, registry, `node_modules` layout) so we’re a drop-in; our value is “same workflow, faster and more predictable.”

---

## Part E: Recommended order of work

1. **Store semantics:** Use reference/link for cache installs when `!must_materialize` (Tarball path). Fix nested placement if needed. Measure warm install.
2. **Correctness of `must_materialize`:** Ensure we only copy when a nested dependency truly needs a materialized directory.
3. **Unify with global_extracted_store:** First unpack registers there; install from cache = reference from that store. One unpack, many projects.
4. **Resolution:** Abbreviated packument, persistent cache, parallel prefetch, global resolution cache keyed by root deps.
5. **Store index:** Avoid redundant writes; consider efficient format for very large stores.
6. **Extraction:** Parallelize; optional internal store format (e.g. zstd) if useful.

After (1)–(3), warm install should be in the same league as the fastest tools; after (4), cold install improves without changing Jhol’s identity. We **beat them on speed by being correct and efficient**, and we **stay unique** by being the one that doesn’t hang, explains conflicts, and is formally specified.
