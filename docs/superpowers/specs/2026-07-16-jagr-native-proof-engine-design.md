# JAGR Native Proof Engine Design

**Status:** Approved revised architecture; implementation has not started
**Date:** 2026-07-16
**Product name:** JAGR
**Scope:** Replace generic OL3P/CDCL on JAGR's production resolution hot path while preserving the four-layer identity, exact resolution quality, deterministic output, and safe fallback behavior.

**Revision:** The approved native proof engine now uses a Dual-Geometry architecture. Contextual Instance Fibers and Living Range Crystals preserve npm placement and peer context. Exact topology uses certified low-width matrix decomposition and a Hodge Elimination Loom; exact resolver search uses Semantic Entropy Membranes, a JAGR Membrane Transfer Algebra, Continuation-Complete Crystals, an exact Braid dispatcher, Proof Orbit Folding, and a Proof Horizon Circuit. The two geometries have separate width parameters and proof systems. Chebyshev remains an optional routing accelerator, never an exactness authority.

## 1. Executive summary

JAGR currently converts the package-resolution problem into a generic Boolean optimization problem and delegates final authority to OL3P/CDCL. Profiling after the certified-lazy resolver work showed that metadata expansion, tropical Nullstellensatz construction, and Mayer-Vietoris cover construction were real costs, but they were not the terminal bottleneck. Once those costs were reduced, the exact OL3P/CDCL path dominated runtime. On the Express lockfile fixture, the old JAGR path, the new default path, and the refined path all reached a 120-second guard, while Bun 1.3.14 completed in 3.114 seconds.

The problem is therefore not the existence of JAGR's four mathematical layers. The problem is their current relationship with the exact solver. The upper layers mostly analyze or classify the graph, while the final layer still receives a large generic Boolean formula. Lazy metadata refinement can also cause repeated solves after the request fingerprint changes, losing much of the prior exact-search work.

This design replaces the production OL3P/CDCL path with a package-native exact resolver. It materializes only distinct placement and peer contexts, refines behaviorally equivalent versions through split-only Living Range Crystals, removes only proof-dominated cells, propagates constraints and bounds in both directions, and collapses forced packages into a certified shell. The unresolved problem then separates into two exact geometries. The Hodge side performs certified sparse elimination parameterized by the treewidth of the matrix incidence graph. The resolver side compiles components into exact membrane transfer maps parameterized by semantic boundary entropy, then uses continuation compression, exact lattice or zipper joins, orbit proofs, and shared horizon certificates. Every transformation carries a replayable reason, and a separate certificate verifier—not the solver itself—authorizes output.

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
11. Replace full-graph cubic topology work with exact field-sealed low-width matrix elimination while retaining an honest general exact fallback.
12. Reduce candidate count, semantic boundary entropy, compatible join output, and proof duplication before exact enumeration.
13. Model multiple package placements and peer environments without eagerly materializing their Cartesian product.
14. Separate polynomial bounded-width matrix work from irreducible combinatorial boundary-state work.
15. Minimize semantic information crossing a separator rather than merely minimizing its package count.

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
13. Present established mathematical results as original JAGR inventions; JAGR's invention is their proof-bearing package-native composition.
14. Hide decomposition discovery, compatible-join output, proof size, or objective-frontier growth from complexity accounting.

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

The native engine consists of seventeen cooperating units:

1. **Contextual Instance Fiber compiler:** materializes a package instance domain only for a distinct parent placement, workspace scope, or peer environment.
2. **Living Range Crystal compiler:** maintains split-only certified behavioral partitions with stable ancestry across metadata hydration.
3. **Evidence, reason, and conservation ledgers:** store append-only authoritative facts and replayable proof transformations.
4. **Dominance Antichain:** removes a crystal only after support, obligation, placement, policy, and objective containment replay.
5. **Proof-wave and bound propagator:** applies forward and backward domain reductions and objective obligations to a fixed point.
6. **Certified-shell builder:** removes fully proven fibers from the active quotient.
7. **Proof Supernode builder:** contracts components only after equality of their exact observable transfer behavior.
8. **Dual-Geometry planner:** independently measures Hodge matrix width and resolver semantic boundary entropy.
9. **Certified Hodge Elimination Loom:** performs exact low-width sparse elimination and emits a Two-Sided Rank Seal.
10. **JAGR Membrane Transfer Algebra:** represents a resolver component as an exact boundary-input to boundary-output relation with obligations, objective frontier, and proof root.
11. **Continuation-Complete Crystal reducer:** keeps only transfer states that preserve feasibility and optimum against every exterior continuation represented by the compatibility model.
12. **Exact Braid dispatcher:** selects Obligation Möbius Braid when subset-lattice structure is certified, otherwise Signature-Zipper Braid or journal search.
13. **Proof Orbit folder:** quotients only label-, edge-, policy-, placement-, and objective-preserving membrane symmetries.
14. **Conflict Membrane learner:** reuses verified contradiction boundaries under identical authoritative context.
15. **Proof Horizon Circuit:** shares contradiction and objective-bound proofs across covered domain regions.
16. **Certificate verifier:** independently replays feasibility, matrix rank, transfer composition, exhaustive coverage, and optimality.
17. **Shadow/fallback controller:** compares the native result with the old solver and controls rollout.

The normal data flow is:

```text
authoritative metadata
    -> append evidence
    -> materialize Contextual Instance Fibers
    -> split/update Living Range Crystals
    -> construct Dominance Antichain
    -> propagate proof and bound waves
    -> collapse certified shell
    -> measure both exact geometries
        -> Hodge incidence decomposition -> Elimination Loom -> Rank Seal
        -> Semantic Entropy Membranes -> Transfer Algebra
            -> Continuation-Complete Crystals
            -> Möbius Braid / Signature-Zipper / journal search
            -> Proof Orbit Folding
    -> build Proof Horizon Circuit
    -> combine component proofs
    -> verify global certificate
    -> emit lockfile
```

No stage may skip the independent verifier.

### 6.1 Dual-Geometry complexity symbols

The design uses these symbols consistently:

- `V`: authoritative package versions before Crystal compression;
- `F`: materialized Contextual Instance Fibers;
- `E_f`: typed dependency, peer, placement, optional, policy, and crossing edges between fibers;
- `S`: compiled support incidences used by dominance indexes;
- `Q`: active quotient vertices after shell collapse;
- `N_H`: Hodge matrix rows plus columns;
- `M_H`: non-zero Hodge boundary incidences;
- `k_H`: treewidth of the Hodge matrix's bipartite non-zero incidence graph;
- `T_decomp`: time to discover and certify a usable decomposition;
- `t_used`: optional Chebyshev routing degree actually consumed;
- `eta_R`: maximum resolver membrane semantic entropy, `sum(log2(cell_count))` over its fibers;
- `C`: retained continuation-complete transfer states;
- `C_build`: time to construct and certify transfer maps and representative states;
- `C_L`, `C_R`: retained transfer states on the two sides of a composition;
- `B`: canonical boundary signatures;
- `b`: number of Boolean obligation dimensions in a certified subset-lattice join;
- `P`: nondominated exact objective-frontier states;
- `P_work`: exact objective-frontier construction and dominance work;
- `J`: compatible output joins actually produced or aggregated;
- `R_orbit`: certified symmetry-orbit representatives;
- `T_orbit`: symmetry nomination, verification, and expansion-proof work;
- `H`: Proof Horizon Circuit nodes;
- `K`: exact knot decision dimension after contractions;
- `d`: average Crystal branching width inside a knot.

The favorable combined target is:

```text
O(S + F + E_f + T_decomp + k_H^2 N_H + t_used M_H
  + C_build + C_L + C_R + J + P_work + T_orbit + H)
```

This expression deliberately separates exact linear algebra from exact resolver search. With a supplied certified decomposition, exact rank over an arbitrary field targets `O(k_H^2 N_H)`. Resolver state remains exponential in semantic information in the unrestricted worst case: `C`, `P`, or `J` may be `2^Theta(eta_R)`. Decomposition discovery, join output, proof size, and frontier growth are never hidden.

### 6.2 Approved strong-invention map

| Strong JAGR invention | Replaces weak part | Target complexity | Why exact / no quality loss | Expected effect |
|---|---|---:|---|---|
| **Contextual Instance Fibers** | A single global package/version domain | `O(F+E_f)` over materialized contexts | Fibers merge only under verified parent, peer, workspace, and placement equivalence | Preserves npm tree semantics without eager Cartesian expansion |
| **Living Range Crystals** | Rebuilding cells after hydration | Initial `O(S+V log V)`; delta proportional to affected ancestry | Authoritative differences cause monotone splits with exact expansion maps | Reuses unaffected search and proofs |
| **Dominance Antichain** | Blind candidate enumeration | Indexed target `O(S log V)`; dense worst explicitly measured | Every elimination carries replayable support, obligation, placement, and objective containment | Removes inferior choices before search |
| **Proof Supernodes** | Re-solving transfer-equivalent components | Nomination `O(Q+E_f)` plus exact map replay | Contraction requires equality of all observable transfer behavior | Shrinks quotient while preserving exact expansion |
| **Dual-Width Membrane Geometry** | One width for unrelated hard problems | `O(Q+E_f)` structural measurement plus decomposition work | Hodge width and resolver entropy are independently certified | Prevents false linearity claims and chooses correct engine |
| **Certified Hodge Elimination Loom** | Cubic rank and exponential topology-state enumeration | `T_decomp + O(k_H^2 N_H)` with supplied low-width decomposition | Exact field-sealed elimination plus Two-Sided Rank Seal | Gives parameterized near-linear exact topology work |
| **Semantic Entropy Membranes** | Separators chosen by package count | Search cost parameter `eta_R = max sum log2(cell_count)` | All assignments remain represented | Minimizes actual boundary information |
| **Membrane Transfer Algebra** | Separate Supernode, memo, Braid, conflict, and objective interfaces | `C_build` plus exact composition cost | Transfer maps enumerate every certified boundary behavior | Unifies proof reuse and composition |
| **Continuation-Complete Crystals** | Keeping all partial assignments | `C <= 2^eta_R`; reduction cost exposed | Removal preserves compatibility and optimum for every represented continuation | Compresses exact DP tables |
| **Obligation Möbius Braid** | Direct subset-state joining | `O(b^2 2^b)` where exact subset-lattice preconditions hold | Exact transform and inversion certificate | Replaces applicable `O(3^b)` joins |
| **Signature-Zipper Braid** | Raw half-assignment storage | `O(C_L+C_R+J)` time; `O(B+P)` memory | Streams every compatible representative signature | Handles general typed boundaries honestly |
| **Proof Orbit Folding** | Re-solving certified symmetries | Proportional to `R_orbit` | Every quotient carries a verified structure-preserving permutation | Reduces repeated workspace and peer patterns |
| **Proof Dividend Scheduler** | Always running every accelerator | Structural scan plus deterministic measured work | Stopping routes unchanged state to another exact engine | Prevents mathematics overhead on easy graphs |
| **Bidirectional Bound Braid** | Weak one-direction objective bounds | Initial `O(E_f+F)`; incremental affected edges | A prune requires replayable forward and backward bounds | Closes losing branches earlier |
| **Conflict Membrane Learning** | Re-solving an equivalent contradiction | Learn `O(reason DAG)`; indexed lookup average `O(1)` plus replay | Reuse requires an identical authoritative membrane and verified reason DAG | Reuses structural conflicts without Boolean clauses |
| **Delta Proof Slicing** | Broad invalidation after hydration | `O(affected proof cone)` | Exact evidence dependencies decide reuse | Preserves unaffected Crystals and transfer proofs |
| **Proof Horizon Circuit** | Repeated branch-proof forests | `O(H + replay)` verification | Shared nodes represent identical authoritative domain regions | Compresses success, UNSAT, and optimality certificates |

### 6.3 Complexity progression

| Topology path | Time complexity | Authority | Scaling interpretation |
|---|---:|---|---|
| Current full dense Hodge/cohomology work | `O(N_H^3)` | Exact | Cubic work follows full matrix dimension |
| Previous single-width Funnel | `O(tM + M f(w) + r_c^3/W)` | Exact after residual proof | Conflates matrix width with resolver-state width |
| Hodge Elimination Loom | `T_decomp + O(k_H^2 N_H)` | Exact Two-Sided Rank Seal | Polynomial in matrix width and near-linear in matrix size when `k_H` is bounded |
| Resolver transfer geometry | `C_build+C_L+C_R+J+P_work+T_orbit+H` | Exact continuation, join, orbit, objective, and horizon proofs | Exponential dependence remains visible through `eta_R`, `C`, `P`, and `J` |

