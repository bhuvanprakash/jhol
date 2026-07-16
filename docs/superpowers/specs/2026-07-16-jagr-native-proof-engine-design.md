# JAGR Native Proof Engine Design

**Status:** Approved revised architecture; implementation has not started
**Date:** 2026-07-16
**Product name:** JAGR
**Scope:** Replace generic OL3P/CDCL on JAGR's production resolution hot path while preserving the four-layer identity, exact resolution quality, deterministic output, and safe fallback behavior.

**Revision:** The approved native proof engine now includes Certified Range Cells, a Dominance Antichain, Proof Supernodes, the Certified Membrane-Local Chebyshev-Hodge Proof Funnel, a Cohomology Conservation Ledger, Membrane Braid Join, a Residual Core Basis, Bidirectional Bound Braid, Conflict Membrane Learning, and an Adaptive Exact Portfolio. These mechanisms replace full-graph candidate enumeration, broad cubic cohomology recomputation, and monolithic depth-first knot search without weakening exactness.

## 1. Executive summary

JAGR currently converts the package-resolution problem into a generic Boolean optimization problem and delegates final authority to OL3P/CDCL. Profiling after the certified-lazy resolver work showed that metadata expansion, tropical Nullstellensatz construction, and Mayer-Vietoris cover construction were real costs, but they were not the terminal bottleneck. Once those costs were reduced, the exact OL3P/CDCL path dominated runtime. On the Express lockfile fixture, the old JAGR path, the new default path, and the refined path all reached a 120-second guard, while Bun 1.3.14 completed in 3.114 seconds.

The problem is therefore not the existence of JAGR's four mathematical layers. The problem is their current relationship with the exact solver. The upper layers mostly analyze or classify the graph, while the final layer still receives a large generic Boolean formula. Lazy metadata refinement can also cause repeated solves after the request fingerprint changes, losing much of the prior exact-search work.

This design replaces the production OL3P/CDCL path with a package-native exact resolver. It compiles behaviorally equivalent versions into Certified Range Cells, removes only proof-dominated cells, contracts membrane-equivalent components into Proof Supernodes, propagates constraints and objective bounds in both directions, and collapses forced packages into a certified shell. The remaining topology passes through a Certified Membrane-Local Chebyshev-Hodge Proof Funnel: spectral work routes proof effort, certified reductions preserve topology, exact local bases are stitched across membranes, and cubic rank work is confined to the irreducible residual core. Hard knots use balanced Membrane Braid Join rather than monolithic depth-first enumeration. Every transformation carries a replayable reason, and a separate certificate verifier—not the solver itself—authorizes output.

The old OL3P/CDCL implementation is not immediately deleted. During rollout it becomes a shadow oracle, emergency fallback, and difficult-case collector. The current default remains authoritative until the native engine passes correctness, determinism, parity, and performance gates.

This remains JAGR. No new product name or alternate resolver identity is introduced.

## 2. Goals

The design must:

1. Remove generic CNF construction and OL3P/CDCL from JAGR's normal production hot path.
2. Preserve exact required dependency, optional dependency, peer dependency, override, resolution, platform, prerelease, and lockfile semantics.
3. Preserve exact objective behavior for `latest`, `minimal`, `optimize`, and lockfile-preserving modes.
4. Keep JAGR's four-layer structure and make the layers cooperate through proof-bearing contracts.
5. Maintain one persistent problem state across metadata hydration and search rounds.
6. Resolve easy and mostly forced graphs through propagation without exact branching.
7. Restrict exact search to the smallest unresolved obstruction components.
8. Produce independently replayable certificates for success, UNSAT, decomposition, and objective optimality.
9. Exploit Rust's compact data layout, bit operations, arenas, safe parallelism, and deterministic integer arithmetic.
10. Remain safe by default through shadow comparison, fallback, telemetry, and automatic rejection of uncertified results.
11. Replace full-graph cubic topology work with exact parameterized work over sparse support, certified membrane width, and a small residual core.
12. Reduce both candidate width and exponential knot depth before exact enumeration.

## 3. Non-goals

This work will not:

1. Rename JAGR.
2. Remove any currently supported package-manager semantics.
3. Treat missing metadata as an empty dependency set.
4. Accept a feasible assignment without proving its requested objective.
5. Allow tropical, spectral, or sheaf heuristics to authorize an output by themselves.
6. Copy another package manager's resolver implementation, internal scoring model, or published algorithm line for line.
7. Delete the old exact path before rollout evidence supports doing so.
8. Introduce unsafe Rust merely for speculative speed.
9. Promote the native engine based on microbenchmarks alone.
10. Claim superiority over Bun or npm without reproducible measured evidence.
11. Claim universal linear-time exact Betti-number computation for unrestricted complexes.
12. Allow Chebyshev estimates, hashes, dominance heuristics, or portfolio selection to become correctness authorities.

## 4. Evidence motivating replacement

The certified-lazy work removed or bounded several earlier costs:

- Abbreviated packument domains avoid treating unknown manifests as authoritative.
- Bounded parallel manifest hydration reduces serial network work.
- Exact-frontier Minkowski expansion prevents repeated support re-expansion.
- Tropical Nullstellensatz work is guarded by a matrix-work estimate.
- Mayer-Vietoris cover construction uses deterministic score indexing and compact simplex bitsets.
- Large diagnostic sheaf analyses are scale-adaptive, with explicit full-analysis override.

After those changes, the final profiler sample placed nearly all resolver time inside the core-guided exact path. The dominant nested calls were the OL3P solver, OLL, CDCL search, and repeated conversion of the working formula into CDCL form.

This produces four structural costs:

1. **Representation expansion:** a native package-version choice becomes many Boolean variables, clauses, and auxiliary variables.
2. **Repeated construction:** hydration or domain changes produce changed requests and repeated formula conversion/preprocessing.
3. **State loss:** learned clauses, watches, trails, activities, and objective relaxation state are not preserved end to end across the full refinement lifecycle.
4. **Weak reduction:** upper JAGR layers identify structure and obstruction but do not yet reduce the exact search space through independently verified native-domain proofs.

The replacement attacks these structural costs rather than tuning another local loop.

## 5. Correctness definition

"Without quality loss" has a precise meaning.

A JAGR result is correct only if all of the following hold:

1. Every root dependency required by the active environment is present.
2. Every selected version satisfies every applicable authoritative incoming range.
3. Every required transitive dependency is present at a compatible version.
4. Optional dependencies follow npm-compatible failure and platform behavior.
5. Required peer dependencies are present and compatible under Jhol's peer policy.
6. Optional peers never become required accidentally.
7. Overrides and resolutions are applied at the correct scope.
8. OS, CPU, engine, and prerelease rules are respected.
9. Lockfile and integrity information describe the selected manifests exactly.
10. The assignment is optimal under the requested preference mode, or belongs to the same deterministically ordered optimum class.
11. The same authoritative input produces the same lockfile regardless of thread scheduling, registry JSON key order, cache warmth, or hydration batch boundaries.
12. An independent verifier can replay the result certificate successfully.

