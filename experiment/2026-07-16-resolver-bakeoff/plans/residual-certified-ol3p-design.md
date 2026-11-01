# Residual-Certified OL3P Design (Improved)

**Status:** Experiment design — not promoted; original tree untouched  
**Date:** 2026-07-16  
**Product identity:** JAGR (unchanged)  
**Exact Layer-1 authority:** OL3P / OL3P-S (kept)  
**Scope:** Kill full-graph cubic cost and fat-CNF blowup while preserving tropical + sheaf + MaxSAT quality on a certified residual.

**Relation to other plans:**
- Beats **Pulse** by not skipping L2/L3.
- Beats **Dual-Geometry native replacement** for the constraint “OL3P stays best exact brain.”
- Steals the good Dual-Geometry parts: Fibers, Living Range Crystals, dominance, shell, low-width Hodge Loom on residual, independent verifier, shadow/fallback.

---

## 1. One-sentence thesis

Run the full JAGR math stack and OL3P on the **smallest certified residual quotient**, not the full dependency complex — then let OL3P cores and L2/L3 certificates **tighten each other** until the residual is solved or proven UNSAT.

---

## 2. What was wrong with thinner plans

| Plan | Mistake relative to this goal |
|------|-------------------------------|
| Pulse | Skips L2/L3 → real quality loss |
| Dual-Geometry (as written) | Replaces OL3P → abandons preferred exact engine |
| Today’s JAGR | L2/L3 + CNF on **full** `N` → cubic + Express timeout |

Residual-Certified OL3P keeps: **same layers, same OL3P, smaller exact stage.**

---

## 3. Complexity targets (improved accounting)

Symbols:

- `V` — authoritative versions before Crystal compression  
- `F` — Contextual Instance Fibers  
- `E_f` — typed edges between fibers  
- `S` — compiled support incidences  
- `Q` — residual quotient vertices after certified shell  
- `N_Q` — Hodge/tropical matrix size on residual only (`N_Q ≪ N_full`)  
- `k_H` — treewidth of residual Hodge incidence graph  
- `v_Q` — OL3P Boolean variables after Crystal-CNF (`v_Q ≪ v_full`)  
- `c_Q` — clauses in residual CNF  
- `T_decomp` — residual decomposition discovery  
- `R` — residual refinement rounds (core ↔ topology feedback)  
- `η_Q` — residual semantic entropy  

### Favorable install hot path

```text
O(S + F + E_f                          # compile + force-wave + shell
  + R · (
      T_L3(N_Q)                        # tropical on residual only
    + T_decomp + k_H² N_Q              # Hodge Loom on residual only
    + T_OL3P(v_Q, c_Q)                 # core-guided MaxSAT on residual CNF
  )
  + T_verify)
```

### Explicitly forbidden on install

```text
O(N_full³) dense Betti / full-complex SNF as a required gate
full-graph Nullstellensatz without residual shrink + matrix-work guard
rebuilding OL3P from scratch on every hydration batch
```

### Worst case (honest)

If shell cannot shrink (`Q ≈ full`, hard peer knot): still exponential in `v_Q`. No fake linear claim. Telemetry must report `N_Q`, `v_Q`, `R`, fallback reason.

### Quality vs complexity bargain

| Quantity | Today | This design |
|----------|-------|-------------|
| Topology domain | `N_full` | `N_Q` |
| OL3P vars | `v_full` | `v_Q` via Crystals + shell unit clauses |
| Exact engine | OL3P | **OL3P** |
| L3/L2 presence | yes, full | yes, residual + certificates |
| Intended quality loss | — | **0%** on lockfile semantics + objective |

---

## 4. Architecture (seven units — not seventeen)

```text
1. Contextual Instance Fibers
2. Living Range Crystals + Dominance Antichain
3. Bidirectional force-wave + Certified Shell
4. Residual Quotient Planner (measures N_Q, v_Q, η_Q)
5. Residual Math Forge (L3 then L2 on Q only) → Certificate Bundle
6. Residual OL3P Engine (persistent, Crystal-CNF) ←→ cores feed Forge
7. Independent Verifier + Shadow Controller
```

Data flow:

```text
metadata (append-only evidence)
  -> Fibers / Crystals / Dominance
  -> force-wave to fixed point
  -> certified shell (forced packages leave Q)
  -> loop R = 1..R_max:
       L3(Q) -> tropical certificate clauses / bounds
       L2(Q) -> obstruction support + Loom rank seal (if needed)
       compile Crystal-CNF(Q) ∪ shell units ∪ math certificates
       OL3P solve / improve
       if SAT+optimal: break
       if UNSAT core: shrink Q or emit CertifiedUnsat
       if soft core: tighten residual / objective bounds; continue
  -> verify(shell ∪ assignment ∪ certificates)
  -> emit lockfile  OR  shadow fallback to current path
```

No stage may skip the verifier for a promoted result.

---

## 5. Improvements beyond the first Residual sketch

### 5.1 Crystal-CNF (semantic compression, not clause soup)

