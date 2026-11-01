# Metadata coordinator stall — 2026-10-04

Status: reproduced scheduling defect repaired; general registry-cold gain unproved; 4× NOT GREEN.

## One defect, existing machinery

`expand_with_transitive_dependencies` fetched an expanded version domain on its
coordinator thread whenever later constraints invalidated a single-version
manifest. Independent discovery results waited behind that network request.
Two separate branches implemented this escalation. Received metadata also
created two independent deep JSON allocations for the local and shared caches.

Use one escalation branch on the existing Rayon scope/completion channel.
Existing pending-name set prevents duplicate in-flight jobs. Panic completion
and required-fetch failures propagate through the channel. Both caches now
share one immutable `Arc<Value>` for received and bundled metadata. No new
runtime module, dependency, resolver algorithm, SemVer shortcut or integrity
exception. Source baseline archived before edits.

## Causal reproduction

Six independent diamonds each require child versions 1 and 2. Fresh isolated
caches; controlled loopback delays; 32 workers explicitly selected. Plain and
native installs both make exactly six full-domain requests and all twelve
parent consumers resolve their correct child version.

- Before: request arrival span 923.43/922.64 ms; total 1825.83/1749.61 ms.
- After: span 58.01/76.32 ms; total 1148.05/953.11 ms.

Sequential local diagnostic, not registry-cold or a universal speed claim.
Earlier post-change timings overlapped a core build and remain excluded from
speed evidence. Clean observations: [wave fixture](metadata-coordinator-wave-20261004.json).
Required full-domain HTTP failures return nonzero with no success marker in
plain/native installs: [failure fixture](metadata-coordinator-failure-20261004.json).

Runnable checks:

```sh
CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- python3 scripts/bench/check_cold_request_budget.py target/release/jhol --resolution-mode wave
CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- python3 scripts/bench/check_cold_request_budget.py target/release/jhol --resolution-mode wave-error
```

## Registry cold acceptance

Exact roots react18.2.0, typescript5.7.3, express4.21.2. Thirty accepted randomized
interleaved triplets, each with fresh project/HOME/XDG/tool caches and disabled
harvest, daemon, prepared-tree, JGrid/JAGR-S and resolution-mirror reuse.
Native Jhol and Bun both skip scripts. OS/CDN and unrelated load uncontrolled.

- Saved Jhol: median1590.69 ms, p95 2967.81 ms.
- Fixed Jhol: median1583.50 ms, p95 2870.83 ms.
- Bun1.4.2: median1545.30 ms, p95 1924.82 ms.
- Before/fixed median ratio1.0045; paired bootstrap95%CI[0.9775,1.1013].
- Bun/fixed ratio0.9759; paired bootstrap95%CI[0.9155,1.5581].

No established registry-cold speed improvement. Median peak RSS66,101,248→
63,463,424 bytes is a point estimate, not an allocation isolation experiment.
Metadata/discovery stage median1391.5→1351.5 ms remains dominant; stage contains
concurrent archive prefetch too, so cannot attribute it entirely to metadata RTT.

All90 accepted installs pass actual Node realpath-aware required/peer SemVer,
CommonJS/ESM and executable checks. All93 observed installs pass. Initial round1
was overlapped by owned local correctness processes; retained raw, excluded all
three timing observations. One clean round31 supplied replacement triplet.
Lesson: complete correctness/build jobs before launch; owned jobs invalidate a
whole overlapping triplet. No favourable-round selection.

Raw samples, exact commands, environment, binary hashes, embedded runner/checker,
stages, RSS, retained exclusions and10000 paired bootstrap draws:
[accepted report](cold-metadata-coordinator-accepted-20261004.json).

## Verification and preservation

Core with HTTP feature544 passed,0 failed,3 ignored; CLI30/30. Safe release build
2m02s, peak69%, two jobs, LTO disabled,16 codegen units. Local SRI/bin, nested
consumers, cache reuse, missing-domain and explicit-symlink cases12/12; clean
wave4/4 and failure2/2. Blueprint `verify.sh` passed against an owned temporary
copy with final release: policy, offline JS/Python/Rust receipts, lock
invalidation, snapshot, database plan, model verification/inference, and dev
spawn; watchdog peak58%. No live example or global cache cleanup.

Candidate SHA256 `1195cb122cebb2dba027c0f55126abb34c14aeefe6972c6d86a03e81bb39bd17`.
Saved SHA256 `7e9cf4b558432bba027affa43ebee41155ea19ba0a22a752d3168db0b8eb5ffd`.
Source changed only formatting after candidate build; binary logic matches.

Both repository walkthroughs and all three Jhol Antigravity walkthroughs reread.
Prior117 full doc reviews hash-reverified unchanged; authored deltas reviewed.
Latest c808 Antigravity SQLite opened read-only; all6512 step payloads extracted
(38,205,419 bytes). This is traversal, not complete reasoning review of its21MB
tool-result corpus. Latest task/plan/walkthrough fully read; session claims stay
qualified. Three paper hashes unchanged. The fix executes Jhol's existing
bounded discovery design; does not validate universal zero-backtracking,
topological correctness, complexity or4× claims.

Dirty main,63 existing deletions, zero staged files and installed binary retained.
[Preservation hashes](metadata-coordinator-preservation-20261004.json).
Workspace-wide, cross-platform and remaining paper-algorithm gates remain open.
Retain this as a reproduced coordinator-stall repair, not as a claimed general
cold-speed breakthrough. Do not repeat this experiment as an assumed4× route.
