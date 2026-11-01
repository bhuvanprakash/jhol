# Cold install optimization (2026-07-16)

## What changed (no simplification — Jhol soul kept)

1. **Wave-parallel transitive expand** — BFS waves use JMR `parallel_abbreviated_first_resolve` + exact-pin micro-fetch (not serial full packuments).
2. **Abbreviated-skip-micro** — when abbreviated packument already has `dependencies`, skip the second HTTP round-trip.
3. **In-worker link** — download/unpack/link/bin overlap in the worker pool.
4. **npm exact pins** — bare `4.18.2` is `=4.18.2` (Rust semver caret was wrongly accepting `4.22.2`).

Knot L1 + full tree + CAS/JMR path unchanged.

## Profile (cold express-app, empty cache)

| Stage | Before | After |
|-------|-------:|------:|
| expand / graph | ~21045 ms | **~2947 ms** |
| download+link | ~4527 ms | **~1801 ms** |
| total | ~25 s (bad network sample) / ~4.0 s bakeoff | **~4.8 s profile / ~4.0 s bakeoff** |

## Bakeoff vs Bun (isolated caches, cleaned)

| Run | Seconds | nm |
|-----|--------:|---:|
| jhol cold | **3.99 / 4.17** | 96 |
| bun cold | 5.10 / 5.46 | 95 |
| jhol warm | 0.074 / 0.079 | 96 |
| bun warm | 0.046 / 0.050 | 95 |

**Cold: jhol ahead of Bun on this fixture.** Warm: Bun still slightly ahead.
