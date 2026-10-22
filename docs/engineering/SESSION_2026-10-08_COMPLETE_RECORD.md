# Session Record — 2026-10-08: The Complete Optimization & Truth Audit

> One session, one rule: **nothing is claimed until measured; nothing is fixed
> blind; the soul (store, resolver, formal semantics, CoW isolation, no acronym-ization,
> no competitor cloning) is never traded for speed.** Every appendix below is
> backed by interleaved measurements, asserted work, and differential tests.
> All numbers logged with host load at the time.

## Environment notes (matters for every number)
- macOS arm64, 10 cores; bun 1.4.2; jhol built release (LTO, thin).
- Host load wandered 0.5 → 97 (foreign `multrocessing` pools, not jhol).
  Same-window interleaved runs are load-fair; absolute ms from load>10 windows
  are inflated and flagged.
- Measurement traps closed this session: bun's cache lives in passwd-home
  (`$HOME` tricks do NOT wipe it — must remove `~/.bun/install/cache` +
  `~/Library/Caches/bun`), jhol's harvest resolves via getpwuid
  (`JHOL_NO_HARVEST=1`), per-call `python3 -c` timing adds ~25ms (single-process
  harness used instead), `rm node_modules` while APFS clone is live
  ("Directory not empty" races).

## What was fixed (with proofs)

| # | fix | file(s) | proof |
|---|---|---|---|
| 1 | dssc_first_install: sequential extract → parallel + store-link first | install.rs | 525 tests |
| 2 | download window 4→16, clamp 64 | registry.rs | 1.38s→0.60s, plateau at 16 |
| 3 | disk-hit packument parse parallelized (VORTEX L3) | registry.rs | benchmark |
| 4 | lattice `.jlat` codec bincode→JSON (write-only!) | jagr_infinite.rs | 87/87 hits, expansion 1159→~0ms |
| 5 | fresh-project bundle replay (no lockfile): deps-manifest key + lockfile in bundle | install.rs, proof.rs, trampoline | 67-81ms, tree verified |
| 6 | `bundled_index_allowed` ranges online | registry.rs | expansion 1987→103ms, 86→4 requests |
| 7 | deps/meta enrichment from archives (601/607, 82 w/ peers) | scripts | differential harness |
| 8 | integrity backfill across all lanes (ci dead-end) | lockfile_patch.rs | legacy 0→87/88 integrity, ci rc=0 |
| 9 | synth peer/platform truth (meta.json) | registry.rs | strict-peer identical both paths; 24 os/cpu entries |
| 10 | store-first ordering over embedded seed | registry.rs | lodash 4.17.23→4.18.1 |
| 11 | OR-range/multi-comparator store matching | registry.rs | 0 misses / 73 specs (was 4) |
| 12 | synth-starvation retry with full packuments | lockfile_write.rs | armed, workspace green |
| 13 | trampoline exec-loop fixes (C fallback, jhol-bin target, size guard) | trampoline.c, main.rs | no 127-loop, chain verified |
| 14 | `install --quiet` lane wall (args.len()!=1 vs sibling whitelist) | trampoline main.rs | 90→41ms, sha256-identical tree |
| 15 | P1 `.jres` write-only→live (bincode×Value class, 2nd instance) | jagr_infinite.rs | hit proven, self-heal proven |

## What was rejected WITH numbers (kept in code, env-gated or documented)

| idea | why rejected |
|---|---|
| hardlink farm materialization | APFS hardlink = **213µs/link** vs clonefile ~10µs/node → 500ms vs 23ms |
| parallel clonefile | no gain — APFS serializes node-clone work |
| zstd vs gzip | 2× slower for 12% smaller |
| bincode packuments | +21× disk, parse gain eaten by write |
| GRM wiring into jagr_infinite | record shape can't serve lockfile; phase unused by normal installs; ceiling ≈0 |
| daemon default-on | A/B 1.0× (30 vs 48ms paper claim not reproduced) |
| JGrid/JAGR-S default | (their own ablation) parity +197MB |
| spectral warm start / effective resistance / heat kernel | 1.0× / 0.09× / 0.01× |
| C-sentinel re-implementation in C | would violate the repo's own mtime-proof soundness rule |
| TCP prewarm | each ping costs more than the handshake (README, re-confirmed) |

## The dormant-but-harmful → dormant-but-documented sweep
- `.jlat` write-only → **fixed** (#4)
- `.jres` write-only → **fixed** (#15)
- GRM `resolution_cache.bin`: written, read only by dead resolver — consistent with rejection
- `zero_copy_metadata` store: proven live in unit test, writers unreachable in current flows, graceful-miss reads — documented, not deleted
- `binary_lockfile`: no external callers — dead, safe
- `cache_packument()` no-op: documented-intentional (byte-LRU already retains)

## Scoreboard (calm-window medians, same-window interleaved, asserted)

| axis | jhol | bun 1.4.2 | verdict |
|---|---:|---:|---|
| cold, true empty (store parity) | 282-546ms | 2.9-4.9s | jhol 6-10× |
| warm fresh project | 31-97ms | 36-103ms | jhol 1.05-1.5× |
| reinstall / noop | 6.4-35ms | 7.9-43ms | jhol 1.2-1.25× |
| PATH entry (this session's install) | 563ms faster than stale binary | — | 9.2× |

## PATH state (what the user's shell uses)
`~/.cargo/bin/jhol` = front(33KB) → `jhol-trampoline` → `jhol-bin`
(old Oct-5 full CLI backed up at `/tmp/bk-cargo-jhol-oct5`; restore: `cp` it back).
Smoke suite green: `--help`, `install`, `install <pkg>`, `doctor`, `why`, `ci`.

## Git state
HEAD `5bfc48db`, main worktree ONLY, nothing committed (standing instruction),
292 files modified/untracked, all docs appended to
`docs/engineering/OPTIMIZATION_TIER1_3_6_8.md` (Appendices 1-21) + this record.

## The three lessons that produced every win
1. **A cache you never measured reading is a lie.** Two write-only bincode×Value
   traps (`.jlat`, `.jres`) and one dead-branded gate (`--quiet` wall) hid in code
   whose comments said they worked.
2. **jhol's own machinery is the fastest non-clone innovation.** L0 bundles,
   capsule, store-metadata unification, trampoline chain — every fix reused
   existing pieces; zero competitor patterns imported.
3. **Physics decides before architecture does.** APFS prices hardlinks 20×
   clonefile; process spawn is a floor; registry TTFB dominates cold. No design
   beats a syscall price.
