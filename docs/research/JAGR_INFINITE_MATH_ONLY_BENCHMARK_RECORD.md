# JAGR-Infinite math-only resolver experiment record

## Scope

This is a local, metadata-only comparison of Jhol's default JAGR-Infinite path
against Bun. npm is intentionally out of scope for this record. The experiments
do not download package tarballs, extract packages, or create `node_modules`.
They use an in-process synthetic registry bound to loopback, with generated
packuments and deliberately forbidden tarball URLs. The registry recorded zero
tarball requests in the controlled runs.

Jhol was invoked with `install --lockfile-only --native-only --no-scripts`,
without `--resolver` or `JHOL_RESOLVER`; that exercises the configured default
resolver. Each run used a fresh project and isolated cache/home. Bun was invoked
with `bun install --lockfile-only`, `BUN_INSTALL_CACHE_DIR`, `HOME`, and
`XDG_CACHE_HOME` isolated per run. Timings include process startup and lockfile
writing. They are end-to-end metadata-only times, not pure solver timings.

Recorded environment: macOS, Bun 1.4.2, Jhol release binary from repository
revision `5bfc48db`. No repository source was changed for these experiments.

## Controlled 1,200-package independent fan-out

The project directly depends on 1,200 synthetic packages, each with three
available versions (`1.0.0`, `1.1.0`, `1.2.0`). Root ranges cycle evenly among
`^1.0.0`, `~1.1.0`, and `<1.2.0`. The expected highest satisfying version is
respectively `1.2.0`, `1.1.0`, and `1.1.0`.

Three fresh-cache repetitions per tool:

| Tool | Repetition times (ms) | Median | Packages | Range mismatches |
|---|---:|---:|---:|---:|
| Jhol / JAGR-Infinite default | 308.1, 264.7, 254.9 | 264.7 ms | 1,200 | 0 |
| Bun 1.4.2 | 101.8, 84.3, 76.9 | 84.3 ms | 1,200 | 0 |

Both lockfiles selected the expected version for all 1,200 direct dependencies.
On this synthetic local-registry case, Jhol took about 3.1x Bun's median time.
This is a specific fixture result, not a broad package-manager ranking.

## Jhol packument-concurrency sweep

Same 1,200-package independent fan-out, three fresh project/cache repetitions
per setting. The host's detected default is 40; these runs explicitly tested
16, 32, 40, and 64.

| `JHOL_PACKUMENT_CONCURRENCY` | Repetition times (ms) | Median |
|---:|---:|---:|
| 16 | 263.7, 222.4, 222.2 | 222.4 ms |
| 32 | 225.3, 227.2, 233.6 | 227.2 ms |
| 40 (default) | 229.0, 227.4, 229.5 | 229.0 ms |
| 64 | 232.0, 267.8, 243.7 | 243.7 ms |

This sweep is noisier and separate from the paired comparison above. It does
not support simply raising concurrency: 64 was slower, while 16–40 clustered
closely. Treat 16 as a candidate for a larger, repeated evaluation, not as a
new recommended default.

## Resolver stage evidence on a transitive chain

A separate 1,200-package chain fixture, with mixed semver ranges and generated
transitive edges, was run three times per tool. Median end-to-end times were
947.7 ms for Jhol and 311.3 ms for Bun. This fixture is different from the
independent fan-out above and should not be combined with its numbers.

One Jhol diagnostic run on that chain reported:

| Jhol stage | Time |
|---|---:|
| Fetch and expand 1,200 packuments (`fetch_dep_tree`) | 498.3 ms |
| Build chart and constraints | 7.1 ms |
| JAGR solve plus initial materialization | 5.5 ms |
| Finalization/reconciliation | 14.2 ms |
| Full resolver path | 531.2 ms |

The stage timings are from one diagnostic run, not the three-run end-to-end
median. The diagnostic divides the resolver path; CLI startup and lockfile
writing are outside the reported full-resolver figure.

## What the evidence says so far

The measured bottleneck is not the core JAGR selection computation on these
fixtures. Chart construction, selection, and finalization totalled about 27 ms
in the chain diagnostic, versus about 498 ms for packument fetch/expand. The
independent-fan-out comparison also shows a gap despite trivial graph structure.
The next investigation should therefore measure the metadata path—HTTP request
handling, JSON parse/pruning, per-package worker scheduling, and lockfile output—
before changing resolver mathematics.

In the current implementation, `fetch_dep_tree` uses a scoped thread per
in-flight packument up to the configured concurrency, stores parsed JSON values,
prunes versions, expands newly discovered dependencies, and uses a channel to
collect results. These are code-level candidates for profiling, not proven
individual causes. The concurrency sweep says increasing the worker count alone
does not close the gap.

The synthetic checks establish only expected version selection for a direct
independent fan-out; they do not prove full npm ecosystem compatibility, peer
dependency correctness, optional/platform behavior, or production registry
performance. No claim of a 10x advantage is supported by this experiment.

## Next useful tests

1. Repeat paired Jhol/Bun cold-cache fan-out runs with randomized tool order and
   at least 10 repetitions; report median and p95, and retain per-run outputs.
