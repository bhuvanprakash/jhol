# JAGR Pulse Engine (experiment arm)

**Status:** experiment proxy — not promoted  
**Date:** 2026-07-16  
**Rule:** do not modify original tree; this folder only.

## Idea

Kill cubic on install by policy. **Superseded for quality-preserving path by**
`residual-certified-ol3p-design.md` (keep OL3P + L2/L3 on residual `Q`).

Pulse remains useful as a *skip-topology proxy* in bakeoff harness only.

```text
install Pulse:
  evidence -> Fibers/Crystals (future) -> force-wave -> shell
  -> residual knot K
  -> journal exact OR GRM capsule OR OL3P fallback
  NEVER Nullstellensatz / Betti / Hodge on install hot path

observatory (jhol jagr):
  full Dual-Geometry / layers later; capsules may teach Pulse
```

## Experiment proxy (today)

Dual-Geometry not coded yet. Proxies:

| Arm | Meaning today |
|-----|----------------|
| `dual_proxy` | `jhol jagr --run-all-layers` — pay L3+L2+L1 (topology + OL3P) |
| `pulse_proxy` | `jhol jagr --skip-layer2 --skip-layer3` — skip cubic filters, L1 only |
| `bun` | `bun install` cold |

Also microbench: dense boundary-matrix cubic vs bitset force-wave (Pulse shape).