If any item cannot be proven, the native engine cannot authorize output.

## 6. High-level architecture

The native engine consists of fifteen cooperating units:

1. **Domain and Range Cell compiler:** converts authoritative metadata into compact native domains and certified behavioral equivalence cells.
2. **Evidence and cohomology ledger:** stores append-only facts, manifest hashes, and replayable topology-preserving transformations.
3. **Dominance Antichain:** removes a cell only after proving support containment, absence of additional conflict, and objective non-inferiority.
4. **Proof-wave and bound propagator:** applies forward and backward domain reductions and objective obligations to a fixed point.
5. **Reason arena:** records why every cell was removed, package forced, bound derived, or topology transformed.
6. **Certified-shell builder:** removes fully proven packages from the active quotient.
7. **Proof Supernode builder:** contracts components only after crossing-edge and membrane-behavior equivalence replay.
8. **Obstruction-quotient decomposer:** isolates unresolved components and their certified boundaries.
9. **Certified Chebyshev-Hodge Funnel:** routes spectral work, performs certified reductions, constructs membrane-local exact bases, and isolates an irreducible residual core.
10. **Membrane Braid Join:** solves balanced knot halves and exhaustively joins compatible boundary proof signatures.
11. **Residual Core Basis:** performs exact sparse bitset rank maintenance only on Funnel leftovers.
12. **Conflict Membrane learner:** reuses verified contradiction boundaries under identical authoritative context.
13. **Adaptive Exact Portfolio:** selects propagation, decomposition, Braid Join, journal search, or residual-core solving by certified structure.
14. **Certificate verifier:** independently replays feasibility, topology conservation, decomposition, branch closure, and optimality.
15. **Shadow/fallback controller:** compares the native result with the old solver and controls rollout.

The normal data flow is:

```text
authoritative metadata
    -> append evidence
    -> compile/update Range Cells
    -> construct Dominance Antichain
    -> propagate proof and bound waves
    -> collapse certified shell
    -> contract Proof Supernodes
    -> decompose obstruction quotient and membranes
    -> run Certified Chebyshev-Hodge Funnel
    -> hydrate missing proof-support metadata when necessary
    -> choose exact portfolio engine per knot
    -> Braid Join balanced knots / journal-search tiny knots
    -> solve only the irreducible Residual Core Basis
    -> combine component proofs
    -> verify global certificate
    -> emit lockfile
```

No stage may skip the independent verifier.

### 6.1 Complexity symbols and claim boundary

The design uses these symbols consistently:

- `V`: authoritative package versions before Range Cell compression;
- `E`: typed dependency, peer, optional, policy, and crossing edges;
- `S`: compiled support incidences used by dominance indexes;
- `Q`: active quotient vertices after shell collapse;
- `M`: sparse simplices or non-zero boundary incidences in the active topological support;
- `n`: full active topological matrix dimension before certified reduction;
- `t`: fixed Chebyshev polynomial degree used for routing;
- `w`: maximum certified membrane boundary width;
- `K`: exact knot decision dimension after contractions;
- `d`: average cell branching width inside a knot;
- `r_c`: dimension of the irreducible exact residual core;
- `Delta`: number of changed residual boundary vectors;
- `W`: machine word width, initially 64 bits.

The topology target is:

```text
O(tM + M f(w) + r_c^3/W)
```

where the favorable local-basis path has `f(w) = w^2/W`, while exhaustive boundary-state stitching has conservative `f(w) = 2^w/W`. This is exact and linear in sparse support `M` when `t`, `w`, and `r_c` are bounded. It is not a claim that unrestricted exact cohomology is universally linear. If membrane width or residual-core dimension grows, telemetry exposes that growth and the exact fallback remains authoritative.

### 6.2 Approved strong-invention map

| Strong JAGR invention | Replaces weak part | Target complexity | Why exact / no quality loss | Expected effect |
|---|---|---:|---|---|
| **Certified Range Cells** | Searching every version separately | Build `O((V+E) log V)`; solve cells instead of versions | Split a cell whenever authoritative dependency, peer, platform, policy, manifest, or objective behavior differs | Compresses large version histories into semantic choices |
| **Dominance Antichain** | Blind candidate enumeration | Indexed target `O(S log V)`; dense worst `O(V^2/W)` | Every elimination carries a replayable support-containment and objective proof | Removes inferior choices before search |
| **Proof Supernodes** | Large obstruction quotient | `O(Q+E)` hashing plus linear verification | Crossing-edge and membrane-behavior equivalence are replayed before contraction | Shrinks exact knot dimension `K` |
| **Certified Membrane-Local Chebyshev-Hodge Funnel** | Full cubic topology matrices and broad rank work | `O(tM + M f(w) + r_c^3/W)` | Chebyshev only routes; exact reductions, local bases, membrane stitching, and residual rank authorize | Makes favorable topology work parameterized `O(M)` |
| **Cohomology Conservation Ledger** | Unverifiable topology reduction | `O(M alpha(n))` construction; linear replay | Every permitted contraction includes a homology-preserving witness and reversible accounting entry | Makes aggressive reduction independently safe |
| **Membrane Braid Join** | Exponential depth-first knot search | Balanced target `O(d^(K/2))` time and memory instead of `O(d^K)` | Both halves are exhaustive and the verifier checks every accepted boundary-signature join | Strongest exact hard-knot reduction |
| **Residual Core Basis** | Rank rebuilding over the full complex | Update `O(Delta r_c n/W)`; final dense core `O(r_c^3/W)` | Exact sparse bitset basis with no approximate answer or size cutoff | Confines cubic work to `r_c` |
| **Bidirectional Bound Braid** | Weak one-direction objective bounds | Initial `O(E+V)`; incremental `O(affected edges)` | A prune requires replayable forward and backward bounds | Closes losing branches earlier |
| **Conflict Membrane Learning** | Re-solving an equivalent contradiction | Learn `O(reason DAG)`; indexed lookup average `O(1)` plus replay | Reuse requires an identical authoritative membrane and verified reason DAG | Reuses structural conflicts without Boolean clauses |
| **Adaptive Exact Portfolio** | One engine for every graph shape | Structural scan `O(Q+E)` | Every selected engine emits the same independently checked proof format | Avoids a single engine's worst graph shape |

### 6.3 Complexity progression

| Topology path | Time complexity | Authority | Scaling interpretation |
|---|---:|---|---|
| Current full dense Hodge/cohomology work | `O(n^3)` | Exact | Cubic work follows the full matrix dimension |
| Earlier Chebyshev-Hodge Funnel | `O(tM + Mr + r^3/W)` | Exact only after residual proof | Near-linear when unresolved support rank `r` stays small, but `r` can grow with the graph |
| Certified Membrane-Local Funnel | `O(tM + M f(w) + r_c^3/W)` | Exact after conservation, stitching, and residual replay | Sparse work follows `M`; hard work is parameterized by membrane width `w` and residual core `r_c` |

