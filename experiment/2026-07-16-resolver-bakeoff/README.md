# 2026-07-16 resolver bakeoff

Isolated experiment. **Original tree untouched.**

## Arms

| Arm | What runs | Maps to |
|-----|-----------|---------|
| `dual_proxy` | `jhol jagr --run-all-layers` | Dual-Geometry *direction* (L2/L3+OL3P; Loom not coded) |
| `pulse_proxy` | `jhol jagr --skip-layer2 --skip-layer3` | Pulse (skip topology) |
| `bun` | `bun install` | Bun |

**Preferred design:** `plans/jagr-knot-ol3p-replacement-design.md`  
Replace OL3P entirely with **JAGR Knot** (native residual exact search). Match OL3P quality. Kill cubic on install. Keep Jhol soul.

Older notes: `residual-certified-ol3p-design.md` (bridge), `pulse-engine-design.md` (skip layers — rejected as product).

**Prototype:** `residual_ol3p/` (packument gate + bitset spine; Knot search next)  
Results: `results/RESIDUAL_CERTIFIED_TEST.md`

**Brutal bakeoff:** `results/BRUTAL_KNOT_BAKEOFF.md` (Knot vs OL3P vs jhol vs Bun)  
**Parity gates:** `results/NEXT_GATES.md` — Knot v4 = **9/9** + capsule scale (~30× warm)  
**Scale plan:** `plans/knot-v4-million-user-scale.md` (10 lakh users + 4-layer verdict)  
**Production L1:** Knot wired into JAGR3 Layer 1 — see `results/KNOT_L1_PRODUCTION.md`  
**Backup:** branch `backup/pre-knot-l1-2026-07-16` + `backups/jagr-pre-knot-l1/`

Knot v4 in production: `crates/jhol-core/src/jagr/knot.rs` (`JHOL_JAGR_L1=auto|knot|ol3p`).

```sh
cd residual_ol3p && cargo build --release
./target/release/residual-ol3p-experiment \
  --out ../results/brutal-knot.json \
  --packument-fixtures ../fixtures \
  --max-packages 40 --max-versions 12 --max-depth 3 \
  --jhol-bin "$PWD/../../../target/release/jhol" \
  --bun-bin "$(command -v bun)"
```

Plus `harness/topo_vs_pulse_microbench.py` — cubic dense elim vs bitset force-wave.

## Run

```sh
chmod +x harness/run_bakeoff.sh
BAKEOFF_TIMEOUT_SECS=90 ./harness/run_bakeoff.sh
```

Results land in `results/`.
