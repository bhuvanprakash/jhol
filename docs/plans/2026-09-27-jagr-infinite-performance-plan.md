# JAGR-Infinite: Evidence-Led Performance Plan

**Status:** active; resolver and install-path work continues, with changes gated on correctness and measured evidence.
**Scope:** improve cold dependency resolution and install time without changing npm-compatible selection, integrity, install layout, or failure behavior. No competitor implementation is to be copied. Competitors are measurement baselines only.
**Repository rule:** do not commit, push, or tag.

## Executive finding

JAGR-Infinite has meaningful resolver work to optimize, but accelerating it alone will not make ordinary cold installs 10x faster: the common install path normally uses transitive expansion plus `ManifestThenPackumentStrategy`; the JAGR-Infinite entry point is reached by resolver selection, lockfile-only/update, analysis, and conflict-repair routes. A release `5 cold-next` run after the Next.js loadability fix took 9.42–11.02 seconds (median 10.84 seconds) across three isolated runs. That is a post-change Jhol measurement, not a competitor comparison. Earlier profiling recorded high variance in metadata expansion and registry response time.

The immediate product issue found was not the resolver's arithmetic: symlinked `node_modules/next` broke Node's resolution of a Next.js package subpath. Default copy-on-write materialization fixed this locally; the release build, full workspace/all-features suite (583 passed, 10 ignored), cloner tests (3 passed), offline fixture install, and `require('next')` succeeded. The cold benchmark validates its installed runtime tree. No speedup is claimed for this materialization change.

## What the code currently does

The JAGR-Infinite route has lockfile and resolved-bundle shortcuts, fetches a transitive packument tree, builds an atlas, maps direct ranges to ordinal intervals, builds/refines a full-tree chart, optionally follows lockfile homotopy, otherwise compiles/runs the RVM, and finalizes the assignment. Important boundaries:

- Atlas construction and full-chart construction both derive version/dependency data from packuments. This is the clearest candidate for a one-pass shared representation, but must be confirmed with allocation/CPU profiles before refactoring.
- The current route rejects a direct range if published matching versions are not one contiguous ordinal interval. That is a deliberate soundness guard: do not widen it or silently admit versions in gaps. Exact bitmap support or safe fallback is needed for non-convex ranges.
- RG/Ricci data is diagnostic-only when debug logging is enabled; it does not guide the production RVM selection path. Sheaf metrics are recorded on the chart but should not be described as a correctness proof unless their semantics are independently established.
- Resolver performance, cold registry/download performance, warm materialization, and no-op startup are distinct workloads. Combining them obscures where an optimization pays.

## Research-paper claims that need code/evidence gates

The three companion PDFs were read fully. Treat headline values as hypotheses, not baselines, until a reproducible harness and current code path substantiate them.

1. **Paper I — decide and ingest:** the claimed 38 ms cold full-tree Next.js install is not supported by the current 10.84 s isolated cold-next observation. The paper's “zero-copy/one-pass” archive description should be reconciled with Paper II's description of a canonical `store/<hash>.tgz` plus `store_unpacked/` output. The paper's topology-based polynomial/zero-backtrack claims need formal proof and adversarial differential tests before being used as guarantees.
2. **Paper II — store:** JAGR-S/JGrid are described as opt-in because their measured storage/time costs do not beat the default path. Keep that policy until a same-workload end-to-end win includes disk cost and correctness.
3. **Paper III — reuse:** capsules, status/fingerprint checks, GRM, and daemon attach target warm/no-op work. They cannot establish a cold resolver win, and freshness, explicit package requests, platform separation, and strict lockfile integrity remain hard gates.

## Plan

### Phase 0 — Freeze a trustworthy baseline

Build one release-binary harness that runs Jhol and npm, Yarn, pnpm, and Bun on identical pinned fixtures, in interleaved order. Record tool versions, Node version, OS/CPU, registry, cache state, package/lockfile hashes, network bytes, metadata requests, tarball requests, retries, stage timings, CPU time, peak RSS, and disk writes. Run at least 5 repetitions and report median and p95.

Separate these cases; never label them all “cold”:

- empty project cache + empty `node_modules` (true network-cold);
- populated package cache + empty `node_modules` (materialization-cold);
- metadata snapshot/cache warm but artifacts absent (resolver-cold, network excluded or fixed);
- fully warm cache and unchanged no-op.