The revision is not justified by a larger headline speed factor on already easy bounded-rank graphs. Its value is robustness: it prevents broad unresolved rank `r` from silently restoring full-complex cubic behavior, and it exposes the exact structural parameters responsible when favorable scaling does not occur. Whole-resolver speedup remains subject to measured stage share; no algorithmic-stage ratio is reported as an end-to-end benchmark result.

## 7. Native finite-domain model

### 7.1 Stable identifiers

Strings are parsed and interned outside the search loop. The live engine uses stable integer identifiers for:

- package names;
- versions;
- manifests;
- constraints;
- dependency edges;
- proof reasons;
- obstruction components;
- boundary states.

Identifiers remain stable while metadata is appended. New versions or packages extend the tables; existing identifiers never change.

### 7.2 Version ordering

Every package owns a deterministic table of available versions. The table retains:

- the original semver value;
- the JAGR ordinal;
- prerelease status;
- platform eligibility;
- manifest knowledge level;
- exact preference rank for each supported objective.

No floating-point value participates in correctness or deterministic ordering. Objective ranks use exact integers or fixed-width tuples.

### 7.3 Domain representation

A package domain is a bitset over its version table. Common operations become word operations:

- range filtering is bitwise intersection;
- conflict removal clears bits;
- domain union combines alternative support;
- forced selection is population count one;
- contradiction is population count zero;
- branch restoration replays journaled word changes.

Small domains may use inline words. Larger domains use contiguous arena-backed words. The abstraction must preserve one deterministic iteration order.

### 7.4 Compiled dependency support

For each known package version and dependency edge, the compiler stores a target-domain mask describing compatible versions. Required, optional, and peer edges remain distinct typed relations. Unknown target manifests do not block knowing the target version range, but unknown source manifests prevent that source version from becoming constraint-certified.

### 7.5 Certified Range Cells

Versions of one package may share a Range Cell only when they have identical authoritative behavior under every dimension that can affect resolution:

- required, optional, and peer support masks;
- override and resolution scope;
- OS, CPU, engine, and prerelease eligibility;
- lifecycle or package policy relevant to resolver semantics;
- manifest knowledge and authoritative hash context;
- canonical objective rank class for the active preference mode;
- lockfile-preservation behavior when an existing selection is present.

The compiler constructs canonical behavior signatures and sorts or indexes them in `O((V+E) log V)`. Hash equality proposes a cell; deterministic field-by-field comparison certifies it. A late manifest or policy fact that distinguishes two versions splits the cell without changing stable version identities. Search operates on cells, but the final certificate names the exact selected version and proves deterministic representative choice inside the winning cell.

### 7.6 Dominance Antichain

For cells `a` and `b` of the same package, `a` dominates `b` only when all of the following are proven in the current context:

1. Every required, optional, peer, platform, policy, and boundary support available to `b` is also available to `a`.
2. Choosing `a` introduces no conflict or obligation absent from `b`.
3. The active objective ranks `a` equal to or better than `b` under every still-reachable context.
4. Lockfile-preservation and deterministic tie-breaking do not prefer `b`.

The live candidate set is the antichain of non-dominated cells. Each removal stores the compared support bitsets, objective tuple, context root, and replay result. Failure to prove any condition retains `b`; dominance is never inferred from semver order alone.

## 8. Append-only evidence and conservation ledger

The evidence ledger is the source of truth for the live resolution.

Each evidence item contains:

- stable evidence identifier;
- evidence kind;
- source package and version when applicable;
- target package and range when applicable;
- manifest hash;
- required/optional/peer flags;
- override or platform context;
- generation first observed;
- authoritative provenance.

The ledger obeys three rules:

1. Facts are appended, never weakened or silently replaced.
2. A manifest update with a different authoritative hash creates a new context generation and invalidates dependent proof reuse.
3. A proof may depend only on evidence identifiers present in its context.

This allows hydration to extend one persistent problem without rebuilding its identity.

The same append-only discipline applies to topology transformations. A cohomology-conservation entry records the affected chains, the permitted reduction kind, its local witness, Betti accounting delta, membrane context, and inverse replay information. Spectral scores and hashes may locate a possible transformation, but only a validated conservation entry permits it to alter the exact complex.

## 9. Knowledge states

Each package version advances monotonically through:

```text
Unknown
  < VersionKnown
  < ManifestKnown
  < ConstraintsCompiled
  < ConstraintCertified
```

Meaning:

- `Unknown`: no usable version fact exists.
- `VersionKnown`: the version exists and may participate in a speculative domain.
- `ManifestKnown`: authoritative manifest payload and hash exist.
- `ConstraintsCompiled`: dependency, peer, optional, conflict, and platform relations are compiled.
- `ConstraintCertified`: every relation used to force or eliminate this version has passed independent replay.

Backward transitions are forbidden within a context generation. A changed manifest creates a new context rather than mutating an existing proof.

## 10. Bidirectional proof and bound braid

### 10.1 Forward wave

For each remaining source version, forward propagation computes which target versions support its required and peer edges. If a target package loses support, its domain shrinks.

### 10.2 Backward wave

When a target domain changes, backward propagation revisits only source versions whose support masks intersected the removed target versions. A source version with no remaining support for a required edge is eliminated.

### 10.3 Bidirectional objective bounds

The forward side computes the cheapest objective contribution of mandatory root closure through remaining cells. The backward side computes unavoidable peer, boundary, override, and policy obligations implied by every surviving target. A branch may be pruned only when their replayable combination cannot beat the incumbent. Initial construction is `O(E+V)`; updates touch affected edges. Either bound alone may prioritize work, but an uncertified or incomplete bound cannot close a branch.

### 10.4 Fixed point

Propagation continues until the work queue is empty. It may terminate with:

- a stable set of domains;
- a forced package;
- an empty domain and contradiction proof;
- a missing authoritative manifest request;
- a newly separable obstruction component.

### 10.5 Reason recording

Every domain change records a reason node. Primitive reasons include:

- root range;
- dependency range;
- required peer range;
- override/resolution;
- explicit conflict;
- platform rejection;
- engine rejection;
- missing required support;
- branch assumption;
- objective bound.

Composite reasons reference child reason identifiers in the arena. Reasons are immutable and hashable. They contain enough information for replay but avoid copying full manifest JSON.

## 11. Reversible domain journal

Exact search must not clone the whole resolver state.

Each branch checkpoint stores a journal offset. A journal entry contains:

- package identifier;
- bitset word index;
- prior word value;
- new word value;
- reason identifier;
- generation.

Backtracking restores entries in reverse order to the checkpoint. Queues and derived component state use generation stamps so stale entries can be ignored safely.

The journal is also useful for proof construction: the engine can identify every reduction produced under a branch assumption.

