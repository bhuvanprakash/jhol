# Knot Rising Star — Design & Execution Guide

> Status: Planning reference for the Knot L1 solver evolution.
> Goal: reduce reliance on OL3P + sheaf cohomology fallbacks; make Knot the
> default complete solver with a provable optimality trigger.

---

## 1. What Knot Is (current state)

Knot is a **package-domain MAC (Maintaining Arc Consistency) search**, not a SAT solver.

- **Domains** = bitsets of versions per package (`DomainBits`, `FlatBitMatrix` with SIMD)
- **Propagation** = arc consistency over edge support rows (GFP loop, journaled undo via `JournalArena`)
- **AC-soften**: when propagation empties a domain, it *widens incoming edges*
  (`open_supports_into`) until feasible — MaxSAT-relaxation done greedily
- **Resolution capsules**: in-process cache keyed on u64 graph fingerprint (50k cap)
- **Ordering**: `focus_packages` (SCC obstructions) + Betti scores branch first;
  prefer-latest objective
- **Fallbacks**: second soften round, greedy completion, then OL3P

Entry: `knot_solve(request, betti_order)` — `crates/jhol-core/src/jagr/knot.rs:1037`.
Wired as Layer 1 in `jagr_resolver.rs:1030-1070` with `scc_obstruction_pkgs` focus
(line 1031) and Betti ordering (line 1032).

---

## 2. Where the 1% gap lives (5 mechanisms)

1. **Greedy softening ≠ minimal relaxation.** `soften_until_ac` picks the *first*
   emptied victim (`g.domain.iter().position(...)`), opens its edges, repeats.
   OL3P's MaxSAT finds the *minimum-weight* relaxation set. Knot relaxes "enough",
   not "least". On adversarial graphs this over-relaxes → worse (older) selection.

2. **Victim choice ignores structure.** `pick_variable` uses SCC focus + Betti for
   *branching*, but `soften_until_ac` does NOT use it for *which edge to relax*.
   The same signal is used asymmetrically.

3. **No lower bound ⇒ no optimality proof.** Knot stops at "feasible". It never
   knows whether its relaxation count is *provably* minimal, so the 1% is
   undetectable — you can't measure what you can't bound.

4. **Capsule is all-or-nothing on the whole graph.** Any transitive change
   invalidates the full fingerprint. `star_edit_cover` exists for delta
   re-resolution, but the capsule store doesn't decompose.

5. **Hard/soft semantics are binary.** One `weak` flag. Real npm semantics are
   graded: explicit root dep is hard; transitive peer-dep is soft. Knot can't
   express "never break the root, but bump anything transitive".

---

## 3. Pareto innovations (high leverage, low effort — do these first)

### P1 — Lexicographic objective: (max version recency, min relaxations)
Track the number of weakened edges as a secondary objective. MAC search already
journals which edges were opened — count them. A selection with the same
"lateness" but fewer relaxations wins.
- ~10 lines. Fixes mechanism #1.

### P2 — Conflict-directed victim selection
In `soften_until_ac`, prefer the emptied victim belonging to `scc_obstruction_pkgs`
(already in `KnotRequest.focus_packages`). Relaxing an SCC-internal edge fixes the
cyclic obstruction with fewer total relaxations than a DAG edge.
- Reuses data Knot already receives. Fixes mechanism #2.

### P3 — Weighted edge classes (semantic softening)
Replace binary `weak` flag with weight enum:
`Hard(root) > Hard(transitive) > Soft(peer) > Soft(optional)`.
Never soften `Hard(root)`. Soften lowest-weight-first.
- Semantic ordering, mirrors human resolver intent. Fixes mechanism #5.

### P4 — Post-hoc local optimality certificate (SEE SECTION 6)
After Knot returns a selection: for each weakened edge, try re-tightening and
re-propagate once. If feasible, keep tight. Result: **no single relaxation is
removable** — a certificate. When tightening fails, provably hit the 1% → escalate.
- THE most important Pareto item: turns the 1% from unknown to detectable.

### P5 — Per-SCC capsule decomposition
Fingerprint each SCC + boundary separately. A change in SCC-A doesn't invalidate
SCC-B's cache. Combined with `star_edit_cover`, delta re-resolution becomes
O(changed SCC) not O(graph). Fixes mechanism #4.

---

## 4. New innovations (research-grade, Jhol-native)

### N1 — Tropical relaxation monoid
Model each edge with a relaxation *cost* (0 hard, 1 soft) in the tropical
semiring. Search becomes tropical shortest-path over the lattice of relaxations;
minimal relaxation set = tropical distance-to-feasibility. Make *cost*
propagation a fixed point too (like AC propagation already is).
- Extends Jhol's own tropical geometry into the search itself.

### N2 — Persistent content-addressed resolution capsules
`capsule_store` (process-local, 50k cap) becomes JAGR-S objects:
key = full sha256 graph hash, value = selection + relaxation certificate + objective.
Cross-process, offline, shareable with the daemon.

### N3 — Markov over solver decisions
Use `second_order_markov`: learn which victim/edge pairs were relaxed in past
unsat cases → seed soften order for similar future graphs.