Every successful run must check lockfile reproducibility, SRI, expected package tree, `npm ci` acceptance where applicable, and Node `require`/ESM loadability for fixture entry points. Keep failure rate, not just successful latency. A 10x claim is allowed only for a named case and workload when the full distribution and quality gates pass.

### Phase 1 — Attribute JAGR-Infinite's CPU and allocation cost

Add low-overhead per-stage counters/timers around bundle/lockfile hit checks, `fetch_dep_tree`, atlas construction, constraint mapping, full-chart construction/refinement, homotopy/RVM, and finalization. In a separate profiling run capture allocations and bytes per stage. Do not infer cost from call-graph complexity alone.

Acceptance: repeatable stage totals account for at least 95% of resolver wall time; benchmark output includes package, version, edge, and candidate counts; debug output is not enabled for headline runs.

### Phase 2 — One-pass version/edge representation

Prototype a compact immutable `PackageFacts` representation produced once per packument: canonical package ID, parsed/sorted versions, dependency/peer facts, and metadata fingerprint. Build atlas charts and the final constraint chart from these facts instead of independently extracting the same JSON. Keep the original path behind a test-only/reference switch until result parity is proven.

Acceptance: exact resolved assignment and error/fallback behavior match the current path across pinned fixtures, prerelease/range edge cases, overrides, peers, optionals, workspaces, platform filters, and malformed/missing metadata. Report allocation reduction and stage-level before/after; revert if end-to-end resolver time does not improve.

**Progress (2026-09-28):** E173 skips clustered atlas charts that full-tree resolution did not consume. On the 98-packument Express fixture, three warm-cache samples per binary produced byte-identical lockfiles; median resolver time excluding logged metadata-cache fetch was 4.71 ms before and 4.64 ms after, within noise. E174 removes a redundant version-key map; latency effect unmeasured. E175's global parsed-version catalog was reverted: lockfiles matched, but process timing (30 ms vs 40 ms median) was too noisy to prove a gain and the catalog raised peak-memory risk. E176 makes the RVM `VerifyDeps` gate check complete assignments against exact chart candidate bitmaps; it does not replace final SemVer legality checks. E177 moves package-graph topology and spectral diagnostics off the normal path; `JHOL_DEBUG_JINF` preserves the report. Focused JAGR tests pass (20), full workspace/all-features passes (586 passed, 10 ignored), release build succeeds. A fresh online debug/normal run pair emitted byte-identical lockfiles (SHA-256 recorded in `INNOVATION_LOG.md`) on this one fixture. Independent stage timers now show, in one debug run, 1.916 s packument fetch, 4.93 ms chart/constraints, 0.420 ms diagnostics, 1.54 ms solve/materialization, and 2.78 ms finalization; another run fetched metadata in 1.311 s. This is a network-dominated single-sample profile, not an E177 before/after result. No resolver speedup or 10× claim. Next: profile resolver-only with a fixed metadata snapshot across larger/version-heavy and peer-conflict fixtures; audit cache correctness/freshness; then prototype exact dependency/peer candidate propagation with differential tests. Defer topology-as-solver work until Paper I's dimension/β₂ conflict is resolved mathematically.

**Full-install profile (2026-09-28):** On the isolated Next.js fixture, cold runs measured 14.787 s and 9.231 s at default settings versus 9.228 s and 9.691 s with `JHOL_PACKUMENT_CONCURRENCY=64`; `after_expansion` accounted for 9.091–14.481 s, while cache installation took 106–229 ms. All four runs had the same lockfile SHA-256 and loaded Next.js. Two samples per setting were insufficient to infer a concurrency speedup; default remains unchanged. E180 then split network traffic around expansion and found that stage includes speculative tarball prefetch. One paired debug run measured 18.615 s with prefetch versus 20.504 s without it; lockfiles matched and both trees loaded. Keep prefetch enabled, but treat the ~1.9 s difference as directional only. Next: repeated interleaved release runs and per-request category timing before changing expansion or prefetch behavior. JAGR-only tuning cannot deliver a 10× true-cold install improvement by itself.