## 12. Certified shell

A package may leave the active obstruction quotient only when:

1. Its domain contains exactly one version.
2. That version is at least `ConstraintsCompiled`.
3. All required outgoing edges have non-empty certified support.
4. All required peers are satisfied.
5. No unresolved override, platform, or context condition touches it.
6. Its forcing reasons replay successfully.

The package and its proof become part of the certified shell. Shell packages remain visible as immutable boundary assignments but no longer participate in exact branching.

If later evidence invalidates a shell proof, the context generation changes and the affected shell component is reopened. Unrelated shell components remain reusable.

## 13. Proof Supernodes, obstruction quotient, and membranes

### 13.1 Proof Supernodes

Before constructing the final obstruction quotient, JAGR groups packages or already-reduced components by canonical membrane behavior. Equal hashes merely nominate a contraction. Certification then compares all internal typed relations, every crossing edge, boundary obligation, objective contribution, evidence generation, and policy context. Only components that are interchangeable at every observable boundary become a Proof Supernode.

Supernode construction targets `O(Q+E)` hashing plus linear verification of nominated groups. The proof preserves an expansion map to original packages and cells so the verifier and lockfile writer never lose identity. A failed equivalence check leaves the components separate.

### 13.2 Quotient contents

The active obstruction quotient contains only packages that remain relevant because of:

- multiple viable versions;
- conflicting incoming ranges;
- peer cycles;
- optional-to-required interactions;
- unknown authoritative metadata;
- objective-sensitive alternatives;
- membership in a replayable topological obstruction support;
- connection between two unresolved components.

### 13.3 Membrane

Each unresolved component has a membrane describing its interface with the certified exterior:

- fixed boundary assignments;
- incoming authoritative ranges;
- peer obligations crossing the boundary;
- objective contribution already fixed outside;
- manifest/context hashes;
- proof identifiers supporting the boundary.

Two components may be solved independently only when all paths between them pass through certified membrane assignments and neither objective nor peer constraint crosses the separation unresolved.

### 13.4 Decomposition proof

Component independence is itself certified. The proof enumerates crossing edges and shows that each is discharged by a fixed boundary assignment. The verifier rejects recombination if it finds an unaccounted unresolved crossing constraint.

### 13.5 Certified Membrane-Local Chebyshev-Hodge Proof Funnel

The Funnel replaces full-complex cubic cohomology work with five ordered stages:

1. **Certified chain/coreduction:** apply only topology-preserving reductions whose local free-pair, collapse, or equivalent admissibility conditions can be replayed. Record every transformation in the Cohomology Conservation Ledger.
2. **Chebyshev harmonic-support routing:** run a fixed-degree sparse Chebyshev recurrence over the reduced Hodge operator in `O(tM)`. It estimates where near-zero spectral support may remain and orders membranes and residual vectors. It never proves a Betti number, SAT, UNSAT, or branch closure.
3. **Certified membrane decomposition:** choose small separators over the reduced obstruction support and prove that every cross-membrane incidence is represented in its boundary signature.
4. **Exact membrane-local bases:** compute exact cycle, boundary, and cocycle bases locally using compact sparse/bitset coordinates, then carry canonical boundary signatures between membranes.
5. **Exact residual authorization:** exhaustively stitch boundary states and run exact rank maintenance over only the irreducible residual core.

The intended complexity is:

```text
certified reductions       O(M alpha(n))
Chebyshev routing          O(tM)
local exact basis          O(M w^2/W)         favorable
boundary-state stitching  O(M 2^w/W)         conservative
residual exact rank        O(r_c^3/W)
combined                   O(tM + M f(w) + r_c^3/W)
```

The implementation chooses the favorable local-basis path only when its proof shows that no combinatorial boundary-state enumeration is required. Otherwise it pays the conservative exhaustive term. There is no membrane-width cutoff that changes the answer: large `w` routes to another exact portfolio engine or fallback.

### 13.6 Cohomology Conservation Ledger

Each exact topology transformation stores:

- original and replacement chain identifiers;
- coefficient field and canonical orientation convention;
- boundary incidences before and after transformation;
- local admissibility witness;
- change, normally zero, to tracked cycle/boundary dimensions;
- membrane boundary map;
- evidence context root;
- deterministic inverse/replay recipe.

The verifier recomputes the local boundary identities and accumulated Betti accounting. Approximate eigenvalues, floating-point tolerances, or spectral gaps never appear in the authorization proof.

### 13.7 Residual Core Basis

Vectors surviving all certified reductions form the residual core. Exact bitset elimination maintains only changed boundary vectors, targeting `O(Delta r_c n/W)` incremental work and `O(r_c^3/W)` for a dense `r_c`-dimensional final core. The basis records pivots and source vectors so rank, dependence, and topology claims are replayable. No approximate vector is inserted into the authoritative basis.

## 14. Four-layer contracts

### 14.1 Layer 0: structural influence

Layer 0 supplies deterministic structural evidence:

- dependency-component membership;
- certified boundaries;
- articulation-like separator candidates;
- high-impact edges;
- prior proof-capsule matches;
- obstruction-pressure features.
- Certified Range Cell and Proof Supernode candidates.

Layer 0 may choose work order and propose decomposition. It cannot remove a version without a replayable native-domain reason.

### 14.2 Layer 3: tropical feasibility

Layer 3 operates on the unresolved quotient, not the full certified graph. It may produce:

- admissible objective lower-bound hints;
- infeasible direction candidates;
- tropical slack features;
- compact candidate obstruction supports.

A Layer 3 reduction becomes authoritative only after translation into native-domain evidence and verifier replay. Expensive algebraic analysis remains optional and budgeted. A skipped Layer 3 analysis never changes correctness.

Chebyshev recurrence belongs to the same advisory boundary: it may order support, reductions, or membranes, but cannot certify exact nullity or authorize a resolver decision.

### 14.3 Layer 2: sheaf gluing

Layer 2 identifies where locally valid choices may fail to glue globally. Its production output is a proposed minimal obstruction support over packages and boundary edges. The Certified Funnel turns an accepted support into exact membrane-local bases, exhaustive boundary signatures where required, and a residual-core proof.

The advisory support alone does not claim SAT or UNSAT. Exact membrane stitching and residual rank may become authoritative only through the conservation ledger and independent replay. Full diagnostic cohomology remains available through explicit analysis mode.

### 14.4 Layer 1: native proof engine

Layer 1 becomes the sole production authority before independent verification. It consumes:

- authoritative finite domains;
- replayable reductions;
- certified membranes;
- validated component decompositions;
- exact objective ranks.

It performs Range Cell selection, dominance replay, native propagation, portfolio selection, exact knot search, branch closure, component recombination, and global proof construction.

## 15. Native exact knot search

### 15.1 Entry condition

Exact branching begins only after propagation reaches a fixed point and no immediately useful authoritative hydration is pending.

