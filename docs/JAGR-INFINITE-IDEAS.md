# JAGR-∞ ideas, measured one by one

Method: a benchmark harness over the **real** packument cache
(`crates/jhol-core/examples/resolve_bench.rs`), which reads the same gzipped HTTP
bodies an install reads, plus `JAGR_PROFILE_CHART=1` phase timings and
`select_versions_stats`. Nothing here is claimed without a number.

Corpus: 275 packuments, 6,715 edges, 3,774 distinct `(target, spec)` pairs.

## Verdicts

| # | idea | verdict | measured |
|---|---|---|---|
| 1 | Binary version-space index (drop gunzip + JSON) | **landed: 28x on the cache view** | see below |
| 2 | Capability-equivalence classes | **not yet justified** | chart build is mask-bound, not version-bound (see below) |
| 3 | Time/policy as extra bitmap axes | **promising, unbuilt** | one extra `intersect` per axis |
| 4 | Exactly-computed LCV ordering | **blocked** | search never runs on real trees (see blocker) |
| 5 | Certificate lockfile / `why`-`why-not` | **promising, unbuilt** | derivation data already in the graph |

## Shipped and kept

### Mask memoisation per `(target, spec)`
6,715 edges → 3,774 mask computations (**1.78× fewer**), and the cache is the
place the *compiled* spec lives, so the two wins compound.

### Compile the range once; match N versions — **12.4× on chart assembly**

`version_satisfies_parsed` is written for one-off checks: it trims, probes for an
exact pin, and takes a mutex-guarded global cache lock **per call**. Selecting a
version means asking "does this spec match?" for *every published version of the
target*, so the lock and the hashing were the hot loop.

`registry::compile_spec` decides the variant once — `Any` (stable only), `Exact`
(a bare pin is equality, not caret), `Alternatives` (one `VersionReq`, or several
joined by `||`), `Unsupported` — and the inner loop becomes a pure comparison.

| phase | before | after |
|---|---:|---:|
| extract (versions + deps + peers) | 2.4 ms | 3.0 ms |
| **assemble (masks)** | **141.6 ms** | **11.4 ms** |
| chart total | 144.0 ms | 14.5 ms |
| propagate | 0.6 ms | 0.7 ms |

Semantics are unchanged **by construction** and pinned by
`compiled_spec_tests`: 25 spec shapes × 16 versions, including pins, wildcards,
prereleases, `||` alternatives, hyphen ranges and junk, comparing the compiled
answer against the one-off predicate.

Also landed: published version strings are parsed **once per package** instead of
once per `(spec, version)` pair.

### Binary resolver index — **67× faster load, 5.3× end-to-end**

`crates/jhol-core/src/jagr_index.rs`. The registry cache stores gzipped HTTP
bodies because that is what HTTP gives us; it is not a format for repeated
resolution. The index holds only what the resolver reads — sorted version
strings, and each dependency/peer declaration with **the releases that declare
it** — as a flat length-prefixed little-endian blob
(`magic "JHOLJAG1"`, per package: name, versions, deps, peers).

Measured on 344 packuments / 34,899 edges / 2.27 MB of index:

| phase | JSON path | index path |
|---|---:|---:|
| load (gunzip + JSON parse + prune) | 302.7 ms | **4.5 ms** |
| chart build | 69.6 ms | 65.4 ms |
| **end to end** | **372 ms** | **70 ms** |

- The index write costs 157.7 ms **once** per packument (at cache-write time),
  amortised across every later resolve; the bench measures build+encode
  separately so it is not hidden inside the read number.
- Chart build is unchanged because it was already mask-bound; the win is the load.
- Determinism: packages encoded in sorted name order, edges in sorted
  `(target, spec, optional)` order, so the same inputs produce the same bytes — a
  cache key must be a function of its inputs.
- Robustness: every read is bounds-checked; truncated, foreign-magic and
  trailing-byte blobs return `None` so the caller falls back to the packument
  instead of panicking.

### The index is now a real cache layer, self-healing

The write side started as a hook in the fetch path — and **never fired**: a real
install wrote 63 packuments and 0 indexes, because the cache is written by more
than one layer. Rather than add a second hook, the loader materialises what it
parses: every caller that wants the resolver's view comes through
`load_cache_dir`, so the accelerator installs itself the first time anything
reads the cache.

| reading 344 packuments (2.27 MB of index) | time |
|---|---:|
| JSON path (gunzip + parse + prune) | 426.0 ms |
| cache view, first read (parses + writes indexes) | 57.4 ms |
| **cache view, indexes present** | **15.2 ms** |

Per-file validation is **stat-only** — length + mtime recorded in the header —
because validating by hashing the packument would spend what the index exists to
save. Stale, truncated, foreign-magic or trailing-byte blobs are rejected and the
entry falls back to JSON, so a bad index costs one parse and never a wrong
answer. Writes are atomic (temp + rename): a half-written index is never read.

Tests: round-trip equality, staleness detection, truncation at five offsets,
missing file, mixed cache (one entry from index, one from JSON, then both from
index), and stale-index-rebuilds-from-JSON.

**Honest status:** the live install path still parses JSON, because other
consumers need the full packument (lockfile writing, integrity, lifecycle). What
landed is the cache layer plus the measurement; the switch is to make the
resolver's packument gather call `load_cache_dir` instead of the per-name JSON
read.

**A defect this work exposed:** `extract_deps` returns a `HashSet`-derived `Vec`,
so a package's edge order depended on hashing — the solver is pure, but the chart
handed to it was not. Edges are now sorted in the builder, which makes "same
manifest, same tree" true for the chart as well as the solve. The equality test
(index-chart == JSON-chart, edge for edge) is what caught it.

Remaining step: wire the index into the registry cache (write `.idx` beside each
packument, read it when present and valid, fall back otherwise). The writer and
reader exist and are tested; the integration point is the packument cache.

## Why idea 2 (capability classes) is deferred

Classes were expected to be the big constant-factor win, on the theory that
patch-heavy packages inflate the search space. The measurement says otherwise:
extraction is 3 ms and **mask assembly was 98% of chart build**. Classes collapse
*versions per signature*, which saves candidate enumeration — a search cost — and
search is not where the time is. Revisit after the load cost drops, when chart
build is small enough for candidate enumeration to matter.

## The blocker that gates ideas 4 and much else

Propagation fails on every real root (`express`, `jest`, `yargs`, `@types/node`)
with a genuine multi-major conflict — `media-typer`, `type-fest`, `isexe`,
`p-tyy`, `jest-worker`. That is not a bug in the solver: **a flat solve that
requires one version per package cannot represent an npm tree**, where several
majors coexist via nesting. Historically the old bitmaps were compared
positionally across different ordinal bases, which silently *accepted*
incompatible versions; the nested layout then produced a correct-looking tree.
With correct intersections the conflict surfaces. So:

* search-dependent ideas (exact LCV, budget policy) cannot be measured until the
  nested layout resolves per dependent;
* landing the base-aware `intersect` fix requires that rework first.

## Next, in order

1. **Wire the index into the registry cache** — write `.idx` beside each cached
   packument, read it when valid (keyed by packument hash), fall back otherwise.
   Reader/writer/equality tests exist; acceptance is `resolve_bench` load < 20 ms
   on the real cache and byte-identical selections with `JAGR_NO_INDEX=1`.
2. **Nested layout per dependent** — unblocks the correct-intersection fix and
   everything downstream.
3. **Certificate lockfile** (`jhol prove`, offline, O(packages) popcounts) and
   **`--as-of` / policy axes** — both are one more `intersect` once the index
   exists.
