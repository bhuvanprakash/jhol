# Graph capsule + Domino JMR (2026-07-16)

## Expand to ~100ms (hit path)

**Graph capsule** (`~/.jhol/graph_capsules/<xxh3>.json`): selection + live edges + tarball URLs.
Domino-validated on load. Expand metadata **2 ms** on hit (goal was 100 ms).

Corolla join happens **after** expand mark — expand stage stays metadata-only.

## Domino fixes this pass

1. JMR `predict_full_install` — AND of all live ranges (not first-wins); retract + prune orphans
2. JMR install gate — `coverage >= 1.0` (was `> 0.5` footgun)
3. Corolla skips CAS hits on capsule/JMR rehit

## Numbers (express-app, isolated HOME)

| Path | expand | wall | nm |
|------|-------:|-----:|--:|
| capsule rehit | **2 ms** | **0.155 s** | 96 |
| cold | 0.9–1.7 s | 1.2–2.2 s | 96 |
| Bun cold | — | ~2.8 s | 95 |

Honest: first-ever empty-cache expand still network-bound. Capsule/JMR is how Jhol hits ≪100 ms expand without copying competitor lockfile tricks.

Soul: JAGR3 / Knot / JMR / Corolla / Domino / capsule.