### 15.2 Branch candidate ordering

The engine chooses a package using deterministic obstruction pressure. Pressure combines:

- remaining domain energy;
- boundary influence;
- peer coupling;
- tropical slack risk;
- historical obstruction frequency;
- expected information or branching cost.

Pressure is an integer tuple with deterministic tie-breaking by stable package identifier. It affects speed only.

### 15.3 Adaptive Exact Portfolio

After an `O(Q+E)` structural scan, the engine selects one exact strategy for each component:

- proof-wave propagation for forced shells;
- certified decomposition for separable components;
- Membrane Braid Join for balanced, boundary-sparse knots;
- reversible journal search for tiny or strongly asymmetric knots;
- Residual Core Basis for irreducible topological support.

Selection affects runtime only. All strategies consume the same authoritative domains and emit the same proof vocabulary. A component may move to another strategy if its structure changes, but already-certified work remains reusable through its membrane signature.

### 15.4 Version ordering

Candidate versions are tried in the exact deterministic preference order for the active objective. This finds a strong incumbent early but does not replace proof of optimality.

### 15.5 Membrane Braid Join

For a knot with decision dimension `K`, choose the smallest balanced certified membrane. Enumerate the left and right halves independently, producing canonical boundary proof signatures containing assignments, obligations, objective contributions, and contradiction roots. Join only compatible signatures and verify that the enumeration covers every half-assignment.

A balanced split targets `O(d^(K/2))` time and memory rather than `O(d^K)`. This is meet-in-the-middle exhaustive proof coverage, not heuristic pruning. If the boundary is too wide or the split is unbalanced, the portfolio chooses journal search or a different separator rather than claiming the target bound.

### 15.6 Branch lifecycle

For each candidate:

1. Record a journal checkpoint.
2. Restrict the package domain to the candidate.
3. Record a branch-assumption reason.
4. Run bidirectional proof waves.
5. Hydrate required authoritative proof support if permitted by the current budget.
6. Rebuild only affected quotient components.
7. Close contradiction branches with an empty-domain reason proof.
8. Recurse only into unresolved obstruction knots.
9. Record complete certified assignments as incumbents.
10. Restore the journal checkpoint.

### 15.7 Branch closure

A branch closes only through one of:

- replayable empty-domain contradiction;
- violated authoritative required edge;
- violated authoritative peer edge;
- certified decomposition component UNSAT;
- exact objective lower bound no better than the incumbent;
- verified memoized component proof under an identical membrane signature.
- verified Conflict Membrane proof under an identical authoritative boundary signature.

No timeout, heuristic score, or advisory mathematical result closes a branch.

## 16. Objective-bound engine

### 16.1 Objective representation

Objectives use deterministic integer tuples. The tuple may include, depending on mode:

- lockfile deviation count;
- direct dependency preference ranks;
- transitive dependency ranks;
- number of changed packages;
- package-count or deduplication penalties;
- explicit policy penalties.

The comparison order is part of the public resolver semantics and must be documented and tested.

### 16.2 Bounds

For every open branch or component, the Bidirectional Bound Braid combines a verified optimistic root-forward mandatory-closure bound with unavoidable peer/backward obligations from the membrane and surviving targets. A complete assignment supplies a certified upper bound.

The engine may stop only when every unexplored branch has either:

- a contradiction proof; or
- a lower bound that cannot improve the incumbent.

### 16.3 Optimality certificate

The final proof contains the incumbent objective tuple plus a forest of branch closures demonstrating that no better tuple remains reachable. The verifier recomputes comparisons using the canonical objective definition.

## 17. Boundary resonance memo and Conflict Membrane Learning

Solved obstruction components are memoized by a resonance signature containing:

- component structure hash;
- membrane assignment hash;
- active domain-mask hash;
- manifest and evidence context root;
- objective mode and policy hash.

The memo value contains:

- certified component assignment or UNSAT proof;
- local objective contribution;
- boundary assumptions;
- proof root;
- exact context generation.

Memo reuse requires signature equality and proof replay. Hash equality without replay is insufficient for authorization.

When a contradiction is discovered, the engine walks its immutable reason DAG and minimizes only by replay: a boundary fact is removed when the contradiction still verifies without it. The resulting Conflict Membrane contains a subset-minimal replay-proven authoritative boundary signature, its reason root, context root, and affected cell mask. It is not claimed globally minimum unless exhaustive minimization is explicitly performed and certified.

Learning costs `O(reason DAG)` plus replay work. Indexed candidate lookup is expected `O(1)`, but reuse still requires signature equality and verifier replay. This provides JAGR-native structural learning without translating the problem into Boolean clauses.

## 18. Information-gain hydration

The engine distinguishes search uncertainty from metadata uncertainty.

When authoritative metadata is missing, each candidate hydration receives a deterministic score derived from:

- number of unresolved edges it could compile;
- obstruction-knot membership;
- boundary influence;
- expected domain reduction;
- prior core/obstruction frequency;
- encoded metadata cost estimate.

Hydration uses bounded parallel batches, but results are committed to the evidence ledger in stable request order. A request failure for an optional package may be tolerated according to package semantics; a missing required authoritative manifest blocks certification and triggers fallback or error.

Crucially, hydration extends the same live domains, journal, reason arena, and component memo. It does not recreate the exact solver.

## 19. Independent certificate verifier

The verifier is intentionally simpler than the solver. It does not perform heuristic search.

It validates:

1. Context and manifest hashes.
2. Every Range Cell equivalence and deterministic representative selection.
3. Every dominance elimination and its support/objective containment proof.
4. Root requirement satisfaction.
5. Selected-version membership in authoritative domains.
6. Required dependency closure.
7. Peer dependency compatibility.
8. Optional dependency semantics.
9. Overrides, resolutions, platform, engine, and prerelease behavior.
10. Every domain-elimination reason used by the final proof.
11. Every certified-shell forcing proof.
12. Every Proof Supernode equivalence and expansion map.
13. Every conservation-ledger transformation and Betti accounting entry.
14. Every membrane-local basis and boundary-signature stitch.
15. Every Residual Core Basis pivot and rank claim.
16. Every component membrane and decomposition proof.
17. Exhaustive Braid Join half-coverage and accepted joins.
18. Every contradiction branch closure and learned Conflict Membrane reuse.
19. Every memoized proof used in the result.
20. The final objective tuple.
21. The bidirectional lower-bound closure of all potentially better branches.

The verifier returns only:

```text
Verified(solution, objective, proof_root)
Rejected(reason)
```

A rejected native result is never written to a lockfile.

## 20. Result states

The native engine has four explicit terminal states:

1. `CertifiedSolution`: complete assignment and verified objective proof.
2. `CertifiedUnsat`: authoritative contradiction proof covering all branches.
3. `NeedsMetadata`: exact list of missing authoritative proof-support manifests.
4. `Unfinished`: budget exhausted without a correctness conclusion.

