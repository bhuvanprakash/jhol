# Corolla Pulse + H2 abbreviated expand (2026-07-16)

## Soul kept

JAGR3 / Knot L1 / JMR / CAS. No competitor math copy. Corolla = Jhol expand∥download helix.

## Bugs / wrong-but-looked-right fixed this pass

1. **Corolla wrote `.tgz` but never `store_index`** — classify missed cache; fixed flush on `finish()`.
2. **Corolla unpacked inside expand** — CPU serialized into expand wall; now **download-only**, unpack on cache-install workers.
3. **Per-download `read_store_index()` under 64 workers** — disk thrash; `download_tarball_to_store_hash_only_fast` skips index read.
4. **Abbreviated packuments via rayon+blocking** — cold expand network floor; now **HTTP/2 `buffer_unordered`** with same DashMap/lattice/ETag layers.

## Fair cold express-app (isolated HOME, 3× each)

| Arm | Walls (s) | expand_ms | nm |
|-----|-----------|-----------|-----|
| jhol Corolla | **1.159 / 1.015 / 1.000** | 946 / 826 / 823 | 97 |
| jhol no-Corolla | 1.630 / 1.805 / 1.449 | 1077 / 1060 / 832 | 98 |
| Bun | 3.556 / 3.493 / 3.526 | — | 96 |

Warm jhol (reuse Corolla cache): **~72 ms**.

Express pin: **4.18.2** (exact).

## Trajectory

| Era | expand | cold wall |
|-----|-------:|----------:|
| Broken / sequential micro | ~21000 ms | invalid nm |
| Wave + abbr | ~3300 ms | ~4 s |
| **H2 abbr + Corolla download-only** | **~820–950 ms** | **~1.0–1.2 s** |

## Knobs

- `JHOL_COROLLA=0` — disable helix (expand then cold download).
- Default Corolla on.

Workdir sample: `work/fair-20260716T100355Z`.
