# Deep paper/runtime review — 2026-10-04

Status: scoped correctness repairs verified; 4× registry-cold gate remains NOT GREEN.

## Read scope

Re-read all three research papers completely: GRM/Instant Lock, JAGR-S Storage,
and Holographic Dependency Resolution. Followed `JAGR2_SEMANTICS.md`,
`LOCKFILE_DESIGN.md`, and `BOTTLENECKS_AND_PRODUCTION_READINESS.md`.
Earlier full walkthrough/Antigravity review remains in the preceding evidence;
this pass does not claim a second full Antigravity reread.

## Reproductions

- JAGR legality and peer checks searched only the immediate parent and root.
  Two new intermediate-ancestor regressions failed before the fix, then passed.
  A nearer incompatible instance cannot be bypassed by a compatible root copy.
- Required child1 unavailable while another parent needs child2: both plain and
  native release installs returned0, wrote success markers, and omitted children.
  Fixed release returns1 and writes no success marker in both cases.
- First valid nested install succeeded; second fresh project sharing its cache
  returned0 but Node consumers failed, in both plain and native modes. Outer
  expansion cache persisted conflicting flat selections without conflict state.
  Fixed release passes both cold and fresh-project reuse consumers.
- Nested clone install performed one unnecessary store relink and created aliases.
  Fixed release performs zero relinks; explicit symlink mode performs one and
  remains runnable. This is a work-counter result, not a speedup percentage.
- Lock replay reconstructed all peers as non-optional. Preserve original peer
  metadata; regression verifies absent optional peers produce no conflict.

Raw observations, old/new binary hashes and exact fixture source:
[deep-layout-regressions-2026-10-04.json](deep-layout-regressions-2026-10-04.json).
Local HTTP fixture timings are not registry-cold speed evidence.

## Changes

One canonical ancestor lookup now serves native layout validation, JAGR legality,
peer checks and lock replay. It operates on logical slash-separated node keys,
including scoped names, without host-platform path separators.

Native expansion uses the existing inner cache; outer cache read/write routing
removed. Required child absence is a flat conflict. Required expansion and nested
resolution errors propagate; a known conflict above the nested-analysis budget
fails with an actionable error instead of installing an incompatible flat tree.
Optional dependencies retain their separate skip policy.

Expansion cache generation3 includes effective default registry and strict-peer
policy. Layout capsule generation3 and shared completion proof identityv3 reject
older results as misses; existing cache files remain. Complete registry/auth/
scoped-policy cache coverage still needs its broader gate.

## Verification

- Core with `http-reqwest`:544 passed,0 failed,3 ignored; final watchdog peak69%.
- CLI:30/30 passed; watchdog peak64%.
- Rust front proof:6/6 passed; watchdog peak59%.
- Tampered capsule integration:1/1 passed; watchdog peak69%.
- Release local fixtures:14/14 pass across plain/native, cold/reuse, clones/
  explicit symlinks, missing versions, budget failure, SRI, consumer and bin checks.
- Safe Jhol+trampoline release:3m08s, watchdog peak69%, two jobs, LTO disabled,
  codegen units16. Jhol SHA256
  `7e9cf4b558432bba027affa43ebee41155ea19ba0a22a752d3168db0b8eb5ffd`.

Initial stricter CLI gate failed two optional-dependency fixtures because the
old dropped-root check relied on suppressing nested resolution errors. Corrected
that check to exclude deliberately skipped optional roots, then reran all30.
Failures retained as learning evidence, not hidden by restoring false success.

## Fresh registry-cold diagnostic

Three randomized interleaved rounds per arm/workload; identical exact root pins,
fresh projects and isolated HOME/XDG/tool caches, all prepared/harvest/daemon/store
reuse disabled. No owned build during measurements. Same prior realpath-aware
checker verifies required/peer SemVer edges, root CommonJS/ESM resolution, bins,
and locked input stability. OS/CDN caches and host/network activity uncontrolled.
This diagnostic lacks the30-round confidence-bound acceptance gate.

- locked-react-vite: Jhol876.46ms, Bun680.61ms; Bun/Jhol0.777.
- unlocked-small: Jhol1053.41ms, Bun829.43ms; Bun/Jhol0.787.
- unlocked-medium: Jhol1719.38ms, Bun1510.88ms; Bun/Jhol0.879.

18/18 installs and6/6 front-door reuse checks valid. No speed win in this
three-round check. Do not infer regression size from unmatched prior diagnostics.
Raw: [deep-layout-check-20261004-182011.json](deep-layout-check-20261004-182011.json).

## Research qualification

The papers' exact-instance and required-edge obligations guide these checks.
Topology remains a scheduling/analysis input, not permission to drop SemVer
constraints. Fixed-size hashing does not prove injectivity; zero backtracking,
universal complexity and cold speed claims remain unsupported. The Holographic
paper's uncompressed-file SRI description is not the npm tarball verification
contract: actual registry integrity verifies compressed tarball bytes. Research
papers remain unchanged; qualifications stay in this ledger and engineering log.

No new research algorithm is enabled by this patch. Existing streaming remains
opt-in. These scoped checks do not certify all scripts, workspaces, platforms,
blueprint workflows, experimental routes, or every claim in all three papers.

## Preservation

Dirty main preserved;63 pre-existing deleted paths unchanged and nothing staged.
No Git writes or installed-binary replacement. Source snapshots archived under
`archive/experiments/deep-layout-review-2026-10-04/`. Reclaimed only43,094 generated
Rust debug object files to make local checks fit on disk. Global caches untouched.
All three paper hashes match the earlier baseline.