`Unfinished` is not success and is not UNSAT. It routes to the old fallback during rollout.

## 21. Shadow oracle and fallback

### 21.1 Shadow mode

During initial rollout, the native engine runs alongside the current resolver. Comparisons distinguish:

- exact assignment equality;
- different assignments with equal canonical optimum;
- feasibility mismatch;
- objective mismatch;
- native certificate rejection;
- old-solver error or timeout.

### 21.2 Fallback

Fallback occurs when:

- native verification rejects;
- the native engine is unfinished;
- required metadata cannot be obtained;
- a feature is not yet supported natively;
- shadow policy requires old-solver confirmation.

Fallback is telemetry-visible and preserves current behavior. No native partial assignment contaminates fallback state unless it is passed only as a non-authoritative ordering hint.

### 21.3 Difficult-case collector

Every fallback records a privacy-safe structural capsule:

- obstruction-knot sizes;
- domain widths;
- constraint-kind counts;
- reason for fallback;
- time per stage;
- proof depth;
- no authentication tokens or raw private manifest contents.

This corpus guides later optimization.

## 22. Merkle proof capsules

Capsules evolve from hydration support into reusable proof support. They may contain:

- certified-shell assignments;
- evidence context root;
- manifest hashes;
- component resonance signatures;
- membrane proofs;
- domain-elimination proof roots;
- solved component assignments or UNSAT proofs;
- objective bounds;
- obstruction-support summaries.

Capsules are domain-separated, canonicalized, and Merkle-rooted. Imports require context equality and independent proof replay. Invalid or stale capsules are advisory misses. A capsule can reduce work but cannot change a result.

## 23. Rust implementation principles

The future implementation should exploit Rust through:

- contiguous package, version, edge, and domain tables;
- `u64` bitsets and population count;
- arena-backed immutable proof nodes;
- `SmallVec` for common short dependency and reason lists;
- generation counters instead of global clears;
- reversible word journals rather than full snapshots;
- scoped parallelism only for certified-independent components;
- stable ordered commit of parallel results;
- atomics for telemetry, not semantic state;
- zero string parsing or JSON traversal in the inner search loop;
- explicit memory and branch counters for benchmark attribution.
- interned canonical behavior signatures for Range Cells;
- word-parallel subset tests for dominance proofs;
- structure-of-arrays storage for sparse boundaries and membrane signatures;
- cache-aligned contiguous Chebyshev recurrence buffers reused across degrees;
- compact pivot-indexed residual bases using `u64` words;
- separate solver and verifier types so advisory spectral state cannot satisfy an exact-proof interface.

Unsafe code is excluded from the initial design. It may be considered only after profiler evidence, a written safety argument, Miri-compatible tests where applicable, and demonstrated material benefit.

## 24. Determinism

Determinism is a correctness property.

The engine must define stable order for:

- package identifiers;
- version identifiers;
- evidence commit;
- work queues;
- obstruction components;
- branch variables;
- candidate versions;
- proof children;
- component recombination;
- lockfile serialization.

Parallel tasks may finish in any order, but semantic commits occur in canonical order. Fixed-point and objective calculations use integers. Hash maps cannot determine externally visible ordering.

## 25. Error handling

Errors are classified as:

- registry/metadata unavailable;
- malformed authoritative manifest;
- unsupported semantic feature;
- certificate rejection;
- internal invariant violation;
- native budget exhausted;
- fallback failure;
- certified dependency conflict.

Internal invariant violations fail closed and trigger fallback where safe. Certificate rejection includes a compact stage and reason identifier. User-facing conflicts reconstruct explanations from proof reasons without exposing internal hashes unnecessarily.

## 26. Testing strategy

### 26.1 Unit tests

Unit tests cover:

- bitset-domain operations;
- stable version ordering;
- forward and backward propagation;
- optional and peer edge distinctions;
- reason replay;
- journal restoration;
- shell eligibility;
- membrane construction;
- decomposition verification;
- resonance-signature stability;
- objective tuple ordering;
- lower-bound soundness;
- capsule tamper rejection;
- deterministic pressure ordering.
- Range Cell split and merge certification;
- dominance proof replay and false-dominance rejection;
- Proof Supernode crossing-edge equivalence;
- conservation-ledger replay for every permitted reduction;
- Chebyshev routing invariance of final answers;
- exact membrane basis stitching;
- Residual Core Basis rank replay;
- Braid Join coverage and compatibility;
- Conflict Membrane context rejection;
- deterministic portfolio selection.

### 26.2 Exhaustive small universes

Generate small artificial registries where every assignment can be enumerated. Exhaustive enumeration becomes the independent ground truth for:

- SAT/UNSAT;
- selected objective optimum;
- multiple equivalent optima;
- peer cycles;
- overrides;
- optional-to-required transitions.

The native engine, verifier, and old solver are all compared to enumeration.

### 26.3 Differential fixtures

Run existing real-world fixtures through:

- current default resolver;
- native shadow resolver;
- independent certificate verifier;
- lockfile semantic comparison.

Different lockfile byte order is not automatically a semantic mismatch, but deterministic canonical output remains required for the promoted path.

### 26.4 Metamorphic tests

Equivalent inputs must remain equivalent when:

- registry object key order changes;
- package declaration order changes;
- independent packages are added;
- hydration batch size changes;
- thread count changes;
- caches are cold or warm;
- package names are consistently renamed;
- irrelevant versions are appended outside every active range.

### 26.5 Fault injection

Inject:

- missing manifests;
- stale hashes;
- corrupted capsules;
- network failures;
- interrupted hydration;
- verifier rejection;
- timeout during propagation or search;
- fallback failure.

The engine must never emit an uncertified lockfile.

## 27. Benchmark strategy

Benchmarks separate stage costs:

- metadata requests and bytes;
- domain compilation;
- propagation queue work;
- shell collapse count;
- quotient and knot sizes;
- branch count;
- journal writes;
- memo hits;
- proof construction;
- certificate verification;
- fallback time;
- total lockfile and install time.
- raw versions versus Certified Range Cells;
- dominance candidates, certified removals, and replay cost;
- Proof Supernode compression ratio;
- sparse topology size `M`, membrane width `w`, and residual dimension `r_c`;
- certified reduction and conservation-replay time;
- Chebyshev degree `t`, sparse operator passes, and routing time;
- local basis and boundary-state stitching time;
- Braid Join half-state counts, join candidates, and peak memory;
- Conflict Membrane learn, hit, and replay counts;
- portfolio selection and per-engine attribution.

Required workloads include:

- tiny forced graph;
- existing `overrides-app` fixture;
- Express;
- React;
- peer-heavy cycles;
- optional dependency graph;
- large workspace;
- generated wide domains;
- generated narrow but deeply cyclic obstruction knots.

Cold and warm runs use explicit cache definitions. Bun and npm comparisons must use equivalent operations and lockfile scope. Results include medians, P95, failure counts, and environment information.

