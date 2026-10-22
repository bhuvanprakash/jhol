# Paper Claim Audit — 2026-10-08

**Question asked:** *are all claims in my 3 papers true? if not, what did we reject?*

Method: every claim checked against (a) the code as it runs today, (b) a
measurement from this session where the claim is measurable. Papers were NOT
edited (standing instruction).

Verdict key: **TRUE** = code matches claim and/or measurement confirms ·
**REJECTED** = measured false or superseded by the code's own documented design ·
**UNVERIFIED** = neither reproduced nor disproven this session (usually a
protocol difference).

---

## Paper 1 — GRM & Instant Lock (reuse)

| # | claim | verdict | evidence |
|---|---|---|---|
| 1.1 | GRM lock-free DashMap/XXH3/ordinal resolution memory | TRUE (dormant) | `lock_free_grm.rs`, `jagr_resolver.rs` exist exactly as described; reads only by the pre-infinite resolver (noted) |
| 1.2 | Capsule `JHLCPL` mmap zero-parse install plan | TRUE | capsule fast path observed live; adversarial-open tests exist |
| 1.3 | Soul Path DSSC Stable/Marginal/Fragile + `can_skip_resolve` | TRUE | build emits DSSC certs (575); `dssc_first_install` runs |
| 1.4 | Daemon: thread-per-conn, `DAEMON_CWD_LOCK`, timeouts, `JHOL_CACHE_DIR` isolation | TRUE | all four contracts read in source (E66/E84/E104 lineage) |
| 1.5 | Freshness calculus: manifest hash + binary fingerprint + `nm_entries` | TRUE | `try_exit_before_clap` + fixture `.jhol/status.bin`; noop works |
| 1.6 | no-op install ≈ Bun (~7.5-11ms vs 7.8ms) | TRUE (consistent) | ours: 6.4ms vs 6.9-7.9ms calm window |
| 1.7 | daemon attach 30ms vs direct 48ms (−38%) | **REJECTED** | A/B measured **1.0×** (changed-manifest flow) |
| 1.8 | ~300-pkg warm 76-127ms vs bun 179-235ms | UNVERIFIED | bun warm fresh on our fixture is 36-103ms (different machine/protocol); not reproduced |
| 1.9 | "strict `ci` requires verifiable lockfile with integrity" | TRUE | holds — and the dead-end around it was fixed this session (#8) |

**Paper 1 score: 7 TRUE, 1 REJECTED, 1 UNVERIFIED.**

---

## Paper 2 — JAGR-S (store)

| # | claim | verdict | evidence |
|---|---|---|---|
| 2.1 | JAGR-S v3 store with SwissProbe-16 / Elias-Fano / sheaf locality | UNVERIFIED (parts TRUE) | store code exists; the ns-level microbenchmarks not re-run; mirror is opt-in — matches paper's own §8 |
| 2.2 | Invariant dual-projections + Early L0 lane (H(Σ_spec)/H(Σ_manifest) → bundle) | TRUE | aliases + `try_jgrid_install` live; **extended beyond paper** (deps-manifest key, no-lockfile replay) |
| 2.3 | Tier-0 C sentinel checks `.jhol-ok` mtime and exits 2.4-4.1ms | **REJECTED** | current `trampoline.c` is a 54-line exec-only shim; freshness lives in Tier 1 (Rust). The described C sentinel does not exist in code — and re-adding it would violate the repo's own mtime-proof soundness rule |
| 2.4 | XXH3 deterministic lattice → 100% cache hits | TRUE (after fix) | determinism true; the `.jlat` mirror was actually **write-only** (bincode×Value) until this session (#4) — the claim's spirit was not met in reality before the fix |
| 2.5 | Completeness-marked unpacked trees (`.jhol-extract-complete`) | TRUE | markers gate links everywhere |
| 2.6 | SRI harvest from `_cacache` | TRUE | harvest path live; guarded by `JHOL_NO_HARVEST` |
| 2.7 | Honest default policy (CoW default, JAGRS opt-in) | TRUE | matches repo's own ablation (parity +197MB) |
| 2.8 | "sub-millisecond direct CoW bundle projection" | **REJECTED** | measured **23ms** for 2329-node clonefile; contradicted by the paper's own §3.2 (11.3-18.7ms) |
| 2.9 | warm no-op 4.10ms vs bun 12.08ms | UNVERIFIED (consistent direction) | ours: 6.4 vs 7.9ms — same winner, different protocol |
| 2.10 | store re-materialization 41.25ms vs bun 31.89ms; daemon 15.63ms | UNVERIFIED | plausible class, not reproduced under our protocol |

**Paper 2 score: 4 TRUE, 2 REJECTED, 4 UNVERIFIED (1 of the 2 rejections is also a superseded design).**

---

## Paper 3 — Sheaf Homotopy & Ricci (decide)

| # | claim | verdict | evidence |
|---|---|---|---|
| 3.1 | The six mathematical components exist as modules | TRUE | RG decimation, Ricci scheduling, sheaf Laplacian/Fiedler, Mayer-Vietoris, tropical exponentiation, Markov-Ricci all present in source |
| 3.2 | The production resolver *consumes* these values (this is the paper's central thesis) | **REJECTED** | the repo's own ARCHITECTURE.md: the topology/RG/curvature computations are **diagnostic-only** behind `JHOL_DEBUG_JINF`; the production RVM does not consume them. Our experiments agree: warm-start 1.0×, resistance-ordering 0.09×, heat-kernel 0.01×; profiled math = 1ms of 910ms hot path |
| 3.3 | FastStream SSDP single-pass ingestion | TRUE | streaming projector live in download/harvest paths |
| 3.4 | Ablations: RG 342→61 vars / 11.4× solver speedup / zero-backtrack Ricci 18.5× | **REJECTED as production claims** | measured on the diagnostic path; production resolves without them and is faster than all listed baselines anyway |
| 3.5 | cold full-tree 38ms (Next.js) | **REJECTED under strict protocol** | our jhol-web (363 pkgs): **3.09s** with store, 10.2s network. Paper's "cold" = pre-populated store on RAM-disk = a different metric |
| 3.6 | warm 17ms / no-op 5.3ms / RSS 6.7MB / 1.32× bun | UNVERIFIED (partially consistent) | warm measured 31-97ms on our protocol; RSS not measured this session |
| 3.7 | 493 tests, 100% Node.js loadability | TRUE (loadability) / superseded (count) | suite is larger now (525+525), runtime loads verified repeatedly incl. load-storm incident review |
| 3.8 | Mayer-Vietoris zero-backtrack *guarantee* | **REJECTED as production claim** | true of the module, unused by production (see 3.2) |

**Paper 3 score: 3 TRUE, 4 REJECTED, 1 UNVERIFIED.**

---

## Final tally across all three papers

| | TRUE | REJECTED | UNVERIFIED |
|---|---:|---:|---:|---:|
| Paper 1 (GRM/Instant Lock) | 7 | 1 | 1 |
| Paper 2 (JAGR-S) | 4 | 2 | 4 |
| Paper 3 (Sheaf Homotopy) | 3 | 4 | 1 |
| **TOTAL (21 claims)** | **14** | **7** | **6** |

### Are all claims true? **No — 14 of 21 true, 7 rejected, 6 unverified.**

### The 7 rejections, one line each
1. **P1: daemon −38%** — measured 1.0× this session.
2. **P2: Tier-0 C sentinel does mtime freshness checks** — the C tier is exec-only today; re-adding it would break the repo's own soundness rule.
3. **P2: sub-millisecond CoW projection** — 23ms measured for the real node count; contradicted by the paper's own §3.2.
4. **P3: the resolver consumes the sheaf/RG/curvature math** — diagnostic-only per the repo's own architecture doc; experiments confirm inert on hot paths.
5. **P3: solver ablations as production results** — same cause as #4.
6. **P3: 38ms cold Next.js** — metric definition differs (pre-warmed store); strict cold measured 3.09s.
7. **P3: Mayer-Vietoris zero-backtrack guarantee in production** — real math, unused by the production path.

### Honest note for the author
The pattern in the rejections: **the systems claims that are true are the ones
the code was fixed to honor this session** (lattice determinism, ci integrity,
dual-projections) **and the math claims are true of code paths that don't run in
production.** Nothing here says the research is wrong — the modules exist, the
math is implemented — but the papers describe some things as production
behaviour that the production build deliberately does not do. The update path is
either a "diagnostic math" section relabeled as such, or a wiring project that
must first beat the same measurement gauntlet everything else survived this
session. Papers untouched per standing order.

---

## Brutal re-test of the 6 UNVERIFIED claims (2026-10-08, real 363-pkg jhol-web)

Protocol: interleaved, same machine/minute, all rounds asserted (rc=0, ≥250
entries), host load ~17 (stated; both tools equally loaded), single-process
Python harness, per-round disk cleanup. JAGR-S microbench claims are not
reachable through the CLI and were not fabricated a bench for.

| claim | brutal result | verdict |
|---|---|---|
| **1.8 / 3.6: 300-pkg warm = 76-127ms, bun 179-235** | jhol **711ms**, bun **759ms** (load-17 window) | numbers **REJECTED**; **direction TRUE** (jhol 1.07× faster) |
| **2.1: JAGRS SwissProbe 1.8ns / Elias-Fano 3.4ns / locality** | unreachable via CLI; mirror is opt-in by the paper's own policy | **NOT TESTABLE this session** (no fabricated harness) |
| **2.9: noop 4.10ms, 2.9× bun (12.08ms)** | jhol **6.4ms** calm / 22-26ms loaded; bun **7.9ms** calm / 22-26ms loaded | numbers **REJECTED**; jhol wins noop ~1.2×, **not 2.9×**; bun is ~8ms, not 12.08 |
| **2.10: rematerialization 41.25 vs 31.89** | jhol **979ms**, bun **705ms** | numbers **REJECTED** (20-30× higher, load+scale); **direction TRUE — bun wins remat** (hardlink vs CoW physics, expected & documented) |
| **2.10: daemon 15.63ms, sub-20 IPC, −38%** | attach **1160ms** vs direct **681ms** | **REJECTED** — daemon is **71% slower** at scale (and 1.0× in the small-graph A/B) |
| **3.6: warm 17ms / noop 5.3ms / RSS 6.7MB leaner / 1.32×** | warm 711ms, noop 6.4ms, RSS **jhol 15MB vs bun 7MB (2.11× more)**, warm ratio 1.07× | **ALL NUMBERS REJECTED**; direction of warm win true; **"39% leaner" is false — jhol is 2.1× bun's RSS** (both tiny in absolute terms) |

### Bonus the papers never claimed (found brutally)
**Cold at 363-package scale: jhol 7625ms vs bun 8204ms — jhol WINS cold (1.08×)**
on real projects, the first scale-correct cold measurement this session.

### Final audit tally (27 claims)
- TRUE: **14** (+2 direction-confirmed from the brutal pass, +1 bonus axis)
- REJECTED: **12** (7 original + 5 from this brutal pass)
- UNVERIFIED: **1** (JAGRS ns-level microbenchmarks, not CLI-reachable)

### Author-facing summary (unchanged papers, per standing order)
The systems claims survive measurement in **direction** (warm faster, noop
faster, cold-even-wins-at-scale) but not in **magnitude** — every absolute number
in the papers is optimistic for this host. The math claims remain
diagnostic-only. The two places reality **contradicts the papers' direction**:
jhol is *heavier* on memory (15MB vs 7MB, not 39% leaner), and the daemon is
*slower*, not faster, on the one workload it was claimed to accelerate.
