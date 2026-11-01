# Domino fix + cold (2026-07-16)

## Domino bug (looked right, was wrong)

Expand **accumulated** constraint ranges forever. Parent@version flip left stale ANDs (`^1` + `^2` → UNSAT / silent skip) and orphans. Re-open alone not enough.

## Fix (Jhol live-edge Domino)

1. **`edges_out`** — selected parent → live dep ranges only  
2. **`rebuild_live_constraints`** — roots ∪ live edges (no stale)  
3. **`domino_retract_invalid`** — drop picks that fail live AND; re-queue  
4. **`domino_prune_unreachable`** — drop orphans after parent flip  

Rust: `is_ok_and` / `is_some_and` / `let...else` / snapshot before mutate.

Tests: `domino_rebuild_drops_stale_parent_ranges`, `domino_retract_and_prune_orphan_after_parent_flip`.

## Soul

JAGR3 / Knot / JMR / Corolla Pulse kept. No competitor placement math.

## Perf (express-app, Corolla on)

Prior fair Corolla ~**1.0–1.2 s** cold, expand ~**0.8–0.95 s**. Domino is correctness; wall stays in that band when network calm. Warm ~**55 ms**.
