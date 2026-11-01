# Ablation: agent defaults vs JGrid/JAGR-S vs both (30 Sep 2026)

**Constraint:** jhol-native levers only (harvest, Instant Lock, optional mirrors). No Bun internals.

**Fixture:** `ms@2.1.3`, `is-number@7.0.0`, `mime-types@2.1.35`, `axios@1.7.7`, `chalk@4.1.2`  
**Protocol:** interleaved n=9; fresh `JHOL_CACHE_DIR`; daemons off; `JHOL_PREBUILT=0` + unique prebuilt dir (harvest arm); lockfile present; quality = `require('axios'|'ms'|'chalk')`.

Raw: `docs/architecture/experiments/jgrid-jagrs-ablation-results.json`.

## Arms

| Arm | Meaning |
|-----|---------|
| **A** agent defaults | harvest on; JGrid gated off; JAGR-S off |
| **B** forced mirrors | `JHOL_JGRID=1` + `JHOL_JGRID_INGEST=1` + `JHOL_JAGRS_MIRROR=1` on empty project cache |
| **C** both (safe) | harvest on; JGrid still gated; JAGR-S mirror **writes** on |
| **bun** | fresh `HOME` + `BUN_INSTALL_CACHE_DIR` |

## Fat-binary cold materialize (no trampoline L0)

| Arm | median ms | vs Bun | quality | notes |
|-----|----------:|-------:|:-------:|-------|
| A | 606.6 | 0.93× | yes | grid 0 / jagrs 0 |
| B | 594.6 | 0.95× | yes | grid 0; jagrs ~775 KB written; no speed win |
| C | 554.9 | 1.01× | yes | jagrs write tax ≈ noise |
| bun | 562.4 | 1.00× | — | |

**Verdict:** turning JGrid/JAGR-S back on does **not** buy cold speed on this path. Agent gate stays correct. “Both” ≈ A (parity with Bun this session). Harvest still fired (debug sample: 18+ local SRI hits); quiet harvest medians ~300–400 ms on a calm rerun — host noise dominates A/B/C deltas.

## Instant Lock L0 (jhol soul — whole-tree reuse)

Rust trampoline + `.jhol-prebuilt-key` → symlink `/tmp/.../<hash>-v2/node_modules` (n=15):

| | median | quality |
|--|-------:|:-------:|
| jhol trampoline L0 | **11.8 ms** | yes (axios/ms) |
| bun cold (isolated) | **540 ms** | — |
| **ratio** | **~46×** | |

C front (`jhol-front`) only does `.jhol-ok` warm; L0 lives in Rust trampoline — use trampoline (or front that delegates L0) for this lever.

## What this means for “4× cold”

- **4×+ without copying Bun:** Instant Lock L0 (and prior harvest ~5× when machine-local SRI dense) — already jhol-native.
- **JGrid/JAGR-S as default:** still reject for cold; B wrote bytes, no median win, empty `store.jgrid` on fresh cache.
- Blueprint scaffolding (other agent) orthogonal to this race; keep install/harvest/trampoline as the speed plane.

## Skipped / ponytail

- No new org repos (owner git-write ban + ADR transfer checklist).
- No re-wiring JGrid as default.
- Blueprint crates already present — no duplicate scaffold this round.

## Follow-up path audit (30 Sep 2026)

The n=9 table is an end-to-end comparison for its lockfile install route. It
does **not** prove JGrid materialization speed. In `install_package`, a matching
lockfile can return from `install_lockfile_layout` before the later
`try_jgrid_install` call. A separate resolver-path probe was needed.

I rebuilt `target/release/jhol` from the current checkout, then ran isolated
resolver installs for the same five pinned roots. The seed used
`JHOL_JGRID=1`, `JHOL_JGRID_INGEST=1`, and `JHOL_JAGRS_MIRROR=1`. It exited 0,
installed 34 package manifests, passed `require('axios')`, `require('ms')`, and
`require('chalk')`, and reported `JGrid: ingested 34 packages`. The resulting
stores were 4,296,518 bytes for `grid/store.jgrid` and 784,657 bytes for
`v3/objects.jagrs`.