## 28. Rollout gates

### Gate A: semantic foundation

- Domain compiler matches current authoritative semantics.
- No unknown manifest is treated as dependency-free.
- Stable identifiers and deterministic tables pass repeated tests.

### Gate B: propagation correctness

- Exhaustive small-universe parity is complete.
- Every elimination reason replays.
- Journal restoration is exact.

### Gate C: exactness

- Native knot search matches exhaustive optimum.
- Objective lower bounds are independently validated.
- Certified UNSAT proofs cover every branch.
- Range Cell, dominance, Supernode, conservation, membrane-stitching, and residual-rank proofs all replay independently.
- Disabling Chebyshev routing changes performance only, never the certified result.
- Braid Join and journal search return the same canonical optimum on exhaustive small universes.

### Gate D: real fixture shadowing

- No unexplained feasibility mismatch.
- No objective mismatch.
- No verifier rejection.
- Equivalent-optimum differences are classified correctly.

### Gate E: performance

- Current default median regression is at most 3%.
- P95 regression is at most 5%.
- Express completes below the existing 120-second guard.
- Proof verification is a small bounded fraction of total runtime.
- Native fallback rate decreases rather than hiding regressions.
- Range Cell and Supernode compression never increase promoted median beyond the gate.
- Topology benchmarks report scaling against `M`, `w`, and `r_c`; no universal linear-time claim is inferred from bounded-width fixtures.

### Gate F: controlled promotion

Rollout proceeds:

```text
off -> shadow 100% -> native 1% -> 10% -> 50% -> default
```

Any correctness mismatch returns rollout to shadow. Repeated performance regression returns rollout to off while preserving diagnostic data.

## 29. Planned implementation phases

This section defines architecture order, not the detailed task-by-task implementation plan.

Every phase adds and exercises the verifier fragment for the proof object introduced in that phase. Phase 11 assembles those already-tested fragments into global certificate verification; verification is not deferred until Phase 11.

### Phase 0: preserve oracle and baselines

Freeze current binaries, exact fixture outputs, objective semantics, and benchmark commands. Add no native behavior.

### Phase 1: compile native domains

Introduce stable identifiers, version tables, domain bitsets, compiled typed edges, and Certified Range Cells. Prove cell splits across every authoritative behavior dimension and compare expanded semantics without resolving.

### Phase 2: evidence and reasons

Introduce the append-only evidence ledger, Cohomology Conservation Ledger schema, immutable reason arena, proof hashing, and independent primitive-reason replay.

### Phase 3: persistent propagation

Implement bidirectional proof waves, Bidirectional Bound Braid, fixed-point queues, forced domains, contradictions, and reversible domain journals.

### Phase 4: exact candidate contraction

Implement the Dominance Antichain, word-parallel support-containment proofs, deterministic objective proofs, Proof Supernode nomination, crossing-edge verification, and expansion replay.

### Phase 5: certified shell and quotient

Implement shell eligibility, membrane boundaries, obstruction-quotient construction, component decomposition, and decomposition proof replay.

### Phase 6: Certified Chebyshev-Hodge Funnel

Implement certified chain/coreductions, conservation replay, sparse Hodge operators, fixed-degree Chebyshev routing, certified membrane decomposition, exact local bases, boundary-signature stitching, and the Residual Core Basis. Prove that disabling spectral routing cannot alter output.

### Phase 7: adaptive exact knots

Implement the Adaptive Exact Portfolio, deterministic journal search, Membrane Braid Join, exhaustive half-coverage proofs, component recombination, contradiction closure, and exact objective proof forests.

### Phase 8: structural proof reuse

Implement Conflict Membrane Learning, minimized replayable reason boundaries, resonance memo integration, and strict authoritative-context rejection.

### Phase 9: live hydration

Connect bounded parallel metadata hydration to the same persistent engine. Preserve identifiers, cells, proofs, domains, topology ledgers, and memo state across evidence generations. Split affected Range Cells and reopen only invalidated proof regions.

### Phase 10: four-layer proof contracts

Translate validated Layer 0, Layer 3, and Layer 2 outputs into replayable native-domain reductions, Funnel supports, decomposition proposals, or ordering hints. Keep spectral and otherwise unverifiable outputs advisory.

### Phase 11: verifier and proof capsules

Complete independent verification for Range Cells, dominance, Supernodes, topology conservation, membrane stitching, residual bases, Braid Join, objective closure, component proof reuse, Merkle capsules, tamper rejection, and stale-context invalidation.

### Phase 12: shadow and fallback rollout

Run native and old paths together, classify mismatches, collect difficult cases, and enforce automatic fallback.

### Phase 13: measured promotion

Promote only through correctness and performance gates. Retain the old solver as an explicit fallback until long-term evidence supports further removal.

## 30. Alternatives rejected

### Persistent generic CDCL

Keeping CNF and improving incremental state would lower risk but retain representation expansion and generic solver overhead. It remains useful as fallback work, not the target production architecture.

### Greedy-only resolution

Propagation and preferred-version selection are extremely fast on easy graphs but cannot prove difficult peer, cycle, conflict, and objective cases. Greedy remains a fast path inside the native engine, followed by proof.

### Mathematical layers as direct authority

Allowing tropical or sheaf analysis to finalize resolution would couple correctness to approximate or budgeted analysis. This design preserves those layers while requiring translation to replayable native-domain proof.

### Pure Chebyshev nullity estimation

Increasing polynomial degree can improve spectral localization but cannot, by itself, distinguish an exact zero eigenvalue from an arbitrarily small positive eigenvalue for unrestricted inputs. Chebyshev recurrence is therefore retained as `O(tM)` proof routing while exact local bases and residual rank authorize topology claims.

### Full incremental obstruction basis

Updating a full-complex sparse basis is preferable to rebuilding dense matrices, but it still carries broad coordinate width and can approach cubic work as obstruction support grows. The approved Funnel first performs certified reductions and membrane localization, then maintains exact rank only on `r_c`.

### Monolithic exact knot enumeration

Journaled depth-first search is appropriate for tiny or asymmetric knots but pays `O(d^K)` on balanced hard components. The portfolio retains it where appropriate and uses exhaustive Membrane Braid Join when a certified balanced boundary exists.

### Immediate deletion of OL3P/CDCL

Deleting the old path before exhaustive and shadow evidence would violate the no-quality-loss requirement. It is removed from the intended hot path but retained during rollout.

## 31. Risks and mitigations

### Risk: the native engine recreates a generic constraint solver poorly

Mitigation: keep the model package-native, typed, domain-oriented, and measured against exhaustive universes. Do not add generic clause abstractions to the production core.

### Risk: objective lower bounds are unsound

Mitigation: isolate objective semantics, use exact integer tuples, test bounds exhaustively, and require independent verifier replay. An unproven bound cannot prune.

### Risk: decomposition misses a crossing constraint