### N4 — SCC divide-and-conquer with boundary reconciliation
Tarjan already runs (`compute_version_graph_scc`, jagr_resolver.rs:749). Solve
each SCC with Knot independently (each small enough to be near-exhaustive), then
reconcile boundary edges with one AC pass, O(E). Total stays O(n).

### N5 — Betti lower-bound oracle (SEE SECTION 7)
Use Layer-2 sheaf cohomology **only as a provable floor**: β₁ of the conflict
graph = minimum number of relaxations needed. Knot searches; when relaxation
count hits β₁, it's provably optimal — stop. When greedy exceeds β₁, escalate.
- Makes the sheaf math useful as a bound instead of dead weight.
- Rigorous trigger for the 1%: `relaxation_count > β₁ ⇒ escalate`.

### N6 — Closed-loop measurement
Log every Knot solve (graph stats, relaxations, escalations, OL3P agreement).
After enough real installs, compute the *empirical* 1% — "graphs where Knot and
OL3P differ have features X" — then specialize.

---

## 5. Recommended order

1. **P4 (certificate)** — makes the 1% measurable today, ~30 lines.
2. **P1 + P2 + P3** — close mechanisms #1, #2, #5.
3. **N5 (Betti bound)** — flagship: re-unites the two systems, provable trigger.
4. **N4 / P5 / N2** — structural scaling: SCC decomposition + persistent capsules.
5. **N1 / N3 / N6** — research frontier.

---

## 6. P4 — Local Optimality Certificate (deep dive)

### Goal
After `knot_search` returns a feasible selection, prove that no single edge
relaxation is removable. If the proof succeeds, Knot's answer is locally optimal
(no one-edge-better solution exists). If it fails, the graph is a genuine 1%
case → escalate to OL3P.

### Algorithm (O(relaxed × propagation), still O(n))

```
fn certify_local_optimality(g: &KnotGraph, selection: &HashMap<String,String>) -> Certificate {
    // 1. Rebuild the graph at the final selection.
    // 2. For every edge marked weak in the final solve:
    //    a. tentatively re-tighten: restore original support rows for that edge,
    //       set weak=false
    //    b. re-propagate (GFP) with the selection domains pinned
    //    c. if propagation succeeds (no empty domain, no conflict):
    //         → the relaxation was NOT needed → certificate FAILS
    //       else:
    //         → the relaxation IS necessary → keep weak=true
    // 3. If every relaxed edge came back necessary:
    //      Certificate = LOCAL_OPTIMAL (no single relaxation removable)
    //    else:
    //      Certificate = IMPROVABLE (found a one-edge-better solution)
}
```

### Data needed
- The `pristine` support rows (before any softening) — already available in
  `knot_solve` via `request` → rebuild edges from `request.dependencies`.
- The set of relaxed edges — currently implicit in `g.edges[].weak`.
  Add: track relaxed edge indices during `soften_until_ac`.

### Output contract
```
enum Certificate {
    LocalOptimal,      // every relaxation is necessary → answer is final
    Improvable { edge: u32 },  // found removable relaxation → re-solve or escalate
}
```

### Integration
- `knot_solve` returns `certificate: Certificate` in `KnotResult`.
- `jagr_resolver.rs`: if `LocalOptimal` → accept answer, skip OL3P.
- If `Improvable` → tighten that edge, re-search once, re-certify.
  If still improvable → escalate to OL3P (rare).

---

## 7. N5 — Betti Lower-Bound Oracle (deep dive)

### Goal
Use Layer-2 cohomology (β₁) as a *provable lower bound* on the minimum number of
relaxations. This re-unites sheaf cohomology with the solver:
- cohomology gives a FLOOR (proof of optimality when reached)
- Knot does the SEARCH (fast, O(n))
- OL3P only fires when Knot exceeds the floor without proving optimality

### The math (honest construction)

**Naive version (INCORRECT — do not implement):**
> β₁ of the dependency graph = min relaxations.
>
> Why wrong: a dependency cycle A→B→C→A can be *fully consistent*
> (A@1→B@1→C@1→A@1 all compatible). A consistent cycle needs 0 relaxations,
> so its cycle rank over-counts the bound. The floor must be built on the graph
> of edges that are actually *inconsistent*.

**Correct version — the Inconsistency Graph H:**

1. Run GFP with ALL edges tight (current `propagate_ex`).
2. For each edge e = (s → d), check whether it is *inconsistent*:
   `support[s_domain][d_domain]` intersection is empty — no version pair in the
   current domains can satisfy e.
3. Build H = undirected graph with an edge for every inconsistent dependency edge.
4. Compute the **cycle rank** β₁(H) = |E_H| − |V_H| + c_H
   (rank-nullity of the incidence matrix — O(E·α(V))).

**Claim (floor):** any valid assignment must relax at least β₁(H) edges
*of the edges currently in H* … **but only if relaxing other edges cannot make
an H edge consistent again.** It can. So:

- β₁(H) is a valid floor **per propagation round**: before relaxing anything,
  the minimum relaxation count ≥ β₁(H) for the current state.
- It is NOT a monotone global floor across softening rounds (domains narrow and
  edges can re-consist). Use it as: **(a) an ordering hint, (b) a per-round
  early-exit**, not as a global proof.

### The rigorous floor (what we actually ship)

Three layered guarantees, weakest to strongest:

| # | Mechanism | What it proves | Cost |
|---|---|---|---|
| G1 | β₁(H) per round | Lower bound for the *current* state → order search, early-exit | O(E·α) |
| G2 | P4 certificate | No *single* relaxation is removable → locally optimal | O(r·propagate) |
| G3 | N4 SCC-exhaustive | Small SCCs solved exactly (≤ ~20 versions) → globally optimal for that SCC | 2^|SCC| worst case, tiny SCCs |

**Knot becomes the rising star by combining them:**
- G1 orders softening and stops search early on easy graphs (β₁ = 0 → answer is
  already optimal, no OL3P).
- G2 certifies the answer after search → OL3P skipped.
- G3 makes small hard cores exact → OL3P skipped.
- OL3P fires ONLY on: large-SCC + certificate failure. Genuinely rare.

### Solver integration (the trigger)

```
fn knot_solve_with_oracle(request, betti_bound: usize) -> KnotResult {
    let mut relaxations = 0;
    // ... existing soften + search ...
    // TRACK relaxations as edges are weakened.

    if relaxations < betti_bound {
        // Below the per-round floor: still searching, continue.
    } else if relaxations == betti_bound {
        // Hit the floor → no better solution exists for the current round.
        // Run P4 certificate; if LocalOptimal → DONE, skip OL3P.
    } else if relaxations > betti_bound {
        // Overshot the round floor → greedy is suboptimal here.
        // Run P4 certificate; if Improvable → re-search (P2 victim choice);
        // if still Improvable on a large SCC → escalate to OL3P (rare).
    }
}
```

### Why this reduces OL3P/cohomology reliance

- **Knot alone** handles the 99%: G1 (fast early-exit) + G2 (certificate) + G3
  (small-SCC exact) cover virtually all real npm graphs.
- **Cohomology** is no longer a fallback — it's a cheap *cycle-rank oracle*
  (O(E·α(V)), far cheaper than full sheaf cohomology) feeding Knot's ordering.
- **OL3P** fires only when: large SCC + Improvable certificate — a provable,
  evidence-driven escalation instead of a heuristic one.
- The escalation is now **provably correct**: you only escalate when you have
  *evidence* the greedy path is suboptimal (certificate says Improvable).

### Optional refinement — per-SCC relaxation budget (N4 synergy)
Use the per-component β₁ as a per-component relaxation budget in SCC
divide-and-conquer: each SCC must relax ≥ its own β₁; solve SCCs independently
with budget = β₁(SCC) + slack; reconcile boundary edges with one AC pass.


---

## 8. Acceptance criteria (for coding)

### P4
- [ ] `knot_solve` returns a certificate enum
- [ ] LocalOptimal means: every weak edge re-tightened → propagation fails
- [ ] Improvable leads to one re-search, then OL3P escalation
- [ ] Tests: synthetic 2-edge conflict graph → LocalOptimal
  synthetic removable-relaxation graph → Improvable → escalate

### N5
- [ ] `betti_lower_bound` computes cycle rank β₁(H) on the **inconsistency graph H** (not the raw dependency graph) — test on known graphs: triangle of inconsistent edges → β₁=1; consistent triangle → β₁=0
- [ ] G1: β₁(H)=0 short-circuits to "already optimal, no search" (the 99% easy case)
- [ ] G2: certificate (P4) is the *proof* mechanism; `relaxations == β₁(H)` triggers certificate check, not a bare PROVABLY_OPTIMAL claim
- [ ] G3: small SCCs (≤ ~20 versions) solved exactly via exhaustive per-SCC search
- [ ] Property test: on random graphs, `betti_bound ≤ per-round_min_relaxations` holds **for the state H was computed on** (never claim a global floor)

### Regression
- [ ] Existing `tests/resolver-snapshots/` parity suite still passes
- [ ] `cargo test -p jhol-core` green
- [ ] No OL3P call on the 99% happy path (assert in test)

---

## 9. File map

| File | Change |
|---|---|
| `crates/jhol-core/src/jagr/knot.rs` | P1 (objective), P2 (victim), P3 (weights), P4 (certificate), N5 (oracle), relaxed-edge tracking |
| `crates/jhol-core/src/jagr/jagr_resolver.rs` | wire certificate + oracle into Layer 1 (lines 1030-1070), skip OL3P when LocalOptimal |
| `crates/jhol-core/src/jagr/mod.rs` | re-export new types |
| `crates/jhol-core/src/jagr/tropical_sheaf.rs` | (N5 optional) cheap cycle-rank helper, reuse for beta_1 |
| `tests/` | P4 + N5 property tests, snapshot parity |

---

*Knot is the rising star. OL3P + cohomology become the referee, not the engine.*