The revision removes an unnecessary exponential term from the linear-algebra side while refusing to hide the genuine combinatorial term on the resolver side. Whole-resolver speedup remains subject to measured stage share; no algorithmic-stage ratio is reported as an end-to-end benchmark result.

## 7. Contextual finite-domain model

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

Stable identities additionally cover placement scopes, workspace roots, peer environments, Fiber contexts, Crystal ancestry, transfer states, and proof orbits.

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

A Fiber's version domain is a bitset over its package's version table. Common operations become word operations:

- range filtering is bitwise intersection;
- conflict removal clears bits;
- domain union combines alternative support;
- forced selection is population count one;
- contradiction is population count zero;
- branch restoration replays journaled word changes.

Small domains may use inline words. Larger domains use contiguous arena-backed words. The abstraction must preserve one deterministic iteration order.

### 7.4 Compiled dependency support

For each known source Crystal version and typed Fiber edge, the compiler stores a target-domain mask describing compatible target versions in the relevant placement and peer context. Required, optional, and peer edges remain distinct typed relations. Unknown target manifests do not block knowing the target version range, but unknown source manifests prevent that source version from becoming constraint-certified.

### 7.5 Contextual Instance Fibers

A package instance requirement is not assumed to have one global resolver identity. JAGR creates a Fiber key from the package name, parent placement scope, workspace scope, peer environment, override context, and platform/policy context. Each Fiber owns a domain of possible versions; the version is deliberately not part of the Fiber key. A new Fiber is materialized only when one of these contexts is observably distinct. Two Fibers merge only after exact context equivalence replay.

This preserves multiple installed versions, peer-set behavior, and location-sensitive lockfile semantics without eagerly constructing every theoretical context combination. The output certificate expands each selected Fiber into its exact lockfile path and peer environment.

### 7.6 Living Range Crystals

Versions of one compatible Fiber context may share a Living Range Crystal only when they have identical authoritative behavior under every dimension that can affect resolution:

- required, optional, and peer support masks;
- override and resolution scope;
- OS, CPU, engine, and prerelease eligibility;
- lifecycle or package policy relevant to resolver semantics;
- manifest knowledge and authoritative hash context;
- canonical objective rank class for the active preference mode;
- lockfile-preservation behavior when an existing selection is present.

The compiler constructs canonical behavior signatures and sorts or indexes them in `O(S+V log V)`. Hash equality proposes a Crystal; deterministic field-by-field comparison certifies it. A late manifest or policy fact that distinguishes versions creates monotone child Crystals under a stable ancestry node. Unaffected children retain their domains, reasons, transfer summaries, and proof dependencies. Search operates on Crystals, but the final certificate names the exact selected Fiber and version and proves deterministic representative choice.

### 7.7 Dominance Antichain

For Crystals `a` and `b` of the same compatible Fiber context, `a` dominates `b` only when all of the following are proven in the current context:

1. Every required, optional, peer, platform, policy, and boundary support available to `b` is also available to `a`.
2. Choosing `a` introduces no conflict, placement, peer, or policy obligation absent from `b`.
3. The active objective ranks `a` equal to or better than `b` under every still-reachable context.
4. Lockfile-preservation and deterministic tie-breaking do not prefer `b`.

The live candidate set is the antichain of non-dominated Crystals. Each removal stores the compared support bitsets, objective tuple, context root, and replay result. Failure to prove any condition retains `b`; dominance is never inferred from semver order alone.

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

The forward side computes the cheapest objective contribution of mandatory root closure through remaining Crystals. The backward side computes unavoidable peer, boundary, override, and policy obligations implied by every surviving target. A branch may be pruned only when their replayable combination cannot beat the incumbent. Initial construction is `O(E_f+F)`; updates touch affected edges. Either bound alone may prioritize work, but an uncertified or incomplete bound cannot close a branch.

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

A Fiber may leave the active obstruction quotient only when:

1. Its domain contains exactly one version.
2. That version is at least `ConstraintsCompiled`.
3. All required outgoing edges have non-empty certified support.
4. All required peers are satisfied.
5. No unresolved override, platform, or context condition touches it.
6. Its forcing reasons replay successfully.

The Fiber and its proof become part of the certified shell. Shell Fibers remain visible as immutable boundary assignments but no longer participate in exact branching.

If later evidence invalidates a shell proof, the context generation changes and the affected shell component is reopened. Unrelated shell components remain reusable.

## 13. Proof Supernodes, obstruction quotient, and membranes

### 13.1 Proof Supernodes

Before constructing the final obstruction quotient, JAGR groups Fibers or already-reduced components by canonical membrane behavior. Equal hashes merely nominate a contraction. Certification compares their exact transfer maps: every boundary input must produce the same feasible outputs, obligations, canonical objective frontier, and expansion semantics. Only components indistinguishable to every represented exterior continuation become a Proof Supernode.

Supernode nomination targets `O(Q+E_f)` hashing; authorization additionally replays the complete nominated transfer maps. The proof preserves an expansion map to original Fibers and Crystals so the verifier and lockfile writer never lose identity. A failed equivalence check leaves the components separate.

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
- proof identifiers supporting the boundary;
- surviving Crystal count for every boundary Fiber;
- semantic entropy `sum(log2(cell_count))`;
- transfer-state and objective-frontier identities.

Two components may be solved independently only when all paths between them pass through certified membrane assignments and neither objective nor peer constraint crosses the separation unresolved.

Separators are ranked primarily by predicted semantic entropy, not vertex count. A larger membrane of forced Fibers can be cheaper than a smaller membrane containing several wide Crystal domains.

### 13.4 Decomposition proof

Component independence is itself certified. The proof enumerates crossing edges and shows that each is discharged by a fixed boundary assignment. The verifier rejects recombination if it finds an unaccounted unresolved crossing constraint.

### 13.5 Dual-Geometry planner

The planner builds two different certified structures:

1. The **Hodge incidence geometry** is the bipartite graph connecting matrix rows and columns at non-zero coefficients. Its structural parameter is `k_H`.
2. The **resolver transfer geometry** contains Contextual Instance Fibers and typed constraints. Its structural parameter is semantic membrane entropy `eta_R`, not `k_H` or raw package count.

The structures may suggest separators to each other, but neither width substitutes for the other. The planner records decomposition discovery time separately and verifies every produced bag, running-intersection property, matrix incidence, and resolver crossing edge.

### 13.6 Certified Hodge Elimination Loom

Given a certified decomposition of the Hodge incidence graph, the Loom performs exact field-sealed elimination bag by bag. Interior pivots are eliminated while the active frontier is bounded by `k_H`; pivot count, row operations, boundary projections, and kernel information are appended to the conservation ledger.

The target is:

```text
decomposition discovery       T_decomp
exact rank/elimination         O(k_H^2 N_H)
optional Chebyshev routing     O(t_used M_H)
```

Chebyshev may rank candidate decompositions or elimination regions in deterministic degree blocks. It can be skipped entirely and cannot create a pivot, rank claim, Betti claim, or resolver reduction. If no useful low-width decomposition is found, JAGR uses the exact general residual path and reports its cost rather than changing the answer.

### 13.7 Cohomology Conservation Ledger

Each exact topology transformation stores:

- original and replacement chain identifiers;
- coefficient field and canonical orientation convention;
- boundary incidences before and after transformation;
- local admissibility witness;
- change, normally zero, to tracked cycle/boundary dimensions;
- membrane boundary map;
- evidence context root;
- deterministic inverse/replay recipe.

The verifier recomputes local boundary identities, field operations, accumulated Betti accounting, and the decomposition conditions used by the Loom. Approximate eigenvalues, floating-point tolerances, or spectral gaps never appear in the authorization proof.

### 13.8 Two-Sided Rank Seal

Every rank `r` claim includes two independently replayable sides:

- a pivot/minor or elimination witness establishing rank at least `r`;
- a kernel, factorization, or span witness establishing rank at most `r`.

The proof is sealed to its coefficient field. `u64` XOR operations are authoritative only for `GF(2)`. Other configured fields use exact field-specific arithmetic. General residual elimination remains available when low-width elimination is not applicable; no modular guess or floating estimate authorizes rank.

## 14. Four-layer contracts

### 14.1 Layer 0: structural influence

Layer 0 supplies deterministic structural evidence:

- dependency-component membership;
- certified boundaries;
- articulation-like separator candidates;
- high-impact edges;
- prior proof-capsule matches;
- obstruction-pressure features;
- Living Range Crystal, Fiber, Proof Supernode, decomposition, and orbit candidates.

Layer 0 may choose work order and propose decomposition. It cannot remove a version without a replayable native-domain reason.

### 14.2 Layer 3: tropical feasibility

Layer 3 operates on the unresolved quotient, not the full certified graph. It may produce:

- admissible objective lower-bound hints;
- infeasible direction candidates;
- tropical slack features;
- compact candidate obstruction supports.

A Layer 3 reduction becomes authoritative only after translation into native-domain evidence and verifier replay. Expensive algebraic analysis remains optional and budgeted. A skipped Layer 3 analysis never changes correctness.

Chebyshev recurrence belongs to the same advisory boundary: it may order decomposition or elimination candidates, but cannot certify exact nullity or authorize a resolver decision.

### 14.3 Layer 2: sheaf gluing

Layer 2 identifies where locally valid choices may fail to glue globally. Its production output is a proposed obstruction support, Hodge incidence decomposition, or resolver membrane. The Hodge Loom turns an accepted matrix decomposition into exact elimination proofs; the Transfer Algebra turns an accepted resolver membrane into exact continuation semantics.

The advisory support alone does not claim SAT or UNSAT. Exact elimination, transfer composition, and exhaustive join coverage become authoritative only through their ledgers and independent replay. Full diagnostic cohomology remains available through explicit analysis mode.

### 14.4 Layer 1: native proof engine

Layer 1 becomes the sole production authority before independent verification. It consumes:

- authoritative finite domains;
- replayable reductions;
- certified membranes;
- validated component decompositions;
- exact objective ranks.

It performs Fiber materialization, Crystal refinement, dominance replay, native propagation, Dual-Geometry planning, exact transfer composition, knot search, branch closure, horizon construction, and global proof construction.

## 15. Native exact knot search

### 15.1 Entry condition

Exact branching begins only after propagation reaches a fixed point and no immediately useful authoritative hydration is pending.

### 15.2 Branch candidate ordering

The engine chooses a package using deterministic obstruction pressure. Pressure combines:

- remaining Crystal entropy;
- boundary influence;
- peer coupling;
- tropical slack risk;
- historical obstruction frequency;
- expected information or branching cost.

Pressure is an integer tuple with deterministic tie-breaking by stable Fiber and package identifiers. It affects speed only.

### 15.3 JAGR Membrane Transfer Algebra

A component transfer map has the canonical form:

```text
boundary input
  -> feasible boundary outputs
  + emitted peer/placement/policy obligations
  + nondominated canonical objective tuples
  + proof roots
```

Composition is associative under a fixed evidence context and canonical objective algebra. An empty output is a Conflict Membrane. Equal verified maps define a Proof Supernode. A resonance memo caches a map. Braid engines compose maps. This single algebra prevents different subsystems from inventing subtly incompatible boundary semantics.

### 15.4 Continuation-Complete Crystals

For each compatibility model, JAGR retains a subset of partial transfer states such that every represented exterior continuation that was feasible or optimal before reduction remains feasible or equally optimal afterward. A removed state carries a compatibility-preservation proof, not merely local dominance.

The retained size is `C`, with `C <= 2^eta_R`; no smaller bound is assumed universally. Reduction is enabled only when its exact representative proof costs less than retaining the original states under the Proof Dividend rule.

### 15.5 Exact Braid dispatcher

After a structural scan, the engine selects one exact strategy for each component:

- proof-wave propagation for forced shells;
- certified decomposition for separable components;
- Obligation Möbius Braid for certified subset-lattice boundaries;
- Signature-Zipper Braid for general typed boundaries;
- reversible journal search for tiny or strongly asymmetric knots;
- general exact fallback when compression preconditions fail.

