# Full package manager: jhol vs bun (cold/warm)

**Date:** 2026-07-16  
**Raw:** `pm-vs-bun-full-20260716T045934Z.json`  
**Fixture:** `fixtures/express-app` (`express@4.18.2`, `nodemon@3.0.2`)  
**Isolation:** per-run temp `HOME` + `JHOL_CACHE_DIR` / Bun cache; cleaned after  
**Binary:** `target/release/jhol` 1.0.2 with Knot L1 + install fixes  

## Knot status

**Yes — Knot is fully wired in production JAGR3 Layer 1** (`crates/jhol-core/src/jagr/knot.rs`).  
Env: `JHOL_JAGR_L1=auto|knot|ol3p` (default auto: Knot then OL3P).  
This bakeoff times **full `jhol install`**, not Layer-1-only microbench.

## Install fixes included

1. Link cold downloads into `node_modules` (was missing → empty tree).  
2. Transitive BFS walks `versions[ver].dependencies` (was reading empty packument-root deps).  

## Results (2 reps, express-app)

| Run | Seconds | nm_top | express |
|-----|--------:|-------:|:-------:|
| jhol cold | 4.024 / 4.022 | 96 | yes |
| jhol warm | 0.094 / 0.104 | 96 | yes |
| bun cold | 3.436 / 2.670 | 95 | yes |
| bun warm | 0.046 / 0.044 | 95 | yes |

## Read

- Trees are now comparable (~95–96 top-level packages). Earlier “jhol wins” numbers were invalid (empty/partial `node_modules`).  
- **Bun still faster** on this cold Express install (~2.7–3.4s vs ~4.0s) and slightly faster warm.  
- Warm jhol (~100ms) is competitive but not ahead of Bun (~45ms) here.  
