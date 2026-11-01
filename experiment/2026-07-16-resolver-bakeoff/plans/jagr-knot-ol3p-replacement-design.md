# JAGR Knot — OL3P Replacement Design

**Status:** Experiment design — original tree untouched  
**Date:** 2026-07-16  
**Identity:** Still **JAGR**. Layer 1 changes. Product name unchanged.  
**Goal:** Remove OL3P/CDCL/CNF from the resolution hot path entirely, match OL3P’s exact quality, kill full-graph cubic work on install, stop improvisation theater — keep Jhol’s unique soul.

---

## 0. The observation (design law)

> The most powerful solutions look simple and beautiful, yet carry deep complexity inside.

**Outside (beautiful):** one spine everyone can say in one breath.  
**Inside (deep):** bitsets, reasons, bounds, journals — invisible to the CLI story.  
**Forbidden:** seventeen named engines, dual geometries as product, MaxSAT as identity, “improve Residual+OL3P forever.”

---

## 1. What OL3P actually gives Jhol (quality to match)

OL3P is not magic. It buys four properties:

1. **Exact feasibility** — required deps/peers/platform either satisfied or proven UNSAT.  
2. **Exact objective** — soft prefer (`latest` / `minimal` / `optimize` / lockfile-preserving) with optimality, not greedy guess.  
3. **Explanations** — cores / derivation for conflicts.  
4. **Hard vs soft** — constraints never traded away for speed.

Any replacement must match **these four**, not “looks like MaxSAT.”

What OL3P costs Jhol:

- Package choices exploded into Boolean CNF (representation tax).  
- Solver identity borrowed from SAT world (less Jhol-native uniqueness).  
- Cubic topology + fat formula on the way in (Express death).

**Thesis:** Keep the four properties. Drop CNF and OL3P. Put exactness on **native package domains**.

---

## 2. What must stay (Jhol soul / uniqueness)

Do **not** become Bun/npm clone.

Keep:

| Soul piece | Role after OL3P removal |
|------------|-------------------------|
| **JAGR name + four-layer identity** | Layers remain; Layer 1 becomes Knot, not MaxSAT |
| **Cache-first / GRM / capsules** | Warm path still Jhol’s crown |
| **Fibers + Living Range Crystals** | npm placement / peer context without Cartesian blowup |
| **Append-only evidence + reason DAG** | Replayable “why” — replaces OL3P cores |
| **Independent verifier** | No uncertified lockfile |
| **Topology as observatory** | Betti / sheaf / tropical on `jhol jagr` — uniqueness for *understanding*, not install tax |
| **Offline / frozen / lockfile honesty** | Product contract unchanged |

Lose as *identity*: OL3P, CNF, CDCL watches, soft-clause weights.

---

## 3. The beautiful spine (entire algorithm)

```text
evidence
  → Fibers / Crystals          # compress behavior, not Boolean soup
  → force-wave                 # bidirectional domain wipe to fixed point
  → certified shell            # forced packages leave the fight
  → Knot search on residual Q  # native exact search + objective
  → reason certificate
  → verifier
  → lockfile
```

No CNF. No OL3P. No cubic matrix on install.

If `Q` empty after shell → done (most graphs; packument gate showed Express-like **36/41 shell, 5 residual vars**).

If `Q` huge → still Knot (same engine), not a second religion. Budget may yield `Unfinished` → existing non-JAGR fallback (pubgrub / frozen lock) as **escape hatch**, never as soul.

---

## 4. Layer contracts (soul preserved, cubic killed)

### Layer 0 — structure (advisory order only)

Separators, work order, capsule hints. Cannot delete a version without a native reason.

### Layer 3 — tropical (observatory / optional hint)

May suggest bounds or obstruction pressure for **ordering**. Never authorizes SAT/UNSAT/optimum on install. Full algebraic matrix work is **budgeted or off** on install.

### Layer 2 — sheaf / Betti (observatory)

`jhol jagr`, doctor, betti-history. Install does **not** build full-complex `O(N³)` boundary matrices. Uniqueness kept as *analysis*, not *tollbooth*.

### Layer 1 — **JAGR Knot** (sole exact authority)

Native finite-domain exact search on residual `Q` with objective proof. This replaces OL3P.

---

## 5. JAGR Knot — the deep part (simple API, deep guts)

### 5.1 State

- Per Fiber: `u64` (or word) **domain bitset** of Crystal/version cells.  
- Typed edges: required / optional / peer / override with support masks.  
- **Reason arena:** every wipe stores immutable reason (root range, dep, peer, conflict, branch, bound).  
- **Journal:** branch = word undo log (no full state clone).

### 5.2 Force-wave (same as Residual learning)

Forward support intersection + backward unsupported wipe to fixed point. Shell = popcount 1 + reasons replay.

### 5.3 Knot search (replaces OL3P search)

On residual packages only:

1. Pick variable by deterministic obstruction pressure (entropy, peer degree, stable id tie-break).  
2. Try Crystals/versions in **exact prefer order** for active objective.  
3. Assume → force-wave → recurse or close.  
4. Close only by: empty domain + reason, violated required/peer, or **objective bound** vs incumbent.  
5. Backtrack via journal.