- One Boolean family per **Living Range Crystal** in residual Fibers (not one var per raw version when behavior-identical).
- Exactly-one / at-most-one encoded compactly (sequential counters or native cardinality OL3P already understands if present; else compact sequential).
- Typed support edges → hard clauses from compiled masks.
- Optional deps / platform → soft or conditional hard per existing Jhol semantics.
- Shell Fibers → **unit assumptions** (not deleted evidence): OL3P sees them as fixed literals; verifier expands to versions.

**Quality:** same feasible set as version-level encoding if Crystal splits are sound.  
**Speed:** `v_Q` tracks behavioral cells, not registry noise.

### 5.2 Math Forge = certificate factory (not a skip gate)

L3 and L2 always *may* run on `Q`. They never authorize the lockfile alone.

Emit only replayable objects:

| Source | Certificate kind | How OL3P consumes it |
|--------|------------------|----------------------|
| L3 tropical | infeasible direction / slack bound | hard blocking clauses or soft lower bounds |
| L3 Nullstellensatz (budgeted) | residual algebraic obstruction | hard clauses on Crystal ids |
| L2 obstruction support | package/Crystal set that must participate | assumptions / projection |
| L2 Hodge Loom | Two-Sided Rank Seal on residual | topology invariant; may force separators |
| L2 membrane | component split of `Q` | **membrane-local OL3P** (5.3) |

If matrix-work estimate exceeds Proof Dividend budget: skip *that algebraic expansion*, keep cheaper residual certificates, still run OL3P. Skipping an expansion ≠ skipping the layer identity; layer may return `BudgetedSkip` with no semantic change.

### 5.3 Membrane-local OL3P (compose exact pieces)

When L2 certifies that residual components `Q₁…Qₖ` have only shell-fixed crossings:

1. Solve each `Qᵢ` with its own OL3P instance (or incremental assumptions).
2. Compose assignments under membrane signature.
3. Global objective: Pareto / integer tuple composition identical to current prefer modes.

**Complexity:** sum of `T_OL3P(v_{Qᵢ})` often ≪ `T_OL3P(v_Q)` when separators exist.  
**Quality:** exact if membrane proof replays (same as Dual-Geometry membrane rule).

### 5.4 Core ↔ Topology feedback loop (the deep upgrade)

Today: L3 → L2 → one-shot OL3P. Information flows one way.

Improved:

```text
OL3P soft/hard core C
  -> map literals back to Crystals/Fibers
  -> refine obstruction support for L2
  -> optionally re-run Loom / tropical on shrunk support
  -> inject new certificates into CNF
  -> continue OL3P with persistent state (no full rebuild)
```

Caps `R` with deterministic dividend: stop feedback when measured residual shrink or bound improvement falls below threshold; final OL3P call remains authoritative.

**Quality:** still OL3P-closed. Feedback only adds sound clauses/bounds.

### 5.5 Persistent OL3P across hydration

Hydration must not destroy watches, activities, and learned cores wholesale.

Rules:

1. Append-only evidence; Crystal splits are monotone.
2. Invalidation = exact reverse dependency cone on affected vars/clauses only.
3. Unaffected cores remain; solver assumes new units for newly forced shell.
4. Changed manifest hash → new context generation for affected membrane only.

This attacks the “state loss” structural cost from the Dual-Geometry evidence section **without** deleting OL3P.

### 5.6 Bidirectional objective braid into soft weights

Forward mandatory-closure bound + backward peer/policy obligations become:

- soft-clause weights / relaxation order hints for OL3P-S, and/or
- incumbent cutoff when a complete residual assignment exists.

An uncertified bound cannot delete a feasible better optimum. Bound may only prioritize cores or prune when replay proves dominance — same invariant as the Dual-Geometry bound braid, adapted to MaxSAT.

### 5.7 Observability split (diagnostics ≠ install)

| Mode | Topology domain | Purpose |
|------|-----------------|--------|
| `jhol install` / resolve | residual `Q` only | production |
| `jhol jagr` / `--run-all-layers` | may use full complex | diagnostics, Betti history, research |

Full-`N` Betti remains JAGR soul for analysis commands. Install never requires it.

---

## 6. Four-layer contracts (OL3P-preserving)

### Layer 0 — structural influence

Proposes Fiber merges, Crystal splits, separators, work order. Cannot delete a version without a native-domain reason that replays.

### Layer 3 — tropical feasibility (on `Q`)

Produces certificates and bounds for the residual. Budgeted algebraic work. Never final lockfile authority.

### Layer 2 — sheaf / Hodge (on `Q`)

Produces obstruction supports, membranes, Loom seals. Enables membrane-local OL3P. Full diagnostic cohomology available offline.

### Layer 1 — OL3P / OL3P-S (on Crystal-CNF(`Q`))

**Sole production exact solver** for residual search and objective optimality under prefer modes. Emits assignment + cores. Cores feed Layer 2/3 refinement.

---

## 7. Correctness definition (zero intended loss)

A result is correct only if:

