# Spark + sub-900 cold (2026-07-16)

## Wins this pass

1. **HTTP/2 dual prewarm** (blocking + async) at install start
2. **Spark ignition** — exact root pins Corolla at t=0 via npm URL pattern; SRI upgrade re-queue
3. **Corolla unpack during expand** — finish after expand mark (expand_ms stays metadata)
4. **Domino** — clear tarballs on retract; capsule only live pins
5. **Flat specs Corolla seed** via pattern URLs
6. Packument concurrency default **64** (max 128)
7. Async capsule/flat-spec flush (no wall block on JSON write)
8. Ghost prefetch opt-in `JHOL_GHOST_PREFETCH=1` (off by default)

## Numbers (express-app, isolated HOME)

| Path | wall | expand | nm |
|------|-----:|-------:|--:|
| cold best observed | **~0.93 s** | ~812 ms | 96 |
| cold typical | ~1.0–1.5 s | ~0.9–1.3 s | 96 |
| capsule rehit | **~0.09–0.15 s** | **2–3 ms** | 96 |
| Bun cold | ~2.1–3.5 s | — | 95 |

Expand hit path **much under 100 ms** (2–3 ms). Cold wall sits on the **~900 ms line** on good CDN samples; network variance still dominates first-ever empty cache.

Soul: Spark / Corolla / Domino / capsule / JMR / Knot — unique Jhol.
