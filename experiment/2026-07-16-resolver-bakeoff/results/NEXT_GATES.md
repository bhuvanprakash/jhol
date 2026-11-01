# Next gates — Knot v4 (capsules + scale)

**Raw:** `knot-v4-20260716T040409Z.json`  
**Date:** 2026-07-16  

## Parity

**Strict 9/9** (both feasible + obj match + metamorphic) — held from v3.

Including **hard:express** obj **139**.

## Capsule scale (many-user stand-in)

| Metric | Value |
|--------|-------|
| Unique manifests | 9 |
| Warm requests | 100_000 |
| Hit rate | **1.0** |
| Mean cold | ~0.038 ms |
| Mean warm | ~0.0013 ms |
| Speedup | **~30×** |

Maps to shared resolution capsules / GRM under ~10 lakh users — **not** tarball disk cache.

## 4-layer + Knot?

**Helps** if L1=Knot on install, L2/L3 observatory only, CAS/GRM absorb repeats.  
**Hurts** if all layers tax every install.

See `plans/knot-v4-million-user-scale.md`.

## Still before promote

1. Lockfile/selection vs production jhol  
2. Install stage-split vs Bun/npm  
3. On-disk / shared capsules (not just process HashMap)  
