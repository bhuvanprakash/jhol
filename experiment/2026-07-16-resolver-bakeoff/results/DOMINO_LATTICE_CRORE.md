# DominoLattice + crore-scale JMR (2026-07-16)

## What shipped

1. **Incremental DominoLattice** in `install.rs` — live AND ranges update in O(deg(parent)), not O(|E|) full rebuild each wave.
2. Expand wired to lattice (`seed_root` / `set_parent_edges` / retract / prune).
3. Thread-local **VersionReq cache** in `constraints_satisfied`.
4. JMR `JHOL_MIRROR_MAX_ENTRIES` default **2_000_000** (was 50k).
5. Mirror eviction: **approx oldest-of-sample (64)** — O(sample), not O(|mirror|) min-scan (Domino footgun at crore scale).
6. Unit tests: retract/prune + `domino_incremental_parent_flip_is_local` (50k edges).

## Fair cold bakeoff (express-app, isolated `HOME` + `JHOL_CACHE_DIR`)

| Arm | wall | expand_ms | nm | express |
|-----|-----:|----------:|---:|---------|
| coldA | 1.83s | 1640 | 96 | 4.18.2 |
| coldB | 1.36s | 1189 | 96 | 4.18.2 |
| cold verbose | **1.05s** | 908 | 96 | 4.18.2 |
| capsule hit | 0.24s | **6** | 96 | — |
| bun cold | 2.66s | — | 95 | — |

Quality gate: **nm≈96**, `express@4.18.2`, Domino retract/prune tests pass.

## Honest read

- Capsule/JMR hit path stays ≪100ms expand.
- Fair cold wall **~1.0–1.8s** (network variance); best run **1.05s**. Beats Bun cold on this fixture.
- Remaining wall is mostly npm metadata RTT + H2 packument waves, not Domino CPU.

## Not copied

No competitor lockfile placement maths. Domino live-edge lattice + Corolla helix + GraphCapsule stay Jhol-native.
