# Production readiness — what is guaranteed, and how to check it

Every claim below has a command beside it. If a command stops producing the stated
result, the claim is false — that is the point of writing it this way.


## Documentation map

| document | contents |
|---|---|
| `RESOLVER-JAGR.md` | the resolver: bitmaps, propagation, search, instances, index, policy, history, obligations |
| `INSTALL-PIPELINE.md` | every stage from argv to tree: freshness authority, lock, lanes, resolution path, store, materialisation, failure modes |
| `CLI-REFERENCE.md` | commands, flags, exit codes, environment |
| `ENGINE-JHOLVM.md` | the JavaScript engine: pipeline, value representation, GC, JIT, modules, formatter, limits |
| `OPERATIONS.md` | cache layout, gates, how to measure, troubleshooting, landmines |
| `DEFECT-LOG.md` | twelve defects with root cause, how each was caught, and the rule it produced |
| `JAGR-INFINITE-IDEAS.md` | the measured idea scorecard (what was tried, kept, and rejected, with numbers) |
| `JAGR-INFINITE-ROADMAP.md` | what remains and in what order |
| `ENGINE-GAPS.md` | the engine's measured limits, each with the mechanism a fix needs |
| `AI-FIRST.md` | models as first-class artifacts: what shipped, what is measured, what remains |
| `ENV-AUDIT.md` | the 180 `JHOL_*` variables classified, and the rule for adding another |

## Guarantees

### 1. The installed tree is what the lockfile authorises

```
jhol verify                 # exit 0 only if the tree matches the lockfile
jhol verify --json          # {"state": ..., "detail": ...}
jhol verify --prune         # remove unauthorised directories, then re-verify
```

A version edited on disk is reported by name and range; a directory the lockfile
never authorised is reported as such; a missing package is reported as missing. A
missing *lockfile* is "not verified" and exits 0, because there is nothing to
disagree with — but it is printed, never silent.

Proven by: `crates/jhol-treecheck` (12 tests) and the manual sequence
tamper → `jhol install` → restored version.

### 2. An install converges

```
jhol install      # first run repairs; subsequent runs print "Up to date"
jhol install      # must be idempotent
```

The failure this replaced: warm runs reporting `Added 1 packages` forever because a
store entry with a 0-byte `package.json` and a completion marker counted as
installed. Fixed in `store_extract_complete` (an existing-but-empty manifest is
corruption; an absent one is governed by the marker).

### 3. Concurrent installs serialise

Two `jhol install` runs in one project do not interleave. `flock` on
`.jhol/install.lock`, released by the kernel on any exit path including
`std::process::exit`. Re-entrant within a process (a lifecycle script that installs
does not deadlock).

Proven by: `crates/jhol-core` `project_lock` tests; manual: two cold installs at
once, one waits, both succeed, tree consistent.

### 4. Resolution is deterministic

Same manifest, same tree — independent of map insertion order, run order, or which
component happens to be resolved first. Solver, chart build, propagation and the
instance resolver are all order-independent by construction (sorted everywhere that
iteration order could leak).

Proven by: `proof_obligations` (soundness over 40 random worlds, determinism under
reordering, monotonicity + idempotence) and `determinism_proofs` (propagation from
forward, reverse and shuffled inputs).

### 5. Multi-major trees resolve

A tree needing three majors of one package resolves, with each instance carrying
its derivation.

```
jhol install jest@29.7.0    # 266 packages, nested majors, stable on warm runs
jhol why ansi-styles        # every installed copy, its version and who demanded it
```

Proven by: `jagr_nest` tests (two majors coexist, compatible ranges share an
instance, unsatisfiable ranges report the chain) and the real `jest` install.

### 6. Failures explain themselves

```
jhol why <pkg>              # why it is here, from the lockfile, offline
jhol why-not <pkg>@<range>  # the minimal set of requirements that forbids it
```

`why-not` answers three situations differently: already installed (with its path),
published but forbidden (minimal core), nothing published in range. The core is
minimal by greedy shrink over set intersections — on a real jest tree `chalk@^3`
is blocked by one dependent, not by the thirty that mention chalk.

### 7. History and policy are inputs, not audits

```
jhol install --as-of 2020-06-01            # express 4.17.1, 50 packages
jhol install --allow-license MIT,ISC       # only versions under those licenses
```

Both are resolution constraints. Both fail loudly rather than falling back to
"today" when the data needed is unavailable: a dated install that cannot honour the
date exits 1 with zero packages installed, and a license policy that cannot see a
license excludes the version (with an explicit `--license-allow-unknown` escape
hatch).

### 8. Ten real manifests pass four dimensions

```
for each of express, vite, esbuild, jest, next, eslint, webpack, prisma,
typescript, rollup:
    jhol install   -> jhol verify (exit 0)  -> install again (Up to date)
                   -> jhol ci (wipe + reinstall)  -> jhol verify (exit 0)
```

All ten pass all four. This is the check that found the platform-optional and
lockfile-integrity defects; run it after any change to resolution, materialisation
or verification.

## Numbers, from the harness (not from memory)

| phase | value |
|---|---|
| chart assembly (275 packuments, 6,715 edges) | 11.4 ms |
| cache view through the binary index (344 packuments) | 15.2 ms |
| metadata prefetch, bounded (400 packuments) | 9.8–12.3 s |
| tarball downloads (266 packages) | 19–54 s (registry-bound, network-variable) |
| materialise an existing tree from the store | 379 ms |
| warm install, 266-package tree | 18 ms |
| dated install (`--as-of`) | 21–24 s |

