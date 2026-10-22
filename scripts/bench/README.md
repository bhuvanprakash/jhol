# Bench suite

Python benchmarks live under `scripts/bench/`. Root-level names under `scripts/` are **compatibility shims** that forward to the merged modules (same CLI as before).

## Shell

```bash
bash scripts/bench/run.sh list
bash scripts/bench/run.sh vs-bun
bash scripts/bench/run.sh brutal
bash scripts/bench/run.sh jagr
```

## Paper-claim matrix (`paper_matrix.py`)

Fail-closed, interleaved Jhol/Bun matrix. Every sample must install, pass CJS+ESM
imports, satisfy the reachable dependency closure, produce runnable `.bin` links,
and **observe the route it was asked to take** — otherwise the sample is
`invalid`, not a timing.

```bash
python3 scripts/bench/paper_matrix.py --fixture DIR --rounds 30 \
  --jhol target/release/jhol --out results.json
```

| Flag | Meaning |
|------|---------|
| `--condition registry-cold` | empty caches, `JHOL_NO_HARVEST=1` — the honest network race |
| `--condition local-cache-cold` | empty project cache, seeded `--seed-cache ARM=DIR`; the harvest/durable-origin win |
| `--condition prepared-tree` / `unchanged-noop` | Instant-Lock / no-op strata, reported as their own tables |
| `--arms` | defaults to `harvest,bun`. `jgrid`, `jagrs`, `jgrid+jagrs`, `combined` are opt-in because their routes do not fire today |
| `--drift-policy warn\|fail` | `warn` (default): a valid tree with a different **in-range** resolution records `resolver_drift` instead of failing |

**Keep the three cold conditions as separate tables.** They are different
products and must never be averaged: registry-cold is a network race,
local-cache-cold measures reuse of bytes already on the machine, and
prepared-tree/no-op measure the Instant-Lock front door.

**Fixture requirement:** the lockfile must be internally consistent. A lockfile
that lists `node_modules/ansi-regex` while `strip-ansi` carries no dependency
edge makes `strip-ansi` unresolvable at runtime; bun prunes the unreachable
entry and breaks, while jhol keeps it and passes — which hides a broken fixture.
Regenerate fixtures with `npm install --package-lock-only`.

## Python (`scripts/bench/`)

| File | Modes | Was |
|------|-------|-----|
| `versus.py` | fair \| clean \| truth \| quick \| brutal | five former `versus_*.py` suites |
| `scientific.py` | suite \| versus \| v2 | `scientific_bench.py`, `scientific_jhol_vs_bun.py`; `v2` → `scientific_v2.py` |
| `scientific_v2.py` | (standalone args) | `benchmark_scientific_v2.py` |
| `guardrails.py` | regression \| collect \| guardrails \| fallback-trend \| clean-artifacts \| dashboard | CI guardrail scripts + dashboard |
| `live.py` | all \| bun | `live_bench.py`, `live_bench_jhol_vs_bun.py` |
| `legacy_compare.py` | jhol-vs-bun \| brutal-cold-warm \| measure-cold | legacy comparison scripts |
| `offline.py` | (standalone args) | `benchmark_offline.py` (also `python3 scripts/benchmark.py offline …`) |
| `scales.py` | (standalone args) | `bench_scales.py` |
| `daemon.py` | (standalone args) | `bench_daemon.py` |
| `atelier.py` | pro | `professional_bench.py` |
| `jagr_duel.py` | compare, simple | `benchmark_jagr_comparison.py`, `simple_jagr_benchmark.py` |
| `workload.py` | large, repeat | `benchmark_large_workload.py`, `benchmark_repeat.py` |
| `profiler.py` | deep, cold | `deep_profiler.py`, `profile_cold_install.py` |

## Python (`scripts/` merged at repo root)

| File | Modes | Was |
|------|-------|-----|
| `reports.py` | enterprise-npmrc \| framework-compat \| resolver-fixtures | three `*_report.py` scripts |
| `binary_packages.py` | build \| bundle \| add-from-lock \| expand-set \| update-profiles \| precompute-unpacked | six binary-cache scripts |
| `benchmark.py` | default CLI + `offline` subcommand | `benchmark.py` + offline suite |

Examples:

```bash
python3 scripts/bench/scientific.py versus
python3 scripts/bench/scientific.py v2 --suite small --repeats 3
python3 scripts/bench/guardrails.py regression --baseline benchmarks/baseline.json --results out.json
python3 scripts/bench/live.py all
python3 scripts/reports.py resolver-fixtures
python3 scripts/binary_packages.py build
python3 scripts/benchmark.py offline --suite small
```

Regenerate merged guardrails/live/reports/binary/legacy from sources (after editing shim targets’ canonical copies in `bench/` or restoring full sources):

```bash
python3 scripts/bench/_merge_generate.py
```
