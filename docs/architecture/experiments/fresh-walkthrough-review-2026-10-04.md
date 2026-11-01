# Fresh walkthrough review and install check — 2026-10-04

Status: scoped correctness checks pass after repairs; 4× cold gate NOT GREEN.

## Read scope

Read both authored repository walkthroughs in full: `walkthroughtrampoline-jgrid-occam.md`, `walkthroughwith-jagroccam.md`. Read every Jhol-related Antigravity walkthrough: brain IDs `7ad28845-fe9f-4a2f-a690-659aba1b5c4f`, `9a95525f-d8ec-4562-bca2-3aad55ca96f8`, `c8088f63-72d7-4b09-ae5f-539fceaa9dba`. Three matched the Jhol topic among41 Antigravity walkthroughs; unrelated projects excluded. Followed `JAGR_INFINITE_DETERMINISTIC_INNOVATION.md` and `TRAMPOLINE_AND_JGRID_OCCAM_INNOVATION.md`. Earlier118-document review retained in work progress; this pass does not claim re-reading unrelated documents.

## Reproductions and repairs

- Current C front accepted a fresh timestamp with a required locked package absent. C now delegates shared Rust proof;9/9 fake-backend argument/fallback checks pass. Old C source archived.
- JAGR lockfile replay accepted missing/incompatible required children. Replay now checks nearest ancestor, range compatibility, and top-level direct roots. Unit exercises missing, incompatible, valid, optional, nested and nested-only-root cases. Old source archived.
- First fresh medium comparison: five Jhol installs exited0 but violated `debug -> ms2.0.0` or `send -> ms2.1.3`. Their elapsed times are rejected as performance evidence. Raw `fresh-walkthrough-check-20261004-170707.json` retained.
- Persistent reproduction showed correct parent declaration but child metadata covering only one version. `build_node_layout_with` omitted the conflicting edge; its caller checked only emitted edges. Native single-pass now fetches full domains only for children without a satisfying candidate, then checks every declared required edge against actual ancestor paths. Dedicated regression rejects missing and shadowed conflicting children. Medium post-fix logs confirm single-pass layout, no second resolution.
- Quality checker initially timed out on generated alias cycles; bounded realpath traversal corrected it. A subsequent `.bin` error was a checker error: internal dependency aliases do not each require a separate `.bin`. Corrected checker validates each real package once and follows Node realpaths. Original raw checks and standalone metadata counterexample retained.

## Fresh checks

- Core with `http-reqwest`:539 passed,0 failed,3 ignored; two jobs and memory watchdog, peak66%.
- Layout filter:8 passed.
- Final CLI30 passed and JAGR integration10 passed;0 failed, watchdogpeak68%.
- Rust trampoline baseline:6 passed; shared proof source unchanged.
- C delegation:9/9. Final real frontend no-op/local-rematerialization:6/6.
- Safe release build:2m02s, peak68%, LTO disabled/codegen units16 for laptop safety.
- Final fresh install comparison:30/30 scoped checks pass; watchdog peak64%.

## Current registry-cold diagnostic

Five randomized interleaved rounds per arm/workload. Same exact root pins, fresh project/HOME/XDG/tool caches per sample; harvest, daemon, prepared/JGrid/JAGR-S reuse disabled. Native Jhol, scripts disabled for both. OS/CDN caches and unrelated host/network activity uncontrolled. Five rounds are diagnostic, not the required30-round confidence-bound publication gate. No owned build during timing. Numeric speed claims require valid output; timings from failed first-run medium installs remain excluded.

- locked-react-vite: Jhol 759.83ms; Bun 726.62ms; Bun/Jhol 0.956;10/10 installs valid.
- unlocked-small: Jhol 601.57ms; Bun 491.21ms; Bun/Jhol 0.817;10/10 installs valid.
- unlocked-medium: Jhol 1177.21ms; Bun 1694.24ms; Bun/Jhol 1.439;10/10 installs valid.

Checks: exact root pins, every physical package required/peer SemVer edge, Node ancestor resolution, CommonJS entry resolution, ESM root imports, executable bins, unchanged locked input. They do not certify every script, workspace/platform scenario or all blueprint workflows. Medium fixture pins React18.2.0, TypeScript5.7.3 and Express4.21.2. The full raw report embeds fixture manifests, timing observations, environment, outputs, binary hashes, runner and corrected checker source.

Raw: [fresh-walkthrough-check-20261004-172838.json](fresh-walkthrough-check-20261004-172838.json). Counterexample: [fresh-single-pass-counterexample-2026-10-04.json](fresh-single-pass-counterexample-2026-10-04.json).

## Historical claims qualified

`bench_production_head_to_head.py` uses different dependencies for Jhol and Bun and persistent tool caches. Its no-op, cache-prime and project-analysis results cannot establish true registry-cold superiority. A `[JINF] cache_fast` marker can also occur during lock writing; marker presence alone does not prove the full resolver was the cold resolution path. `DefaultHasher::new()` is not evidence of changing per-process identities. Fixed-size hash injectivity, universal zero-backtracking and constant-time directory installation remain unproved. Research papers unchanged.

## Preservation

Dirty main remains uncommitted;63 pre-existing deletions unchanged. No Git writes, pushes, worktrees, global binary replacement or global cache cleanup. Changed-source baselines archived under `archive/experiments/fresh-walkthrough-review-2026-10-04/`; temporary diagnostic metadata logging removed from final source.

Benchmarked repo Jhol SHA256: `27301fa1f29af4fe1603648014e9fdaad9372c9e7f62d21b01fb1b0f9066cf99`. Installed user Jhol remains `8b186ca8a6ca045e82162321fc3cf6e7fe1841aaba2b5268f3c99dc40f79c518`.