Then I replayed the same manifest into fresh projects with `JHOL_NO_HARVEST=1`:

| Probe | Seeded store | Exit / package tree | JGrid fast-path trace | Quality |
|---|---|---|---|---|
| JGrid only | JGrid store | 0 / 34 manifests | `before_jgrid` then normal fallback; no `jgrid_fast_path` | 3 imports pass |
| JAGR-S only | JAGR-S objects + store index | 0 / 34 manifests; 34 tarballs materialized | JGrid disabled | 3 imports pass |
| Combined | both stores + store index | 0 / 34 manifests; 34 tarballs materialized | `before_jgrid` then normal fallback; no `jgrid_fast_path` | 3 imports pass |

This establishes that JAGR-S can supply package bytes to the normal install
path when its object store and index are present. It also establishes that the
current JGrid artifact did not satisfy this resolver replay, even after seeding
34 packages; the combined arm therefore fell back to JAGR-S/normal materializing
and showed no JGrid hit. The issue to solve is JGrid artifact parity/lookup, not
turning its cold gate on.

These were path/correctness probes, not a timing comparison: each arm ran once,
metadata still made registry requests, and there was no matched control median.
The previous n=9 table remains evidence only for its tested route. Do not claim
4x general cold speed from either that table or an L0 replay whose prebuilt tree
was seeded by an earlier install.

## Clean-store materialization follow-up

I inspected the failed JGrid store at the byte level. It has 34 manifest rows,
but three rows have an empty name offset; their file counts match `chalk`,
`form-data`, and `function-bind`. The store therefore cannot satisfy exact
`name@version` lookups for those packages. The on-disk package manifests and
`read_all_installed_packages` return the expected names, so the bad identities
have not yet been tied to a reproducible source-level cause.

As a control, I re-ingested all 34 installed package trees through the current
`JGridPipeline` into a clean temporary store. All 34 names round-tripped. With
that store and the lockfile removed, an offline replay with `JHOL_JGRID=1` and
`JHOL_NO_HARVEST=1` reached `jgrid_fast_path` in 304 ms of profiler time,
rebuilt 34 `.bin` links, and passed imports of axios, chalk, form-data,
function-bind, and ms. This is one warm-store correctness probe, not a Bun
comparison or a general cold-install result. The earlier blank-manifest
artifact has no reproduced source cause; the repository source was not changed
in this follow-up.

## Current-hook replay confirmation

I exercised `post_install_jgrid_ingest` with the current debug binary, an
isolated cache copy without a JGrid store, and the pinned fixture in offline
mode. It wrote 34 valid manifest identities;
five root imports passed. Replaying that hook-generated cache into a second
empty project (no lockfile, offline, harvest disabled) reached
`jgrid_fast_path` in 263 ms of profiler time / 298 ms reported wall time,
rebuilt 34 `.bin` links, and passed the same five imports. The earlier malformed
store is not reproduced by the current hook. This single warm-store replay
confirms the route works; it is not a matched Bun benchmark, and the older
malformed artifact remains unexplained.

## Matched warm-cache replay (n=9)

I ran nine interleaved installs per tool from fresh project directories with
the same pinned `package.json` and no lockfile. Jhol used the hook-generated
JGrid/store cache with offline JGrid enabled; Bun used its own isolated,
prewarmed install cache. Every sample passed the same five imports.

| Tool | Median wall time | Quality |
|---|---:|:---:|
| Jhol + JGrid | 87.9 ms | pass |
| Bun | 43.5 ms | pass |

Jhol was 2.02× slower in this matched warm-cache replay. The host was noisy
(sample ranges: Jhol 61–327 ms, Bun 31–243 ms), so treat this as directional.
It is not a cold-network test, but it does not support a 4× speed claim for
JGrid on this fixture. The earlier cold lockfile-path n=9 results remain the
relevant evidence for first installs.
