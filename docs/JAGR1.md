# JAGR-1 Resolver Design (Jhol Adaptive Graph Resolver v1)

JAGR-1 is Jhol’s native next-gen resolver strategy.

Tagline: **Exact by math, fast by design, reliable by proof traces.**

## Goals

- **Correctness first**: satisfy dependency + peer constraints as a single solved system.
- **Determinism**: same input graph should yield stable output ordering and lockfile decisions.
- **Speed**: reduce repeated search work using conflict learning and unsat memoization.
- **Reliability**: fail with clear conflict diagnostics instead of hidden divergence.

## Core ideas

### 1) Exact SAT-style solving

JAGR-1 models version selection as constraints over package/version domains:

- root requirements become mandatory constraints
- dependency edges add transitive mandatory constraints
- peer dependencies become global compatibility constraints
- optional peers are soft constraints (non-blocking when unavailable)

The current implementation uses deterministic backtracking with propagation and pruning,
structured so it can evolve into full CDCL internals.

### 2) Speed mechanisms

- **Branch pruning (`learned_forbid`)**: assignments that already led to unsat are skipped quickly.
- **Unsat memoization (`unsat_cache`)**: known-unsat state signatures are cached and reused.
- **Deterministic variable ordering**: stable key ordering improves reproducibility and cache hit quality.
- **Domain capping**: candidate versions are bounded during graph expansion for practical runtime control.

### 3) Reliability mechanisms

- deterministic traversal and signature building
- explicit unsat error surfaces
- solver instrumentation (`SolveStats`) for observability:
  - nodes visited
  - unsat cache hits
  - learned-forbid hits

## Integration in jhol-core

- Entry path: `crates/jhol-core/src/lockfile_write.rs`
- Default strategy: `resolve_full_tree` → JAGR-1 first
- Safety fallback: legacy greedy resolver still exists and auto-fallbacks on JAGR failure
- Override for debugging:

```bash
JHOL_RESOLVER=legacy jhol install --lockfile-only
```

## Current status

Implemented:

- SAT-style exact solver prototype (`sat_resolver.rs`)
- deterministic behavior tests
- search stats + caching/learning primitives
- lockfile resolver integration (JAGR default, legacy fallback)

Planned next:

- watched literal unit propagation
- deeper conflict analysis + minimal unsat core extraction
- incremental solve reuse from lockfile deltas
- optional tree-decomposition fast path for separable graphs

## Validation strategy

1. unit tests for SAT/UNSAT/peer/optional-peer + determinism + stats
2. resolver fixture parity report
3. workspace/CLI smoke tests
4. benchmark comparison against npm/yarn/pnpm/bun (where installed)