**Observability fix (E179):** `JHOL_HTTP_METRICS` previously showed zero for HTTP/2 traffic even on network installs; metrics read only `ureq` counters. The aggregate now includes the existing HTTP/2 pool snapshot without initializing the pool. Clean Next.js run reports 76 successful requests / 110,172,697 downloaded bytes and byte-identical lock output. This is measurement correctness, not a speed improvement. The install-log `cache_hits` field still mixes package count with request count and must not be treated as precise hit-rate evidence.

**Expansion attribution (E180):** With profiling enabled only, Jhol now logs HTTP request and body-byte deltas around transitive expansion. Prefetch-on measured 76 requests / 110,172,697 bytes inside the stage; prefetch-off measured 55 / 25,893,235 there, then spent 11.494 s in cold download/unpack/install. Both modes resolved identical lockfiles and passed runtime loading. This shows why optimizing the expansion phase in isolation could make the install slower; optimize end-to-end elapsed time with correctness gates.

**Transport ablation (E181, 2026-09-28):** Same release binary and isolated `5 cold-next` harness, three runs per mode: HTTP/2 median 9.931 s (9.734–12.417 s); forced HTTP/1.1 median 14.267 s (12.004–17.613 s). Keep HTTP/2 enabled. These were sequential groups, with no retained lockfile hashes; network/time drift and output parity remain uncontrolled. This measures the whole cold install, not JAGR resolver time, and cannot support a 10× claim. Repeat interleaved with retained locks and request-category timings. Benchmark JSON: `/tmp/jhol-cold-next-http2-20260928.json`, `/tmp/jhol-cold-next-http1-20260928.json`.

**Chart-build optimization (E182):** Dependency and peer extraction now deduplicates borrowed `(name, raw spec)` pairs before calling `spec_to_interval`, then preserves existing final `(package ID, interval)` deduplication. This removes repeated SemVer parsing for specs repeated across package releases without changing chart edge semantics. Focused regression and full workspace/all-features tests pass. No runtime speed claim yet; next benchmark `build_full_chart` against the same fixed metadata snapshot before/after and compare serialized chart plus resolved-lock outputs.

### Phase 3 — Exact candidate frontier, not lossy interval shortcuts

Represent non-convex candidate sets with an exact ordinal bitmap (or sorted ordinal runs when sparse). Maintain each package's allowed candidate set as intersections of accumulated direct, transitive, peer, override, engine, and platform constraints. Retain enough metadata to expand/refill when newly discovered constraints grow; never assume a pruned packument is complete. Track the source constraint for every elimination so conflicts can explain and safely backtrack.

Use this as an optimization only when its proof obligations hold. On unsupported registry/range semantics, fall back to the established resolver rather than choosing an approximate version.

Acceptance: differential comparison against the existing resolver and npm-compatible lockfile fixtures yields identical valid assignments/layouts or equivalent explicit errors; property tests exercise arbitrary range unions and prereleases; fuzzed malformed inputs fail closed; peak memory stays bounded.

**Implemented slice (E184, 2026-09-28):** Direct dependency ranges now produce exact bitmap candidates, so disjoint OR ranges do not admit versions in the gap; chart filtering and program cache identity include those bits. Lockfile homotopy rejects an out-of-domain direct selection and falls back to the RVM. JAGR-Infinite unit tests pass (21). This does not yet propagate transitive/peer constraints, and no full install parity fixture exercises the homotopy fallback. Do not claim complete exact-frontier solving or a performance gain; finish those correctness gates before extending this slice.

### Phase 4 — Bounded, content-keyed reuse for resolver inputs

Cache parsed package facts and immutable resolution inputs by canonical package identity plus registry URL/scope, metadata ETag or content digest, resolver schema version, platform/CPU/libc, and all resolution-affecting inputs (ranges, peers, overrides, optionality, workspace context). Define freshness/eviction and a maximum memory/disk budget. Never reuse a stale assignment just because a package name matches. Preserve `jhol ci` integrity verification and explicit install requests.

Acceptance: cache invalidation tests for every key dimension; concurrent writers/readers and corrupt-cache tests; cold/warm results report hit rates and storage overhead. Use the existing lockfile/capsule fast paths where possible rather than introducing redundant stores.

### Phase 5 — Shadow, canary, then default routing

Run the improved JAGR resolver in shadow mode against the established production resolver on real fixtures; compare package names, versions, peer layout, optional variants, lockfile output, and failure reasons. Then allow explicit opt-in canary; automatically fall back on disagreement or unsupported cases. Only route a stable cohort by default after the correctness corpus is green and the named latency target improves without higher failure rate or unacceptable RSS/disk costs.