2. Add low-overhead internal timers for metadata request, body read, JSON parse,
   prune, graph expansion, and lockfile serialization. Confirm instrumentation
   overhead with an uninstrumented control.
3. Run graph-shape pairs at equal package count: deep chain, broad fan-out,
   shared-transitive DAG, and conflicting-range graph. Check each produced
   lockfile against explicit range/edge invariants before comparing speed.
4. Keep any optimization behind correctness tests for peer constraints,
   overrides, aliases, optional dependencies, and cycles. Re-run the same
   lockfile-only fixtures; do not use downloaded package contents as a shortcut
   or weaken resolution semantics.

## Reproduction notes

The synthetic registry used for this run was a temporary local process on
`127.0.0.1:60029`; it is not part of the repository. Its endpoint was stopped
after testing. The fixture package names were `mathfan-0000` through
`mathfan-1199` and `mathchain-0000` through `mathchain-1199`.

This file is an experiment record, not a benchmark guarantee. Preserve raw
repetition values and fixture definitions when adding later runs, and clearly
mark changes in binary revision, Bun version, host, cache state, and graph shape.

## 2026-09-30 scheduler hypothesis A/B

### Hypothesis

`fetch_dep_tree` used `std::thread::scope` to create one OS thread for each
in-flight package fetch, then replaced workers as results arrived. The proposed
change was to create a fixed Rayon pool once and reuse its workers. This was a
small scheduling-only experiment: same worker limit, requests, parsing, pruning,
graph expansion, and resolver math.

### Result: rejected

An initial, non-interleaved run suggested a regression. I then built both
variants and alternated them over seven fresh-cache runs each on the same
1,200-package `mathchain-0000` graph. Each run returned 1,200 package entries,
made 1,202 metadata requests, downloaded zero tarballs, and passed the generated
dependency-edge range check (0 violations).

| Scheduler | Seven times (ms) | Median | Range |
|---|---:|---:|---:|
| Original scoped-thread scheduler | 707.7, 284.5, 281.2, 293.9, 309.9, 463.4, 386.6 | 309.9 ms | 281.2–707.7 ms |
| Fixed Rayon pool prototype | 410.8, 312.8, 275.9, 292.2, 340.3, 352.3, 362.8 | 340.3 ms | 275.9–410.8 ms |

The high run-to-run variance and worse prototype median do not support keeping
the pool. The source experiment was reverted; it is not part of the active
resolver. The result does not prove that thread creation is free, only that
this pool design did not produce a reliable end-to-end win here.

### Concurrency sweep

Three fresh-cache runs per setting on that same chain fixture, using the
original scheduler:

| `JHOL_PACKUMENT_CONCURRENCY` | Times (ms) | Median |
|---:|---:|---:|
| 8 | 313.3, 287.3, 274.1 | 287.3 ms |
| 16 | 282.2, 278.6, 286.1 | 282.2 ms |
| 24 | 273.5, 305.5, 307.3 | 305.5 ms |
| 32 | 282.7, 325.7, 285.1 | 285.1 ms |
| 40 (default) | 332.9, 291.2, 288.0 | 291.2 ms |
| 64 | 330.6, 286.6, 285.1 | 286.6 ms |

These medians are close relative to the observed noise. Do not change the
default concurrency based on this sweep.

### Re-profiled resolver stages

Three diagnostic runs on the recreated local synthetic chain registry returned
these stage ranges:

| Stage | Observed range |
|---|---:|
| Fetch and expand 1,200 packuments | 215.5–234.8 ms |
| Chart build and constraints | 2.6–3.9 ms |
| Topological preprocessing | 2.2–2.3 ms |
| JAGR solve plus initial materialization | 2.3–2.5 ms |
| Finalization | 6.1–6.7 ms |

The recreated registry fixture is not byte-for-byte identical to the earlier
fixture that measured 498.3 ms for `fetch_dep_tree`; these numbers are a new
measurement, not evidence that the earlier run became faster. The repeated
result across both fixture generations is that the fetch/expand stage dwarfs
the math stages.

### Next optimization candidate

Do not optimize JAGR selection math for this gap. First split the metadata stage
into request wait/body read, JSON parse, pruning, and frontier scheduling. If
parse/prune is material, prototype a Jhol-owned compact JAGR metadata projection:
retain every version and all dependency, peer, optional/platform, dist, and
integrity facts needed for a correct lockfile, but avoid constructing and
retaining irrelevant JSON fields. This is a hypothesis, not an implemented or
measured win. If request wait dominates instead, optimize Jhol's own connection
reuse/multiplexing and cache path; a local mathematical model cannot remove the
need to obtain metadata for previously unseen packages from a remote registry.

The current stage split implies that even making all measured JAGR
chart/topology/solve/finalization work free would save only about 14 ms out of
roughly 239 ms of resolver time on this fixture (~6%). Reaching Bun's roughly
96 ms end-to-end median would require reducing the metadata stage by about 130
ms, not a solver-math rewrite. This is an Amdahl-style bound for this fixture,
not a performance promise.
