# Production Knot L1 wiring (2026-07-16)

## Backup

- Git branch: `backup/pre-knot-l1-2026-07-16` @ `e9f754b0`
- File copy: `experiment/2026-07-16-resolver-bakeoff/backups/jagr-pre-knot-l1/`

## What landed

- `crates/jhol-core/src/jagr/knot.rs` — Knot v4 (MAC + AC-soften + capsules + journal)
- `JagrResolver` Layer 1: **Knot first**, OL3P fallback
- Env: `JHOL_JAGR_L1=auto` (default) | `knot` | `ol3p`

## Tests

- `cargo test -p jhol-core knot` — 2 passed
- `cargo test -p jhol-core --lib` — 302 passed, 1 ignored

## Seat

Knot replaces **only** Layer 1 exact solve inside JAGR3. L0/L2/L3 unchanged.
