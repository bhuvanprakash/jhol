# Brutal bakeoff — JAGR Knot vs OL3P vs jhol proxies vs Bun

**Date:** 2026-07-16  
**Raw:** `brutal-knot-20260716T024119Z.json`  
**Scope:** experiment only  

## Read this first (brutal honesty)

| Timer | What it is | Fair vs Bun? |
|-------|------------|:------------:|
| `jagr_knot` / OL3P arms | Resolve on **in-memory** domains (synthetic or capped packument) | **NO** |
| `dual_proxy` / `pulse_proxy` | `jhol jagr` analysis (layers + OL3P) | **NO** |
| `bun install` | Full product: resolve+fetch+extract+link | product baseline |

Knot **wins the resolve microbench**. Knot does **not** beat Bun install — different race.

---

## 1) Synthetic express-like (hard residual clique)

| Arm | Engine | Time | Notes |
|-----|--------|-----:|-------|
| full_crystal_ol3p | OL3P | 54.0 ms | 163 CNF vars |
| residual_ol3p | OL3P | 40.7 ms | 133 vars |
| residual_certified_ol3p | OL3P | 45.2 ms | 133 vars |
| **jagr_knot** | **Knot** | **0.25 ms** | 58 nodes, **no CNF** |

Knot ≈ **160–200×** faster than OL3P on same graph. All ok=true.

## 2) Scale sweep

| n | full OL3P | residual OL3P | **Knot** |
|---|----------:|--------------:|---------:|
| 20 | 4.65 ms | 1.08 ms | **0.03 ms** |
| 40 | 13.5 ms | 6.49 ms | **0.08 ms** |
| 60 | 25.3 ms | 11.2 ms | **0.33 ms** |
| 80 | 44.3 ms | 15.5 ms | **0.14 ms** |

## 3) Real packument domains (capped 40 pkgs / depth 3)

| Fixture | full OL3P | residual OL3P | **Knot** | ok |
|---------|----------:|--------------:|---------:|:--:|
| express-app | 0.19 ms (41 vars) | 0.02–0.37 ms (5 vars) | **0.06 ms** (34 nodes) | all |
| overrides-app | 0.04 ms | ~0 | **~0** | all |
| peer-deps-app | 0.48 ms (13 vars) | 0.12–0.14 ms (11 vars) | **0.01 ms** | all |

## 4) jhol jagr proxies (seconds — analysis path)

| Fixture | dual (all layers) | pulse (skip L2/L3) |
|---------|------------------:|-------------------:|
| express-app | **8.06 s** | 3.77 s |
| overrides-app | 0.16 s | 0.14 s |
| peer-deps-app | 0.60 s | 0.12 s |

## 5) Bun install (full product)

| Fixture | bun install |
|---------|------------:|
| express-app | **1.94 s** |
| overrides-app | 0.024 s |
| peer-deps-app | 0.20 s |

---

## Verdict

1. **Knot beats OL3P hard** on same domains (esp. synthetic express-like: 54 ms → 0.25 ms).  
2. **Knot does not beat Bun** on install wall-clock — Bun is still the product bar (~2 s Express).  
3. Today’s `jhol jagr` dual (~8 s) loses to Bun (~2 s) and to Knot microbench — different work, but shows Layer+OL3P path is not competitive as a story.  
4. Quality in this gate: all arms `ok=true` on same graphs; Knot also returns prefer-latest-style objective. **Not yet** lockfile-byte parity vs OL3P/npm.  
5. Sure enough to keep developing Knot in experiment; **not** sure enough to claim “beats Bun” or promote to main without install stage-split + parity gates.

## Rust features used in Knot path

- `u64` domains + bit scan  
- CSR + worklist force-wave  
- domain-snapshot backtrack  
- optimistic objective bound prune  
- rayon for parallel fixture arms  
- packument fetch via `jhol-core`