Topology metrics may guide candidate ordering after ablation demonstrates value; they must not override semantic constraints. A novel contribution should be the exact incremental candidate-frontier/one-pass-facts engine and its correctness/performance evidence—not an unverified claim that graph topology removes NP-hard worst cases.

## Performance target reality

The measured true-cold Next.js fixture median is currently 10.84 s. A 10x target would be about 1.08 s on the same named workload and environment. If registry latency, TLS setup, or downloaded bytes dominate, resolver-only work cannot close that gap. First target 10x on **resolver-only** runs with registry data fixed locally; then report separately how much that changes true-cold end-to-end time. No algorithm can promise 10x across arbitrary networks and manifests without narrowing the workload or changing what “cold” means.

## Paper I implementation audit (2026-09-28)

The paper remains read-only. Current source contradicts several implementation claims, so benchmark claims must not be presented as current JAGR behavior:

- Paper I §5 defines version-set vertex stalks, compatible-pair edge stalks, and restriction maps. `compute_sheaf_invariants` currently receives package-name adjacency and computes connected components, a cycle-rank-like count from raw directed edge occurrences, directed triangle counts, and an unweighted graph spectral estimate. This is not the paper's cellular-sheaf coboundary/Laplacian construction; reported `β₁`, `β₂`, and spectral gap are not evidence of the stated sheaf invariants.
- RVM compilation currently emits `LoadChart -> GreedySCC -> VerifyDeps -> StoreResult -> Halt`. `GreedySCC` picks each package's highest allowed ordinal independently; it does not invoke RG decimation, Ricci scheduling, SCC solving, or backtracking. `CheckDep` and `CheckPeer` currently return `true`; `KnotInvariant` returns defaults; MaxSAT opcodes are stubs. The production resolver's later packument-backed law validation and PubGrub fallback protect final results, but do not make those RVM/paper algorithms real.
- `precompute::spec_to_interval` lacks the target package's published-version domain, so ordinary valid dependency/peer ranges become full ordinal intervals in chart edges. Exact selected-version legality is deferred to finalization. This is safe only because finalization and resolver fallback remain authoritative; it also means chart candidate propagation is not yet an implemented JAGR speedup.
- E176 strengthens `VerifyDeps` to enforce complete assignments inside chart candidate bitmaps. This is one correctness gate only; exact dependency and peer constraints remain enforced by finalization, and the mathematical claims above remain open.
- E177 keeps package-name topology, Fiedler, RG, and Ricci diagnostics behind `JHOL_DEBUG_JINF`. Current RVM bytecode does not consume chart Betti data, so normal installs avoid building that graph; debug report remains available. One fresh-network fixture pair produced identical debug/normal lockfiles, but no before/after latency result exists. Independent stage timers indicate registry fetch dominates this particular run; fixed-input resolver-only profiling is next.

Required path to make these claims real without weakening install correctness: resolve the paper's 1-D/β₂ specification conflict first; define and test coherent version-domain operators; then use validated invariants only as candidate-ordering hints with exact packument-based legality checks and fallback; finally reproduce each claimed runtime/complexity result on named fixtures with ablations. Do not use graph proxies as sheaf/homology claims or state NP-complete worst-case guarantees as universal polynomial guarantees.

## Questions to lock scope before implementation

1. By “cold,” do you mean empty Jhol cache plus real internet downloads, empty `node_modules` with a warm artifact cache, or resolver-only with a fixed metadata snapshot?
2. Is 10x required for the resolver stage, end-to-end install, or both? Which exact fixture and baseline manager define the target?
3. Should JAGR-Infinite become the default resolver, or stay opt-in until differential parity is established?
4. Which platforms are release blockers (macOS arm64, Linux x86_64/arm64, Windows)?
5. Are package mirrors/prewarmed metadata allowed, or must the benchmark use only the public registry with empty caches?
6. Which semantics are hard gates for the first rollout: peer conflicts, optional/platform packages, workspaces, aliases, overrides, prereleases, lifecycle scripts, or all of them?
7. What maximum resolver RSS and cache growth are acceptable in pursuit of latency?
8. May the next implementation phase edit resolver code and tests only, preserving all papers and existing user changes, with no commit/push?
