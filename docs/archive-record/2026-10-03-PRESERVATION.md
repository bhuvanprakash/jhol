# Preservation record — 2026-10-03 upgrade

Baseline: `5bfc48dbdb9db9f57ee55df6223737b8a97ff97ffae2`. Working tree: dirty,
uncommitted (owner rule). No git write was performed to build this record.

## Reconciliation — the number that matters

| | Count |
|---|---|
| Files deleted in the working tree (`git status --short \| grep '^ D'`), re-measured after the orphan moves | **63** (57 pre-existing + the 6 tracked-but-uncompiled moves below) |
| Of those, already archived before this round | 6 |
| Of those, restored from the baseline into `archive/` by this round | 51 |
| Working-tree deletions with **no** archived copy | **0** |
| Additional proven-orphan `.rs` files moved out of `crates/jhol-core/src/` | 6 |
| Total deletions accounted for | **63** |
| Files under `archive/` (incl. `README.md`, `MANIFEST.md`) | **71** |

**63 working-tree deletions in, 63 archived copies out, 0 unrecorded.** Every deleted path has
a matching archived copy and a `MANIFEST.md` row; every archived file has a row.

## A1 — closing the deletion gap (51 files restored)

Mechanism, read-only git only:

```sh
git show "HEAD:<path>" > "archive/<mapped-path>"
```

`git checkout` / `git restore` were not used — they destroy uncommitted work.
Mapping follows the archive's existing shape: core modules →
`archive/jhol-core/orphans/`, whole crates → `archive/<crate>/`, scripts →
`archive/scripts/<name>`.

| Group | Files | Archived to |
|---|---|---|
| `crates/jhol-core/src/*.rs` (7 new; 6 were already there) | 13 total | `archive/jhol-core/orphans/` |
| `crates/jhol-pipeline/**` (whole crate: Cargo.toml, PLAN.md, spec.md, `benches/`, `src/format`, `src/ingester`, `src/materialize`, `src/store`) | 24 | `archive/jhol-pipeline/` (relative paths preserved) |
| `scripts/*.py` | 20 | `archive/scripts/` |

Verified closing:

```
$ while read p; do <map to archive path>; [ -s "$a" ] || echo "MISSING $p"; done < /tmp/w3-del.txt
deletions=57 unaccounted=0
```

Where each removed core module went — this is the consolidation the upgrade
performed, and the public path is preserved by a re-export, so nothing was
actually lost even before the archive copy:

| Removed file | Still reachable as | Evidence in the live tree |
|---|---|---|
| `crates/jhol-core/src/osv.rs` | `audit::osv` | `lib.rs:87` `pub use audit::osv;` |
| `crates/jhol-core/src/distributed_grm.rs` | `grm::distributed` | `lib.rs:88` `pub use grm::distributed as distributed_grm;` |
| `crates/jhol-core/src/cdn.rs` | `http_client::cdn` | `lib.rs:89` `pub use http_client::cdn;`; `http_client.rs:835` `pub mod cdn` |
| `crates/jhol-core/src/incremental_resolver.rs` | `incremental_resolution::incremental_resolver` | `lib.rs:90` `pub use incremental_resolution::incremental_resolver;` |
| `crates/jhol-core/src/global_cache_2.rs` | `global_cache` | `global_cache.rs` carries the same symbol set (`CacheEntry`, `InstallPatterns`, `CacheStats`) with `GlobalCache2` → `GlobalCache`; `global_cache_2` is declared nowhere in the live `lib.rs` |
| `crates/jhol-core/src/content_addressed_store.rs` | nowhere — never declared | absent from **`HEAD`'s** `lib.rs` too; it was already unreferenced at the baseline |
| `crates/jhol-core/src/sat_resolver_tests.rs` | nowhere | `HEAD`'s `lib.rs:72` had `mod sat_resolver_tests;`; the live tree removed the declaration |
| `crates/jhol-pipeline/**` | `crates/jhol-core/src/jgrid/` | `jgrid/` has the same subdirectory shape (`format`, `ingester`, `materialize`, `store`) plus `PLAN.md` and `spec.md` |

## A2 — the record

- `archive/README.md` — policy, baseline, restore procedure (both the
  `git show` form and the `cp` form), and the rule for archiving something new.
- `archive/MANIFEST.md` — generated, one row per file under `archive/`:
  original path, archived path, kind, reason, date, baseline, restore command.
  Cross-checked against `find archive -type f`.

## A3 — orphan inventory (proof before any move)

Method: a file was treated as an orphan only if **all three** held.

1. Absent from every `target/**/*.d` dep-info file — meaning `rustc` was never
   given the path by any of the 15 workspace members under
   `cargo check --workspace --all-targets` (1544 `.d` files scanned, 271 `.rs`
   paths recovered).
2. No `mod <name>;` in any `.rs` under `crates`, `src` or `tests`.
3. No `#[path = ...]` or `include!` pointing at it.

