# Unwired, unused or unreferenced — and kept anyway

Date: 2026-10-03 · Baseline: `5bfc48db` · Owner rule: *do not remove something just
because it is not wired or not used or not completed.*

This file is the record the rule asks for. It lists things that are dead,
dormant or empty **in the live tree**, states the evidence that says so, and says
what would make each one live. None of them were moved or deleted unless the
proof was absolute (see the last section).

## A. Compiled, declared, but never called from the production path

These are reachable from a `mod` declaration, so they compile and their tests
run. They are **not** orphans and were **not** moved. `graft callers`-equivalents
(`grep -rn` over `crates src tests`) were used because each name is unique.

### A1. `crates/jhol-core/src/jagr/jagr_resolver.rs` — `JagrResolver`

- Declared: `crates/jhol-core/src/lib.rs` (via `jagr/mod.rs:22
  pub use jagr_resolver::JagrResolver;`).
- Only call site: `crates/jhol-core/src/lockfile_write.rs:1266`
  (`JagrResolver::new().resolve(&request)`) — the `jhol jagr` /
  `--jagr` diagnostic path, not the install path. Constructed at
  `crates/jhol-core/src/config.rs:625`.
- Smoking gun that it is not on the hot path:
  `crates/jhol-core/src/lib.rs:487` takes `_resolver: &crate::jagr::JagrResolver`
  — underscore-prefixed, i.e. deliberately unused — and `lib.rs:126` has the
  root re-export commented out.
- The native install path (`install.rs::expand_with_transitive_dependencies`) and
  JAGR-Infinite do not call it. This matches the recorded finding in `AGENTS.md`:
  *"`JagrResolver::resolve()` is NEVER called by the install command."*
- **Why kept:** JAGR is the project's resolver identity, not a spare part. Its
  layers feed `jagr_infinite` and the `jhol jagr`, `jhol audit-jagr`,
  `jhol betti-history`, `jhol jagr-impact` commands.
- **What would make it live:** routing `--resolver jagr` to
  `JagrResolver::resolve()` instead of aliasing it to JAGR-Infinite. That is the
  resolver-flag-honesty decision (W2 / owner call), not an archive decision.

### A2. `crates/jhol-core/src/spectral_sparsifier.rs`

- Declared `pub mod spectral_sparsifier;` at `crates/jhol-core/src/lib.rs:76`.
- `grep -rn "SpectralSparsifier" crates src tests` outside its own file returns
  nothing → no production caller found.
- **Why kept:** it compiles, it is tested, and it is one of the documented
  innovation modules (graph sparsification feeding the JAGR analysis stack).
  Removing it to shrink the tree is exactly what the owner forbade.
- **What would make it live:** wiring it into the `jagr` analysis pipeline as a
  pre-pass on large charts.

### A3. `crates/jhol-core/src/zero_copy_metadata.rs`

- Declared `pub mod zero_copy_metadata;` at `crates/jhol-core/src/lib.rs`.
- `grep -rn "ZeroCopyMetadata" crates src tests` outside its own file returns
  nothing → no production caller found.
- **Why kept:** same reasoning as A2 — compiled, tested, documented capability.
- **What would make it live:** using it on the packument read path where the
  metadata ceiling is currently dominated by materialising `serde_json::Value`
  copies (the ~12× duplication problem W1 is measuring).

## B. Compiled only under a non-default `cfg` — invisible to a default build

These did **not** appear in any `target/**/*.d` dep-info file for the default
`cargo check --workspace --all-targets` run, which makes them *look* like orphans.
They are not: each has an explicit, deliberate gate. Moving them would orphan
them for real.

| Module | Gate | Evidence | Callers |
|---|---|---|---|
| `crates/jhol-core/src/fuse_mount.rs` | `#[cfg(target_os = "linux")]` | `lib.rs:78-79` | `tests/innovations.rs:247` (`jhol_core::fuse_mount::{instant_mount_node_modules, JholFS}`) |
| `crates/jhol-core/src/io_uring_wrapper.rs` | `#[cfg(target_os = "linux")]` | `lib.rs:80-81` | `tests/innovations.rs:225` (`jhol_core::io_uring_wrapper::{batch_read_files, IoUringWrapper}`) |
| `crates/jhol-runtime/src/jsc_engine.rs` | `#[cfg(feature = "jsc-engine")]` | `crates/jhol-runtime/src/lib.rs:5-6` | `jhol-runtime/src/lib.rs:36`, `:66`; `crates/jhol-runtime/examples/bench_jsc.rs:18,24` |