1. All current Jhol/npm semantic rules hold (deps, optionals, peers, overrides, platform, engines, prerelease, lockfile).
2. Shell Fibers are forced by replayable reasons.
3. Every L2/L3 certificate used by OL3P replays on residual evidence.
4. OL3P assignment satisfies Crystal-CNF and expands to exact versions/placements.
5. Objective matches requested prefer mode (or same deterministic optimum class).
6. Independent verifier accepts the combined proof.
7. Determinism: same authoritative input → same lockfile regardless of thread timing / hydration batching.

**Designed quality loss: none.**  
**Designed diagnostic loss on install: full-complex Betti.** That is not resolution quality.

---

## 8. Result states

1. `CertifiedSolution` — verified shell + OL3P assignment + certificates  
2. `CertifiedUnsat` — residual UNSAT core covers all branches under evidence  
3. `NeedsMetadata` — missing authoritative manifests block certification  
4. `Unfinished` — budget / timeout → shadow fallback (never fake SAT/UNSAT)

---

## 9. Shadow, fallback, rollout

Same discipline as Dual-Geometry gates, OL3P-centric:

```text
off -> shadow 100% vs current JAGR -> residual 1% -> 10% -> 50% -> default
```

Mismatch classes: assignment equal / equal optimum / feasibility mismatch / objective mismatch / verifier reject / timeout.

Fallback: current full-path JAGR or existing resolver policy. No uncertified native partial contaminates lockfile.

---

## 10. Implementation phases (experiment → promote)

| Phase | Deliverable | Gate |
|-------|-------------|------|
| 0 | Freeze baselines, Express timing, fixture locks | — |
| 1 | Fibers + Crystals + support masks (experiment crate OK) | semantic expand parity |
| 2 | Force-wave + shell + residual `Q` metrics | shell soundness |
| 3 | Crystal-CNF compiler + OL3P on `Q` only (L2/L3 off) | exhaustive small-universe parity |
| 4 | Persist OL3P across hydration | determinism metamorphic |
| 5 | L3 certificates on `Q` → CNF injection | no quality regress vs phase 3 |
| 6 | L2 membranes + Loom on `Q` + membrane-local OL3P | Express under guard |
| 7 | Core ↔ topology feedback (`R` rounds) | shadow parity |
| 8 | Verifier + capsules + rollout | Gate E performance |

**Critical order:** Phase 3 must beat Express timeout *before* investing in Loom. OL3P-on-residual is the primary win; math certificates are multipliers.

---

## 11. Benchmark claims (allowed vs forbidden)

**Allowed:**
- Report `N_full` vs `N_Q`, `v_full` vs `v_Q`, `R`, memo/core hits, stage times.
- Median/P95 vs current JAGR and vs Bun on equal fixtures.
- Chebyshev / spectral only as routing; disable mode must match answers.

**Forbidden:**
- Claim linear-time exact resolution universally.
- Claim “faster than Bun” without raw reproducible runs.
- Attribute end-to-end speedup to a single microbench stage.

---

## 12. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Crystal merge unsound → quality loss | Split on every authoritative behavior dim; hash nominate, field replay certify |
| Residual still huge | Membrane split + feedback; fallback; report `v_Q` |
| Certificate clause unsound | Verifier rejects; shadow mismatch halts rollout |
| Feedback loops waste time | Proof Dividend on `R`; max rounds |
| Persistent OL3P nondeterminism | Canonical clause ids; stable var order; commit order |
| Recreate Dual-Geometry complexity | Hard cap: seven units; no Möbius/Zipper required for v1 |

---

## 13. Invariants

1. Domains shrink monotonically within branch/context.  
2. Evidence append-only within context generation.  
3. Shell exit requires replayable force proof.  
4. L2/L3 never authorize lockfile alone.  
5. OL3P is the only residual search authority on the promoted path.  
6. Every injected certificate replays.  
7. Unknown metadata never authorizes empty deps.  
8. Timeout ⇒ `Unfinished`, not SAT/UNSAT.  
9. Full-`N` topology is optional diagnostics, not install correctness.  
10. Shadow mismatch ⇒ no promotion.

---

## 14. Final decision statement

JAGR keeps its four-layer soul and keeps **OL3P as Layer-1**.  
The invention is **Residual Certification**: compile Fibers/Crystals, collapse the forced shell, run tropical + sheaf/Hodge only on `Q`, solve with persistent OL3P on Crystal-CNF(`Q`), and close a core↔topology feedback loop under an independent verifier.

```text
complexity aim:  O(S+F+E_f + R·(T_L3(N_Q) + k_H² N_Q + T_OL3P(v_Q)))
quality aim:     0 intended loss vs full JAGR semantics + OL3P optimality
cubic aim:       never O(N_full³) on install
```

---

## 15. Next experiment step

Inside `experiment/2026-07-16-resolver-bakeoff/` only:

1. Prototype Crystal-CNF + shell metrics on express/overrides/peer fixtures.  
2. Time `T_OL3P(v_full)` vs `T_OL3P(v_Q)` using copied OL3P (path dep / copy), no original edits.  
3. Compare to Bun and current `jhol jagr` arms already in the bakeoff harness.
