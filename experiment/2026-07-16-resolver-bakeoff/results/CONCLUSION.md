# Experiment conclusion (2026-07-16)

**Original `crates/` / `src/` not modified.** Work only under `experiment/`.

## Honest labels

| Arm | Reality |
|-----|---------|
| dual_proxy | Your Dual-Geometry **not coded yet**. Proxy = full JAGR L3+L2+L1 (`--run-all-layers`). |
| pulse_proxy | Pulse **not coded yet**. Proxy = skip L2/L3, still OL3P L1. |
| bun | Real Bun install. |

## Wall times (fresh run `bakeoff-20260716T010909Z`)

| Fixture | dual_proxy | pulse_proxy | bun |
|---------|------------:|------------:|----:|
| express-app | 6.225s | 4.018s | 0.331s |
| overrides-app | 0.514s | 0.321s | 0.837s |
| peer-deps-app | 1.200s | 0.752s | 0.201s |

## Read

1. **Pulse proxy beats dual proxy** on all three (skip topology saves ~35% express, ~37% overrides, ~37% peer-deps).
2. **Bun still wins express + peer-deps hard.** Skipping L2/L3 not enough — L1 OL3P remains the hog. Matches your profiler story.
3. **overrides-app:** pulse beats bun (0.32 vs 0.84) — small graph, Bun cold install overhead.
4. **Microbench:** force-wave at 1000 pkgs ~2.8ms; dense elim n=128 ~5.8ms. Topology micro alone not Express killer; full JAGR path (metadata+OL3P) is.

## Verdict for next build

Pulse direction validated as *faster than full-layer JAGR today*. Not yet Bun-class on Express until native force-wave + residual knot **replace** OL3P, not merely skip L2/L3.

Re-run: `BAKEOFF_TIMEOUT_SECS=90 ./experiment/2026-07-16-resolver-bakeoff/harness/run_bakeoff.sh`