Result: 9 candidates, 3 rejected as false positives, 6 confirmed.

| Candidate | Verdict | Proof |
|---|---|---|
| `crates/jhol-core/src/fuse_mount.rs` | **kept — wired** | `lib.rs:78-79` `#[cfg(target_os = "linux")] pub mod fuse_mount;` |
| `crates/jhol-core/src/io_uring_wrapper.rs` | **kept — wired** | `lib.rs:80-81` `#[cfg(target_os = "linux")] pub mod io_uring_wrapper;` |
| `crates/jhol-runtime/src/jsc_engine.rs` | **kept — wired** | `jhol-runtime/src/lib.rs:5-6` `#[cfg(feature = "jsc-engine")] pub mod jsc_engine;` |
| `crates/jhol-core/src/content_addressed/tests.rs` | moved | no `content_addressed/mod.rs`; dir holds only this file |
| `crates/jhol-core/src/instant_lock/tests.rs` | moved | no `instant_lock/mod.rs` |
| `crates/jhol-core/src/provenance_log/tests.rs` | moved | no `provenance_log/mod.rs` |
| `crates/jhol-core/src/grm/store/tests.rs` | moved | `grm/store.rs` has no `mod tests;` |
| `crates/jhol-core/src/task_queue/queue.rs` | moved | `task_queue/mod.rs` declares only `adaptive`, `parallel` |
| `crates/jhol-core/src/task_queue/worker.rs` | moved | same |

The three `cfg`-gated files are the reason the dep-info test alone is not a
proof: a default-feature macOS build simply never sees them. Full detail, callers
and "what would make it live" for every dormant module:
[`UNWIRED_BUT_KEPT.md`](UNWIRED_BUT_KEPT.md).

## A4 — verification

| Check | Command | Result |
|---|---|---|
| Deletion gap closed | per-path archive-existence loop over `git status --short \| grep '^ D'` | `deletions=57 unaccounted=0` (pre-move run; the same loop reports `deletions=63 unaccounted=0` after the 6 orphan moves) |
| MANIFEST completeness | `find archive -type f` vs MANIFEST rows | every file has exactly one row (see MANIFEST header) |
| Build not regressed | `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` | exit 0 |
| Nothing deleted | `git status --short \| grep -c '^ D'` | **63** = 57 pre-existing + 6 tracked-but-uncompiled files moved to `archive/jhol-core/orphans/`. Git counts a move as a deletion; all 6 have a MANIFEST row and their content in `archive/`, so no content was lost |

## A2b — precedent chain (linked, not duplicated)

`archive/README.md` and `archive/MANIFEST.md` both point at the records that
already own parts of the preservation story instead of copying them:

- `donotpushtheseonlineanddonotaddtheseingitignoreandmaketherecordofit.md` (repo
  root, untracked) — the owner's not-to-commit list, the author-hygiene rule and
  the two 2026-09-30 `git filter-repo` passes. Those paths are untracked **by
  design**: never deleted, never added to `.gitignore`.
- `docs/archive/REORG_MANIFEST.json` (148 entries) + `docs/archive/REORG_LOG.md` —
  the 2026-09-30 local reorg. Its house style is the precedent this archive
  follows: identical copies deduped, **diverged copies kept** as
  `docs/archive/junk/*.root-dup` with the canonical copy named.
- `Documentation/README.md` — `Documentation/` holds exactly that one file.
  `stat -f '%i %N' docs Documentation` → `157898054 docs`,
  `157804427 Documentation`: separate inodes, separate roles, neither merged nor
  deleted, and `archive/` does not absorb either.

Not archived, deliberately: the 5 root `*.bak-20261003-042423` files
(`ARCHITECTURE.md`, `Cargo.toml`, `CHANGELOG.md`, `GET_STARTED.md`, `README.md`)
are sibling agents' transient pre-edit scratch backups, not history.

## What the owner should know

- Nothing was lost. Every one of the 63 working-tree deletions is now recoverable
  from `archive/` **and** from `git show 5bfc48db:<path>`, whichever is more
  convenient.
- The archive is net-additive: it contains 71 files and W3 removed no content
  from the tree except 6 `.rs` files that were provably never compiled and that
  now sit in `archive/jhol-core/orphans/` with their relative paths intact.
- Three dormant-in-macOS-build modules (`fuse_mount`, `io_uring_wrapper`,
  `jsc_engine`) are wired behind `cfg` gates and were deliberately **not**
  touched — moving them would have turned a working opt-in capability into a real
  orphan.
- `crates/jhol-cloud/` is an empty directory with no `Cargo.toml`: it is recorded
  here rather than deleted, per the preservation rule. It advertises a capability
  that does not exist; either give it content or stop referencing it.
  **Status update 2026-10-24:** it now has a manifest and `src/lib.rs`, is a
  workspace member with 12 passing tests, and implements an offline control-plane
  domain model. No service is deployed. See
  `docs/architecture/FOUNDATION_CRATES.md`.