Mitigation: membranes enumerate every crossing typed edge and the verifier reconstructs crossings from authoritative evidence.

### Risk: metadata evolution invalidates proofs

Mitigation: proof context includes manifest hashes and evidence generation. A changed context reopens only affected components.

### Risk: proof construction erases speed gains

Mitigation: immutable compact reason nodes, structural sharing, arena allocation, and stage benchmarks. Proof is built from operations already required for correctness.

### Risk: parallelism causes nondeterminism

Mitigation: parallelize only certified-independent components and commit in canonical component order.

### Risk: difficult knots remain exponential

Mitigation: exact worst cases cannot be eliminated honestly. Reduce candidate width through Range Cells and dominance, reduce `K` through Supernodes and certified decomposition, use Braid Join for balanced knots, and retain exact fallback. Track `d`, `K`, separator balance, join memory, and fallback causes explicitly.

### Risk: behavioral cells merge versions that later diverge

Mitigation: signatures cover every authoritative semantic dimension, equality is verified beyond hashes, new evidence splits cells, and the final proof expands to an exact version.

### Risk: an unsound dominance relation removes the optimum

Mitigation: require support containment, no added obligation, context-sensitive objective non-inferiority, lockfile-policy compatibility, and independent replay. Uncertain pairs remain in the antichain.

### Risk: spectral approximation leaks into exact authority

Mitigation: type-separate advisory Chebyshev outputs from exact proof objects. The verifier accepts only conservation witnesses, exact bases, exhaustive stitches, and exact residual pivots. A test mode disables Chebyshev and requires identical results.

### Risk: membrane width defeats favorable scaling

Mitigation: expose `w`, use the conservative `2^w` complexity in planning, select another exact strategy when appropriate, and never truncate boundary states. Performance may fall back; correctness cannot.

### Risk: Braid Join uses excessive memory

Mitigation: canonical signature compaction, deterministic batching, externalized proof chunks when measured necessary, and portfolio fallback to journal search. A memory budget may change strategy but never discard states and claim completeness.

## 32. Invariants

The implementation must preserve these invariants:

1. Domains shrink monotonically within a branch and context generation.
2. Evidence grows monotonically within a context.
3. Every removed version has a replayable reason.
4. Every forced package has a replayable forcing proof.
5. Every shell package is constraint-certified.
6. Every component decomposition accounts for all crossing constraints.
7. Every branch closure is contradiction- or bound-certified.
8. Every memo reuse matches its membrane and context exactly.
9. Every emitted assignment passes the independent verifier.
10. Thread scheduling cannot alter semantic output.
11. Unknown metadata cannot authorize absence of constraints.
12. Timeouts produce `Unfinished`, never SAT or UNSAT.
13. Two versions share a Range Cell only under verified authoritative behavioral equivalence.
14. A dominated cell is removed only by replayable support and objective containment.
15. Every Proof Supernode has a verified expansion map and complete crossing-edge proof.
16. Every topology-changing reduction has a conservation-ledger witness.
17. Chebyshev output can order proof work but cannot create an authoritative proof node.
18. Every membrane stitch accounts for every boundary state required by its proof model.
19. Every Residual Core Basis claim is exact and pivot-replayable.
20. Every Braid Join proves exhaustive half-coverage and join compatibility.
21. Every Conflict Membrane reuse matches authoritative context and replays its reason DAG.
22. Portfolio selection cannot change accepted semantics or canonical optimum.

## 33. Observability

Telemetry should report, without affecting semantics:

- packages and versions compiled;
- domain words and memory;
- evidence generations;
- proof-wave operations;
- versions removed by reason kind;
- certified-shell size;
- quotient and knot sizes;
- component count;
- hydration requests and bytes;
- branch and backtrack counts;
- lower-bound prunes;
- memo hits and misses;
- verifier time;
- native result state;
- fallback reason;
- shadow comparison class.
- raw-version-to-Range-Cell compression;
- dominance-antichain width and proof cost;
- Proof Supernode compression;
- topology support `M`, Chebyshev degree `t`, membrane width `w`, and residual dimension `r_c`;
- conservation-ledger entries and replay time;
- Braid Join split balance, half states, joins, and peak memory;
- Conflict Membrane learn/hit/replay counts;
- exact portfolio choice and transitions.

Sensitive registry tokens, private manifest bodies, and filesystem secrets must never enter telemetry.

## 34. Acceptance criteria

The design is successfully implemented only when:

1. OL3P/CDCL is absent from the normal promoted production hot path.
2. The old solver remains available during the agreed rollout period.
3. Every native lockfile is independently certified.
4. Exhaustive small-universe testing finds no mismatch.
5. Existing compatibility and resolver snapshot suites pass.
6. Objective semantics are documented and deterministic.
7. Express completes below the current guard.
8. Default median and P95 performance gates pass.
9. Cache warmth, thread count, and hydration batch size do not change results.
10. Invalid proof capsules are harmless misses.
11. Fallback is visible, bounded, and never presented as native success.
12. Benchmark claims include reproducible commands and raw results.
13. Range Cell expansion reproduces the exact selected version and lockfile semantics.
14. Dominance, Supernode, topology, Braid Join, and learned-conflict proofs independently replay.
15. Chebyshev-enabled and Chebyshev-disabled runs produce identical certified results.
16. Favorable scaling claims are reported with measured `M`, `t`, `w`, and `r_c`; unrestricted worst cases are not mislabeled linear.
17. No topology approximation, membrane-width limit, or residual-size limit changes an answer.

## 35. Final architectural decision

JAGR will not abandon its four-layer identity. It will change what the layers produce and how exactness is achieved.

The approved target is:

```text
native version domains and Certified Range Cells
  + append-only evidence
  + Dominance Antichain
  + bidirectional proof and bound waves
  + certified shell
  + Proof Supernodes
  + obstruction quotient and certified membranes
  + Certified Membrane-Local Chebyshev-Hodge Proof Funnel
  + Cohomology Conservation Ledger
  + Adaptive Exact Portfolio
  + Membrane Braid Join
  + Residual Core Basis
  + Conflict Membrane Learning
  + objective proof forest
  + independent verification
```

Its central execution order is:

```text
Range Cells
  -> Dominance Antichain
  -> Proof Supernodes
  -> Certified Hodge Funnel
  -> Membrane Braid Join
  -> Residual Core
```

The target topology complexity is `O(tM + M f(w) + r_c^3/W)`, favorable parameterized `O(M)` when `t`, `w`, and `r_c` stay bounded. Balanced hard knots target `O(d^(K/2))` through exhaustive Braid Join rather than `O(d^K)` monolithic enumeration. These are structural targets to be validated by instrumentation, not benchmark claims made in advance.

Generic OL3P/CDCL moves out of the intended production hot path and remains temporarily as a shadow oracle, fallback, and difficult-case collector. Promotion occurs only after correctness and performance evidence satisfies the gates above.