**Why kept:** they are opt-in by design (`fuse_mount` is Landlock/`io_uring`-class
Linux-only functionality; `jsc-engine` is the optional JavaScriptCore backend).
They are compiled and exercised on the platforms/features that declare them.

## C. Formerly empty or contentless in the live tree

### C1. `crates/jhol-cloud/` — formerly an empty stub (resolved 2026-10-24)

- **Original finding:** contents were `crates/jhol-cloud/src/` only, with no
  `Cargo.toml` and no `.rs` file, and the directory was absent from the workspace
  `members`. It advertised a capability jhol did not have.
- **Resolution:** the crate now has `Cargo.toml` and `src/lib.rs`, is a workspace
  member, and passes 12 targeted tests for tenants, scoped hashed tokens,
  quotas, idempotency, deterministic CAS routing, and request envelopes.
- **Still not live:** no server, persistence, transport security, live provider,
  or deployment exists. Nothing should claim a hosted `jhol cloud` service.
- **Kept, not deleted.** The original empty directory had no bytes to archive;
  this entry preserves the finding and its resolution.

### C2. Build-output directories (not source — left alone)

`crates/jhol-core/target/{test-cache,jhol-cache}/store`,
`tests/fixtures/react-app/.jhol-cache/{daemon-cache,grid/trees,grid/bundles}`,
`tests/fixtures/peer-conflict/target/jhol-cache/daemon-cache` are empty
gitignored build/cache directories. They are not source, hold no data, and were
deliberately not touched. (`tests/fixtures/react-app/.jhol-cache/` in particular
is test-runner output: removing it can flip fixture-dependent tests.)

## D. Moved to `archive/` — the only files W3 removed from the live tree

Six `.rs` files had **no reference anywhere** and were never compiled. Proof
required all three of: absent from every `target/**/*.d` dep-info file (so never
passed to `rustc`), no `mod <name>;` in any `.rs` under `crates`, `src` or
`tests`, and no `#[path]`/`include!` pointing at them.

| File | Lines | Why it was unreachable |
|---|---|---|
| `crates/jhol-core/src/content_addressed/tests.rs` | 137 | Its directory contains **only** this file — there is no `content_addressed/mod.rs` and no `mod content_addressed;` anywhere, so `use crate::content_addressed::{...}` cannot resolve. Tests for a module that no longer exists. |
| `crates/jhol-core/src/instant_lock/tests.rs` | 106 | Same shape: `instant_lock/` holds only `tests.rs`; no `instant_lock/mod.rs`. (`lib.rs:29` declares `instant_lock_proof`, a different module.) |
| `crates/jhol-core/src/provenance_log/tests.rs` | 140 | Same shape: `provenance_log/` holds only `tests.rs`; no `provenance_log/mod.rs`. |
| `crates/jhol-core/src/grm/store/tests.rs` | 96 | `grm/store.rs` exists but contains no `mod tests;` (`grep -rn "mod +tests" crates/jhol-core/src/grm` → nothing). The two live `mod tests;` declarations are `pubgrub/mod.rs:28` and `mpt_store/mod.rs:3`. |
| `crates/jhol-core/src/task_queue/queue.rs` | 303 | `task_queue/mod.rs` declares only `mod adaptive;` and `mod parallel;`. `grep -rn "mod +(queue\|worker)" crates src tests` → nothing. Its own header says so: *"Much simpler and more reliable than custom implementation"* refers to the replacement. |
| `crates/jhol-core/src/task_queue/worker.rs` | 66 | Same: never declared. |

All six now live under `archive/jhol-core/orphans/` at their original relative
paths (`archive/jhol-core/orphans/task_queue/queue.rs`, …) with a MANIFEST row and
a restore command. `cargo check --workspace --all-targets` still exits 0 after the
move, as it must — they were never passed to `rustc`.

**Soul note, deliberately recorded:** `task_queue/queue.rs` and `worker.rs` were
the hand-written work-stealing queue and worker pool, replaced by
`adaptive.rs` + `parallel.rs`. Existing evidence (N14/N15 in the work log) says
the replacement is the measured winner, so this is a *supersede*, not a
*retirement* — the old implementation is kept and can be brought back if the
adaptive path ever regresses.

## E. What was explicitly *not* done

- `hypergraph_resolver`-class modules were **not** moved in this round; they were
  already archived on 2026-10-02 under `archive/jhol-core/orphans/` with their own
  README, and that record stands.
- No test was deleted, `#[ignore]`d or weakened. No `src/`, `README.md`,
  `CHANGELOG.md`, `Cargo.toml`, `docs/engineering/**` or `docs/production/**` file
  was touched. No git write was performed.