Selection affects runtime only. All strategies consume the same authoritative domains and emit the same proof vocabulary. A component may move to another strategy if its structure changes, but already-certified work remains reusable through its membrane transfer signature.

### 15.6 Proof Dividend Scheduler and Adaptive Chebyshev Pulse

Optional accelerators spend from a deterministic exploration allowance and continue only while measured certified reduction, reusable transfer compression, or eliminated exact work repays their cost. This is a performance rule, never a semantic cutoff: stopping an accelerator routes the unchanged authoritative state to another exact engine.

Chebyshev runs in deterministic degree blocks and stops when additional blocks produce no measured routing dividend. Its work is `O(t_used M_H)` with `0 <= t_used <= t_max`; `t_used = 0` is valid and must produce the same certified result.

### 15.7 Obligation Möbius Braid

When the verifier confirms that boundary compatibility is exactly a subset-lattice partition or cover relation and the objective encoding supports exact transform arithmetic, JAGR composes all boundary subsets through an exact transform and inversion certificate. The target is `O(b^2 2^b)` arithmetic operations instead of direct `O(3^b)` subset splitting.

The transform is not used for arbitrary peer, placement, or policy relations. Failed algebraic preconditions route to Signature-Zipper Braid without semantic loss.

### 15.8 Signature-Zipper Braid

General boundaries are sorted by canonical transfer signature. Each side aggregates assignments only when boundary behavior and Pareto objective frontier are exactly equivalent. The zipper streams every compatible signature pair.

Its honest complexity is `O(C_L+C_R+J)` time and `O(B+P)` memory, where `J` includes compatible outputs; it does not claim universal `O(d^(K/2))` when join output itself is larger.

### 15.9 Proof Orbit Folding

Before joining, JAGR may quotient membrane states by a verified permutation preserving package labels or declared equivalence classes, Crystal behavior, typed edges, placement and peer contexts, policies, evidence hashes, and objective ranks. The proof contains permutation generators and an expansion map. Hash similarity alone never creates an orbit.

### 15.10 Version ordering

Candidate versions are tried in the exact deterministic preference order for the active objective. This finds a strong incumbent early but does not replace proof of optimality.

### 15.11 Branch lifecycle

For each candidate:

1. Record a journal checkpoint.
2. Restrict the Fiber domain to the candidate.
3. Record a branch-assumption reason.
4. Run bidirectional proof waves.
5. Hydrate required authoritative proof support if permitted by the current budget.
6. Rebuild only affected quotient components.
7. Close contradiction branches with an empty-domain reason proof.
8. Recurse only into unresolved obstruction knots.
9. Record complete certified assignments as incumbents.
10. Restore the journal checkpoint.

### 15.12 Branch closure

A branch closes only through one of:

- replayable empty-domain contradiction;
- violated authoritative required edge;
- violated authoritative peer edge;
- certified decomposition component UNSAT;
- exact objective lower bound no better than the incumbent;
- verified memoized component proof under an identical membrane signature;
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

A component does not emit one scalar local cost when placement, deduplication, package count, or lockfile preservation can couple it to another component. For each boundary signature it emits a Pareto Objective Weave containing every nondominated canonical objective tuple. Composition combines tuples exactly and removes one only through componentwise dominance plus canonical tie-break proof. Frontier size `P` is measured and may be exponential in the worst case.

The engine may stop only when every unexplored branch has either:

- a contradiction proof; or
- a lower bound that cannot improve the incumbent.

### 16.3 Proof Horizon optimality certificate

The final proof contains the incumbent objective tuple plus a shared Proof Horizon Circuit. A horizon node covers a canonical domain region and references either a contradiction proof, a bound proving the entire region cannot improve the incumbent, or exact children whose regions partition it. Hash-consing shares identical authoritative subproofs, but hashes never replace replay. The verifier checks complete region coverage and recomputes comparisons using the canonical objective definition.

## 17. Transfer memo, Conflict Membrane Learning, and Delta Proof Slicing

Solved obstruction components are memoized by a resonance signature containing:

- component structure hash;
- membrane assignment hash;
- active domain-mask hash;
- manifest and evidence context root;
- objective mode and policy hash.

The memo value contains:

- certified component assignment or UNSAT proof;
- exact Pareto objective frontier;
- boundary assumptions;
- proof root;
- exact context generation.

Memo reuse requires signature equality and proof replay. Hash equality without replay is insufficient for authorization.

When a contradiction is discovered, the engine walks its immutable reason DAG and minimizes only by replay: a boundary fact is removed when the contradiction still verifies without it. The resulting Conflict Membrane contains a subset-minimal replay-proven authoritative boundary signature, its reason root, context root, and affected cell mask. It is not claimed globally minimum unless exhaustive minimization is explicitly performed and certified.

Learning costs `O(reason DAG)` plus replay work. Indexed candidate lookup is expected `O(1)`, but reuse still requires signature equality and verifier replay. This provides JAGR-native structural learning without translating the problem into Boolean clauses.

Every proof and transfer node also stores an exact compact dependency set of evidence identifiers. When hydration splits a Crystal or changes an authoritative manifest context, JAGR follows reverse dependency links and invalidates only the affected proof cone. Unrelated Fibers, transfer maps, rank seals, or horizon nodes remain reusable. Bloom filters or probabilistic indexes may nominate affected nodes but cannot authorize retaining a proof; exact dependency membership decides reuse.

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

Crucially, hydration extends the same Fibers, Crystal ancestry, domains, journal, reason arena, decomposition certificates, transfer maps, and proof dependency graph. It does not recreate the exact solver. Unknown metadata is treated as an unresolved membrane boundary and cannot authorize absence of constraints.

## 19. Independent certificate verifier

The verifier is intentionally simpler than the solver. It does not perform heuristic search.

It validates:

1. Context and manifest hashes.
2. Every Contextual Instance Fiber identity, merge, placement, and peer-context expansion.
3. Every Living Range Crystal equivalence, split ancestry, and deterministic representative selection.
4. Every dominance elimination and its support, obligation, placement, and objective containment proof.
5. Root requirement satisfaction.
6. Selected-version membership in authoritative domains.
7. Required dependency closure.
8. Peer dependency compatibility and placement.
9. Optional dependency semantics.
10. Overrides, resolutions, platform, engine, and prerelease behavior.
11. Every domain-elimination reason used by the final proof.
12. Every certified-shell forcing proof.
13. Every Proof Supernode transfer-map equivalence and expansion map.
14. Every Hodge decomposition bag, incidence, and running-intersection condition.
15. Every conservation-ledger transformation and field-sealed row operation.
16. Both sides of every rank seal and resulting Betti accounting.
17. Every Semantic Entropy Membrane and crossing constraint.
18. Every Transfer Algebra composition and Continuation-Complete reduction.
19. Every Möbius precondition, transform/inversion certificate, or Zipper-compatible output.
20. Every Proof Orbit permutation and expansion.
21. Every contradiction closure and learned Conflict Membrane reuse.
22. Every memoized proof and exact evidence dependency used in the result.
23. Every Pareto Objective Weave reduction and final objective tuple.
24. Complete Proof Horizon region coverage and bidirectional bound closure.

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
- explicit memory and branch counters for benchmark attribution;
- interned Fiber keys, Crystal ancestry, and canonical transfer signatures;
- word-parallel subset tests for dominance proofs;
- structure-of-arrays storage for sparse matrix incidence, decomposition bags, and membrane signatures;
- cache-aligned contiguous Chebyshev recurrence buffers reused across degrees;
- coefficient-sealed pivot storage using `u64` words only for `GF(2)`;
- compact transfer rows, Pareto frontiers, orbit maps, and Horizon Circuit nodes;
- exact evidence-dependency sets and reverse invalidation indexes;
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
- decomposition bags;
- Crystal splits and Fiber materialization;
- transfer rows, Pareto tuples, and orbit representatives;
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
- deterministic pressure ordering;
- Fiber materialization, merge, placement, and peer-context certification;
- Living Range Crystal split ancestry and expansion;
- dominance proof replay and false-dominance rejection;
- Proof Supernode transfer-map equivalence;
- conservation-ledger replay for every permitted reduction;
- Chebyshev routing invariance of final answers;
- Hodge decomposition certification and malformed-bag rejection;
- Two-Sided Rank Seal replay over each supported coefficient field;
- semantic entropy calculation;
- Transfer Algebra identity, associativity, and composition under fixed context;
- Continuation-Complete reduction and false-representative rejection;
- Möbius transform precondition and inversion replay;
- Signature-Zipper output accounting;
- orbit preservation and false-symmetry rejection;
- Pareto Objective Weave composition;
- Proof Horizon coverage and overlap rejection;
- Conflict Membrane context rejection;
- Delta Proof Slicing precision;
- deterministic exact-engine selection.

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
- total lockfile and install time;
- theoretical contexts versus materialized Fibers;
- raw versions versus Living Range Crystals;
- dominance candidates, certified removals, and replay cost;
- Proof Supernode compression ratio;
- Hodge dimensions `N_H`, nonzeros `M_H`, decomposition time, and width `k_H`;
- resolver semantic entropy `eta_R` separately from raw boundary size;
- certified reduction and conservation-replay time;
- Chebyshev `t_used`, skipped pulses, and measured dividend;
- transfer construction, Continuation-Complete size `C`, and Pareto size `P`;
- Möbius and Zipper dispatch counts, compatible output `J`, and peak memory;
- Proof Orbit candidate, verified, and representative counts;
- Proof Horizon raw branches versus shared nodes `H`;
- exact proof-cone invalidation size;
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
- generated narrow but deeply cyclic obstruction knots;
- repeated workspace/peer structures with valid and invalid symmetries;
- low-`k_H` and high-`k_H` matrices of equal `N_H`;
- equal-width resolver membranes with sharply different `eta_R`;
- worst-case compatible joins and growing Pareto frontiers.

Cold and warm runs use explicit cache definitions. Bun and npm comparisons must use equivalent operations and lockfile scope. Results include medians, P95, failure counts, and environment information.

## 28. Rollout gates

### Gate A: semantic foundation

- Domain compiler matches current authoritative semantics.
- No unknown manifest is treated as dependency-free.
- Stable identifiers and deterministic tables pass repeated tests.
- Fiber placement and peer-context expansion match lockfile tree semantics.
- Crystal hydration splits preserve unaffected proof state.

### Gate B: propagation correctness

- Exhaustive small-universe parity is complete.
- Every elimination reason replays.
- Journal restoration is exact.

### Gate C: exactness

- Native knot search matches exhaustive optimum.
- Objective lower bounds are independently validated.
- Certified UNSAT proofs cover every branch.
- Fiber, Crystal, dominance, Supernode, Hodge decomposition, rank-seal, transfer, continuation, orbit, and Horizon proofs replay independently.
- Disabling Chebyshev routing changes performance only, never the certified result.
- Möbius, Zipper, journal search, and exhaustive enumeration return the same canonical optimum on small universes.

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
- Fiber, Crystal, transfer, and orbit machinery never increase promoted median beyond the gate.
- Hodge benchmarks report `T_decomp`, `N_H`, `M_H`, and `k_H`.
- Resolver benchmarks report `eta_R`, `C`, `P`, `J`, and `H`; no universal linear-time claim is inferred from bounded-entropy fixtures.

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

Introduce stable identifiers, Contextual Instance Fibers, Living Range Crystals, compiled typed edges, and exact placement/peer expansion. Prove Crystal splits across every authoritative behavior dimension and compare expanded semantics without resolving.

### Phase 2: evidence and reasons

Introduce the append-only evidence ledger, Cohomology Conservation Ledger schema, immutable reason arena, exact evidence-dependency sets, reverse proof slicing, proof hashing, and independent primitive-reason replay.

### Phase 3: persistent propagation

Implement bidirectional proof waves, Bidirectional Bound Braid, fixed-point queues, forced Fibers, contradictions, and reversible domain journals.

### Phase 4: exact candidate contraction

Implement the Dominance Antichain with placement/peer obligations, word-parallel support containment, deterministic objective proofs, and preliminary Proof Supernode nomination.

### Phase 5: certified shell and quotient

Implement shell eligibility, obstruction quotient construction, Dual-Geometry measurement, Hodge incidence graphs, Semantic Entropy Membranes, and independent decomposition proof replay.

### Phase 6: Certified Hodge Elimination Loom

