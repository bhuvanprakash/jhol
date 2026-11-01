# Residual-Certified OL3P — packument gate results

**Date:** 2026-07-16  
**Scope:** `experiment/2026-07-16-resolver-bakeoff` only  
**Raw:** `packument-gate-20260716T020828Z.json`

## What this gate is

Real **abbreviated npm packuments** (via `jhol-core` registry) → domain/edge graph → Crystal-CNF → OL3P.

Caps (honest): `--max-packages 40 --max-versions 12 --max-depth 3`  
Not full Express closure; not lockfile-byte parity yet.

## Packument build

| Fixture | fetched pkgs | versions kept | edges | build |
|---------|-------------:|--------------:|------:|------:|
| express-app | 40 | 69 | 73 | 1.06 s |
| overrides-app | 2 | 3 | 2 | 0.29 s |
| peer-deps-app | 5 | 24 | 7 | 0.14 s |

Fetch errors: **0**

## Residual vs full (real domains)

| Fixture | shell | vars full → residual | residual_certified time | ok |
|---------|------:|---------------------:|------------------------:|:--:|
| express-app | 36/41 | **41 → 5** | 2.59 ms | yes |
| overrides-app | 3/3 | 3 → 1 | ~0 ms | yes |
| peer-deps-app | 2/6 | 13 → 11 | 0.43 ms | yes |

**express:** ~88% of packages forced into shell; OL3P residual only **5 vars**. That is the design claim on real metadata.

## vs current JAGR proxies (same fixtures)

| Fixture | dual_proxy (`jhol jagr --run-all-layers`) | pulse_proxy (skip L2/L3) | residual OL3P (capped graph) |
|---------|------------------------------------------:|-------------------------:|-----------------------------:|
| express-app | 9.97 s | 5.59 s | **~0.03–2.6 ms** |
| overrides-app | 0.17 s | 0.15 s | ~0 ms |
| peer-deps-app | 0.52 s | 0.10 s | ~0.4 ms |

Do **not** read residual ms as “beats Bun install.” Different work: capped domain MaxSAT vs full JAGR analysis. Read as: **once domains exist, residual+OL3P is tiny; today’s JAGR spends seconds elsewhere (topology + fat formula).**

## Quality

- Same OL3P engine; smaller CNF. Intended semantic loss: **0** inside the capped model.
- Still missing: full transitive npm closure, peers as first-class, lockfile parity vs `jhol`/`bun`.
- Next: raise caps / add peer edges / compare expanded assignment to `jhol jagr --json` selection.

## Re-run

```sh
cd experiment/2026-07-16-resolver-bakeoff/residual_ol3p
cargo build --release
./target/release/residual-ol3p-experiment \
  --out ../results/packument-gate.json \
  --packument-fixtures ../fixtures \
  --max-packages 40 --max-versions 12 --max-depth 3 \
  --jhol-bin ../../../../target/release/jhol
```
