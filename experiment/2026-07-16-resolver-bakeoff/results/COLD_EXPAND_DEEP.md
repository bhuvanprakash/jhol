# Deep cold expand opt (2026-07-16)

## Soul kept

JAGR3 layers, Knot L1, JMR mirror → abbreviated → micro, CAS/link, full tree.

## Bugs fixed (looked right, were wrong)

1. **Bare semver as caret** — Rust `semver` treats `4.18.2` as `^4.18.2`; npm exact. Now `=4.18.2`.
2. **Silent AND drop** — failed multi-constraint used to skip package; now resolve with **all** constraints via `resolve_version_for_all_constraints`.
3. **No re-open** — tighter later range left stale pick; now invalidate + re-queue.
4. **Empty-deps index poison** — `resolve_tarball_via_manifest` cached synthetic packument with `{}` deps; now copies real deps from manifest.
5. **Sequential micro fallback** — abbreviated miss did sync micro one-by-one (~21s); now batched H2/`parallel_fetch_version_metadata_h2`.

## Expand engine

`parallel_wave_expand_resolve`: mirror → exact micro → rayon abbreviated (ETag/DashMap/lattice) → AND constraints → abbr-only deps when present.

Tried streaming frontier + unbounded async join_all — **worse** (mutex thrash / connection melt). Reverted; wave-parallel wins.

## Numbers (express-app, empty cache, nm=96)

| Stage | Was | Best now |
|-------|----:|---------:|
| expand | ~21000 ms | **~3360–3440 ms** (network variance to ~7s) |
| download+link | ~4500 ms | **~800–2000 ms** |

~2s expand: network floor for ~90 packument/micro round-trips on this link; next wins = warmer JMR/daemon cache, not more CPU cleverness.