Implement coefficient-sealed sparse elimination over certified decomposition bags, conservation replay, general exact fallback, Two-Sided Rank Seals, and Adaptive Chebyshev Pulse. Prove that disabling spectral routing cannot alter output.

### Phase 7: Membrane Transfer Algebra

Implement canonical transfer maps, Pareto Objective Weaves, exact composition, Continuation-Complete Crystals, transfer-equivalent Proof Supernodes, and resonance memo integration.

### Phase 8: exact Braid and proof compression

Implement the Proof Dividend Scheduler, Obligation Möbius Braid with precondition and inversion certificates, Signature-Zipper Braid with explicit `J`, deterministic journal fallback, Proof Orbit Folding, Conflict Membrane Learning, and Proof Horizon Circuits.

### Phase 9: live hydration

Connect bounded parallel metadata hydration to the same persistent engine. Preserve Fibers, Crystal ancestry, proofs, decompositions, transfer maps, and Horizon nodes across evidence generations. Split affected Crystals and reopen only exact reverse dependency cones.

### Phase 10: four-layer proof contracts

Translate validated Layer 0, Layer 3, and Layer 2 outputs into replayable Fiber reductions, Hodge decompositions, resolver membranes, transfer proposals, or ordering hints. Keep spectral and otherwise unverifiable outputs advisory.

### Phase 11: verifier and proof capsules

Complete independent verification for Fibers, Crystals, dominance, Hodge decompositions, rank seals, transfer composition, continuation reduction, Möbius/Zipper joins, orbit proofs, Pareto objectives, Horizon coverage, Merkle capsules, tamper rejection, and stale-context invalidation.

### Phase 12: shadow and fallback rollout

Run native and old paths together, classify mismatches, collect difficult cases, and enforce automatic fallback.

### Phase 13: measured promotion

Promote only through correctness and performance gates. Retain the old solver as an explicit fallback until long-term evidence supports further removal.

### 29.1 Research basis and JAGR originality boundary

The complexity boundaries are informed by primary research:

- Fürer, Hoppen, and Trevisan, *Fast Gaussian Elimination for Low Treewidth Matrices* (ESA 2025), proves `O(k^2(m+n))` exact rank and elimination over an arbitrary field when a suitable tree decomposition is supplied: <https://doi.org/10.4230/LIPIcs.ESA.2025.116>.
- Björklund, Husfeldt, Kaski, and Koivisto, *Fourier Meets Möbius: Fast Subset Convolution*, gives exact `O(b^2 2^b)` subset convolution rather than direct `O(3^b)` evaluation: <https://doi.org/10.1145/1250790.1250801>.
- Groenland, Mannens, Nederlof, Piecyk, and Rzazewski, *Towards Tight Bounds for the Graph Homomorphism Problem Parameterized by Cutwidth via Asymptotic Matrix Parameters* (ICALP 2024), formalizes continuation-preserving representative sets and matching exponential compatibility limits: <https://doi.org/10.4230/LIPIcs.ICALP.2024.77>.
- Samer and Szeider, *Constraint Satisfaction with Bounded Treewidth Revisited*, establishes that structural width, domain size, arity, and the chosen constraint graph jointly govern tractability: <https://doi.org/10.1016/j.jcss.2009.04.003>.

JAGR does not claim those theorems as inventions. Its original contribution is the package-native Dual-Geometry composition: Fiber/Crystal semantics, field-sealed rank proofs, semantic entropy membranes, typed dispatch between exact join algebras, continuation certificates, orbit proofs, hydration-aware proof slicing, and one independent verifier spanning all four JAGR layers.

## 30. Alternatives rejected

### Persistent generic CDCL

Keeping CNF and improving incremental state would lower risk but retain representation expansion and generic solver overhead. It remains useful as fallback work, not the target production architecture.

### Greedy-only resolution

Propagation and preferred-version selection are extremely fast on easy graphs but cannot prove difficult peer, cycle, conflict, and objective cases. Greedy remains a fast path inside the native engine, followed by proof.

### Mathematical layers as direct authority

Allowing tropical or sheaf analysis to finalize resolution would couple correctness to approximate or budgeted analysis. This design preserves those layers while requiring translation to replayable native-domain proof.

### Pure Chebyshev nullity estimation

Increasing polynomial degree can improve spectral localization but cannot, by itself, distinguish an exact zero eigenvalue from an arbitrarily small positive eigenvalue for unrestricted inputs. Chebyshev is therefore optional `O(t_used M_H)` routing while the Hodge Loom and Two-Sided Rank Seal authorize topology claims.

### Single-width Chebyshev-Hodge Funnel

Using one `w` for matrix elimination and resolver-state enumeration conflates polynomial bounded-width linear algebra with genuinely exponential finite-domain compatibility. The approved design separates `k_H` from `eta_R` and applies different exact mathematics to each.

### Monolithic exact knot enumeration

Journaled depth-first search is appropriate for tiny or asymmetric knots but pays `O(d^K)` on hard components. The portfolio retains it where appropriate and uses exact Möbius or Signature-Zipper composition when their certified structure is better.

### Universal meet-in-the-middle bound

Claiming `O(d^(K/2))` ignores compatible join output and Pareto-frontier growth. The approved Zipper complexity includes `J`, while Möbius acceleration is used only under exact subset-lattice preconditions.

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

### Risk: difficult resolver knots remain exponential

Mitigation: exact worst cases cannot be eliminated honestly. Reduce semantic entropy through Crystals, dominance, continuation representatives, transfer-equivalent Supernodes, and orbit proofs. Track `eta_R`, `C`, `P`, `J`, and fallback causes explicitly.

### Risk: Fibers or Crystals merge contexts that later diverge

Mitigation: Fiber keys cover placement and peer environment; Crystal signatures cover every authoritative semantic dimension; equality is verified beyond hashes; new evidence creates monotone splits; final proof expands exact locations and versions.

### Risk: an unsound dominance relation removes the optimum

Mitigation: require support containment, no added obligation, context-sensitive objective non-inferiority, lockfile-policy compatibility, and independent replay. Uncertain pairs remain in the antichain.

### Risk: spectral approximation leaks into exact authority

Mitigation: type-separate advisory Chebyshev outputs from exact proof objects. The verifier accepts only decomposition, elimination, transfer, coverage, and Two-Sided Rank proofs. A test mode disables Chebyshev and requires identical results.