**No Boolean variables.** One decision = one Crystal/version cell.

### 5.4 Objective (match OL3P soft quality)

Objective = deterministic **integer tuple** (same public prefer semantics as today).

- Maintain incumbent complete assignment.  
- Forward + backward **bound braid** on residual: prune only when replay proves cannot beat incumbent.  
- Optimality certificate: every residual region closed by contradiction or bound (Proof Horizon can wait; v1 = exhaustive residual coverage under bounds).

This matches OL3P soft optimum **without** soft clauses.

### 5.5 Explanations (match OL3P cores)

Conflict → walk reason DAG → **minimal nogood** on residual boundary (subset of Crystal assumptions). Store as membrane nogood; reuse when identical boundary returns.

Native package reasons > SAT cores for user-facing `why` / doctor. Uniqueness win.

### 5.6 Crystals (keep; they earned it)

Versions with identical authoritative behavior share a cell. Split monotone on new evidence. Search on Crystals; certificate expands to exact versions. Fewer decisions than raw versions — quality preserved if signatures exact.

### 5.7 Complexity (honest, short)

```text
O(S + F + E_f)           force + shell
+ O(d^K) worst           Knot on residual width d, size K
```

Favorable: `K` tiny after shell (measured).  
No `O(N_full³)`. No CNF size `v_full`.  
Worst case exponential in residual — same honesty as MaxSAT, **smaller base**.

---

## 6. Why this is *better* than OL3P for Jhol (not just different)

| | OL3P Layer 1 | JAGR Knot |
|--|--|--|
| Model | Boolean CNF | Package domains |
| Soft optimum | Soft clause weights | Native integer tuples |
| Conflict story | SAT cores | Package reason DAG |
| Install topology tax | Often on path | Observatories only |
| Uniqueness | Shared with MaxSAT literature | Native to npm/Jhol |
| Small residual | Still MaxSAT machinery | Journal DFS — right tool for K≈5 |
| Soul | Solver-shaped | Package-manager-shaped |

Better = **same four qualities**, **less representation lie**, **more Jhol-native explanation**, **cubic off install**.

---

## 7. What we explicitly will not build

- Obligation Möbius / Signature-Zipper / Dual-Geometry Loom on hot path  
- Residual-Certified OL3P as the product (it was a bridge experiment)  
- Skipping peers/optionals/overrides for speed  
- Heuristic final authority (tropical/spectral scores authorizing lockfile)  
- Claiming Bun-beating without stage-split install evidence  

OL3P code may remain in-tree temporarily as **shadow oracle only** during rollout — not Layer 1.

---

## 8. Correctness = OL3P parity gates

Knot matches OL3P quality when:

1. Exhaustive small universes: same SAT/UNSAT + same optimum class as OL3P.  
2. Fixture shadow: assignment equal or equal canonical optimum vs current JAGR/OL3P path.  
3. Metamorphic: order/threads/hydration batch do not change lockfile.  
4. Verifier rejects any incomplete optimality proof.  
5. Unknown manifest ≠ empty deps.

**Designed quality loss vs OL3P: 0.**  
**Designed loss vs full-complex Betti on install: yes (moved to observatory).**

---

## 9. Rollout (simple)

```text
experiment Knot prototype
  → shadow vs OL3P on fixtures
  → Gate: zero feasibility/objective mismatch
  → promote Layer 1 = Knot
  → OL3P shadow → delete from hot path
```

Performance Gate: Express resolve under current guard; install stage-split vs Bun reported honestly (resolve ≠ install).

---

## 10. Implementation phases (experiment first)

| Phase | Deliverable |
|-------|-------------|
| 0 | Freeze OL3P fixture outputs as oracle |
| 1 | Domains + force-wave + shell + reasons (already half-done in experiment) |
| 2 | Knot journal search + prefer-order branching |
| 3 | Objective bounds + optimality close |
| 4 | Nogood learning from reason DAG |
| 5 | Shadow parity vs OL3P on packument fixtures |
| 6 | Wire observatory-only L2/L3; install never cubic |
| 7 | Promote plan for original tree (separate decision) |

---

## 11. One-sentence product story

**JAGR resolves by forcing what must be true, then exactly searching only the remaining knot — on packages, not Boolean formulas — and proves the prefer optimum with reasons you can replay.**

That is simple. That is beautiful. The complexity lives in bitsets, journals, and proofs — where it belongs.

---

## 12. Decision

| Question | Answer |
|----------|--------|
| Replace OL3P entirely? | **Yes** on hot path |
| Match OL3P quality? | **Yes** — four properties + parity gates |
| Keep Jhol uniqueness? | **Yes** — JAGR layers, Crystals/Fibers, reasons, cache-first, topology observatory |
| Improvisation? | **Stop** — one spine |
| Next code? | Prototype Knot search in `experiment/.../residual_ol3p` (rename path later), OL3P = shadow only |

---

## 13. Million-user addendum (Knot v4)

See `knot-v4-million-user-scale.md`.

Resolution **capsules** + shell-first + journal undo sit on Layer 1.  
Layers 2–3 stay observatory. GRM/CAS own package-byte cache.  
Measured experiment: 100k warm lookups, hit rate 1.0, ~30× vs cold.  