The tarball phase is not ours to fix by adding parallelism: concurrency is already
`(cpus × 4).clamp(32, 128)` and the HTTP client is a pooled agent.

## Verification gates

`sh scripts/verify-all.sh` runs all of them, plus `--workspace --all-features` and an
optional flake hunt (`ROUNDS=5`). CI now has an `all-features` job; without it, 26
`jhol-core` library tests (747 vs 721) were never executed by anything.

```
sh crates/jhol-core/verify.sh    # installer: build, unit, adversarial, E2E offline, determinism, gc
sh crates/jhol-vm/verify.sh      # engine: corpus, formatter, JIT
cargo test -p jhol-core --lib    # 678 tests, 7 ignored (each carrying its reason)
cargo test -p jhol-treecheck --lib
```

Ignored tests are not skipped work: each names a defect and the fix that would make
it pass (`completeness_against_brute_force` documents the base-misaligned bitmap
intersection and the rework it depends on).

## Lockfile completeness, re-measured

An earlier measurement found `express@latest` installing 68 packages while writing 56
lockfile entries. It no longer reproduces — measured after the resolver fixes, three
manifests:

| manifest | lockfile entries | installed | difference |
|---|---:|---:|---|
| `express@latest` | 70 | 70 | **0** |
| `express@4.18.2` | 71 | 71 | **0** |
| `vite@5.0.0` | 58 | 11 | 47, all `optional: true` platform binaries for other OSes/arches (`@esbuild/aix-ppc64`, …) |

The third row is correct behaviour, not a gap: npm lists optional platform packages in
the lockfile and installs only the applicable one, which is exactly what
`jhol-treecheck::entry_platform_applies` decides on the verify side. Any future
completeness check must apply that rule, or every cross-platform tree looks broken.

Pruning stays opt-in, but its justification narrowed: an incomplete lockfile is no
longer observed, so the remaining reason is packages a developer added by hand.

## Known gaps, stated rather than implied

1. **Payload time.** 19–54 s of a cold install is tarball transfer from the
   registry. Measured as not addressable by more concurrency.
2. ~~**Lockfile can be incomplete**~~ **Not reproducible**: 70/70, 71/71 and a
   platform-optional difference on `vite` — see the section above.
3. ~~**Two clap builders.**~~ **Resolved**: one `cli_command()` serves both parsing
   and completion generation, so a flag cannot exist in one tree and not the other.
   `flag()` still treats an undeclared flag as off, as defence in depth.
3. **`--as-of` no longer revalidates settled history.** It used to pay one
   conditional round trip per package — 63 of them, full packuments, megabytes each —
   to be told that a *past* date's record had not changed. Freshness now has two sound
   reasons to trust the cache for a cutoff:

   * the cached file was **fetched after** the cutoff (mtime), so it necessarily
     contains every release published before it — cheap, no parse, and it covers
     packages that stopped publishing years ago;
   * or its newest release is dated after the cutoff, which proves the same thing.

   | | requests | warm wall |
   |---|---:|---:|
   | before | 63 | 4.1–8.0 s |
   | after | **1** | **1.5–1.9 s** (one 5.05 s outlier) |

   Correctness pinned across dates from one manifest: `^4.0.0` resolves to express
   **4.17.1** as of 2020-06-01, **4.18.2** as of 2023-06-01, **4.19.2** as of
   2024-06-01; 62 lockfile entries, `verify` clean.

   What the cost is *not*, measured rather than assumed: `--lockfile-only` is 0.76 s, and
   materialising the same 62 packages from a known lockfile is **22–41 ms**. So
   resolution and writing are both cheap; the remaining ~1 s is the single unavoidable
   full-packument fetch for a package whose cache predates the cutoff.

   Kept on the record: the parallel *frontier* fetch in the resolve loop was reverted —
   6.8–10.6 s against a 3.7–4.7 s baseline that then re-measured at 4.06 s and 8.00 s,
   so it could not support a conclusion. **Method: interleave, medians of ≥5, report
   the spread.**

4. **Peers warn by default, fail in strict mode.** They were recorded and never
   compared with the chosen versions on the instance-resolved path (`--as-of`, policy
   installs); that check now exists (`peer_violations`). Advisory by default is npm's
   behaviour — binding by default turns working trees into failures — and
   `JHOL_STRICT_PEER_DEPS=1` / `--strict-peer-deps` refuses instead.
5. **Hoisting is a simple rule.** Most-demanded instance wins the top level, ties to
   the highest version. Deterministic, not tuned.
6. **~180 `JHOL_*` knobs.** Classified in `ENV-AUDIT.md`: 24 diagnostics, ~40
   escape hatches guarding a default that was once unsafe, ~20 tuning, ~30
   interface. Nothing is retired yet — the classification is the prerequisite,
   because retiring an escape hatch without fixing the default it guards is how a
   project keeps the unsafe thing and renames it.

## The invariants behind all of it

1. **A fast path may save work; it may not decide truth.** Every lane's success is
   checked against the tree.
2. **No copying.** No clause learning, no nogoods, no backjumping. Monotone bitmap
   narrowing, exact ordering, local repair. Explanations are bitmap intersections.
3. **Determinism is part of the interface**, not a property tests happen to check.
4. **Fail loudly, never silently wrong.** A reproducibility feature that falls back
   to today is worse than one that refuses.