### Risk: decomposition discovery erases Hodge gains

Mitigation: report `T_decomp` separately, reuse certified decompositions by exact context, stop optional discovery under Proof Dividend rules, and route to general exact elimination. Never quote `O(k_H^2 N_H)` without the decomposition assumption.

### Risk: transfer joins or objective frontiers explode

Mitigation: account for `C`, `P`, and `J`; use continuation proofs, exact orbit folding, streaming Zipper composition, and journal fallback. A memory budget may change strategy but never discard states and claim completeness.

### Risk: coefficient-field mismatch invalidates topology

Mitigation: seal matrix and proof types to the coefficient field, use XOR bitsets only for `GF(2)`, and require exact field-specific replay for every other field.

### Risk: continuation compression loses a future optimum

Mitigation: a Crystal is removed only by a proof covering every continuation represented by the compatibility model and every nondominated objective tuple. Unsupported relation shapes retain all states.

### Risk: symmetry folding changes package identity

Mitigation: verify every permutation against labels or declared equivalence classes, Fibers, typed edges, policies, evidence, and objective ranks; preserve an exact expansion map.

## 32. Invariants

The implementation must preserve these invariants:

1. Domains shrink monotonically within a branch and context generation.
2. Evidence grows monotonically within a context.
3. Every removed version has a replayable reason.
4. Every forced Fiber has a replayable forcing proof.
5. Every shell Fiber is constraint-certified.
6. Every component decomposition accounts for all crossing constraints.
7. Every branch closure is contradiction- or bound-certified.
8. Every memo reuse matches its membrane and context exactly.
9. Every emitted assignment passes the independent verifier.
10. Thread scheduling cannot alter semantic output.
11. Unknown metadata cannot authorize absence of constraints.
12. Timeouts produce `Unfinished`, never SAT or UNSAT.
13. Fibers merge only under verified placement, workspace, peer, override, platform, and policy context equivalence.
14. Versions share a Crystal only under verified authoritative behavioral equivalence and stable split ancestry.
15. A dominated Crystal is removed only by replayable support, obligation, placement, and objective containment.
16. Every Proof Supernode has an equal verified transfer map and exact expansion map.
17. Hodge and resolver geometries never substitute one width parameter for the other.
18. Every topology-changing reduction has a conservation-ledger witness.
19. Every Hodge rank claim has both lower- and upper-bound witnesses sealed to its field.
20. Chebyshev output can order proof work but cannot create an authoritative proof node.
21. Every transfer reduction preserves every represented continuation and objective optimum.
22. Every Möbius use proves its algebraic preconditions and inversion; every Zipper accounts for `J`.
23. Every orbit has a verified preserving permutation and expansion.
24. Every Conflict Membrane reuse matches authoritative context and replays its reason DAG.
25. Every Proof Horizon node covers its complete declared domain region.
26. Portfolio selection and budgets cannot change accepted semantics or canonical optimum.

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
- shadow comparison class;
- theoretical versus materialized Fiber contexts;
- raw-version-to-Crystal compression and split ancestry reuse;
- dominance-antichain width and proof cost;
- Proof Supernode compression;
- Hodge `N_H`, `M_H`, `T_decomp`, and `k_H`;
- resolver boundary size and semantic entropy `eta_R`;
- Chebyshev `t_used` and proof dividend;
- conservation-ledger entries and replay time;
- transfer states before/after continuation reduction, `C`, `B`, `P`, and `J`;
- Möbius/Zipper/journal dispatch and peak memory;
- orbit candidates, verified orbits, and representatives `R_orbit`;
- raw proof branches versus Horizon nodes `H`;
- evidence changes and invalidated proof-cone size;
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
13. Fiber and Crystal expansion reproduces exact selected versions, placements, peer contexts, and lockfile semantics.
14. Dominance, Supernode, Hodge decomposition, rank seal, transfer, continuation, join, orbit, Horizon, and learned-conflict proofs independently replay.
15. Chebyshev-enabled and Chebyshev-disabled runs produce identical certified results.
16. Favorable Hodge scaling claims include `T_decomp`, `N_H`, `M_H`, and `k_H`.
17. Resolver scaling claims include `eta_R`, `C`, `P`, `J`, and `H`; unrestricted worst cases are not mislabeled linear.
18. No topology approximation, entropy limit, frontier limit, join limit, orbit heuristic, or accelerator budget changes an answer.

## 35. Final architectural decision

JAGR will not abandon its four-layer identity. It will change what the layers produce and how exactness is achieved.

The approved target is:

```text
Contextual Instance Fibers and Living Range Crystals
  + append-only evidence
  + Dominance Antichain
  + bidirectional proof and bound waves
  + certified shell
  + Dual-Geometry planner
      + Hodge incidence decomposition
      + Certified Hodge Elimination Loom
      + Two-Sided Rank Seal
      + Semantic Entropy Membranes
      + JAGR Membrane Transfer Algebra
      + Continuation-Complete Crystals
      + Obligation Möbius / Signature-Zipper Braid
      + Proof Orbit Folding
  + Conflict Membrane Learning
  + Proof Horizon Circuit
  + independent verification
```

Its central execution order is:

```text
Contextual Instance Fibers
  -> Living Range Crystals
  -> Dominance Antichain
  -> Dual-Geometry Measurement
      -> Hodge Elimination Loom
      -> Semantic Entropy Transfer Algebra
          -> Continuation-Complete Crystals
          -> Exact Braid Dispatcher
          -> Proof Orbit Folding
  -> Proof Horizon Circuit
```

The Hodge target is `T_decomp + O(k_H^2 N_H + t_used M_H)` when a certified low-width decomposition is available; `t_used` is optional routing and may be zero. The resolver target is `C_build+C_L+C_R+J+P_work+T_orbit+H`, with genuine exponential dependence exposed through semantic entropy `eta_R`, continuation set `C`, objective frontier `P`, and join output `J`. These are structural targets to be validated by instrumentation, not benchmark claims made in advance.

Generic OL3P/CDCL moves out of the intended production hot path and remains temporarily as a shadow oracle, fallback, and difficult-case collector. Promotion occurs only after correctness and performance evidence satisfies the gates above.
