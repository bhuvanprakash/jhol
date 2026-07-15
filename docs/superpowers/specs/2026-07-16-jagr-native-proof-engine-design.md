# JAGR Native Proof Engine Design

**Status:** Approved architecture; implementation has not started  
**Date:** 2026-07-16  
**Product name:** JAGR  
**Scope:** Replace generic OL3P/CDCL on JAGR's production resolution hot path while preserving the four-layer identity, exact resolution quality, deterministic output, and safe fallback behavior.

## 1. Executive summary

JAGR currently converts the package-resolution problem into a generic Boolean optimization problem and delegates final authority to OL3P/CDCL. Profiling after the certified-lazy resolver work showed that metadata expansion, tropical Nullstellensatz construction, and Mayer-Vietoris cover construction were real costs, but they were not the terminal bottleneck. Once those costs were reduced, the exact OL3P/CDCL path dominated runtime. On the Express lockfile fixture, the old JAGR path, the new default path, and the refined path all reached a 120-second guard, while Bun 1.3.14 completed in 3.114 seconds.

The problem is therefore not the existence of JAGR's four mathematical layers. The problem is their current relationship with the exact solver. The upper layers mostly analyze or classify the graph, while the final layer still receives a large generic Boolean formula. Lazy metadata refinement can also cause repeated solves after the request fingerprint changes, losing much of the prior exact-search work.

This design replaces the production OL3P/CDCL path with a package-native exact resolver. It operates directly on finite domains of package versions, propagates constraints in both directions, collapses forced packages into a certified shell, isolates the small unresolved obstruction quotient, and performs exact search only inside its obstruction knots. Every forced assignment, eliminated version, decomposed component, pruned branch, and objective bound carries a replayable reason. A separate certificate verifier—not the solver itself—authorizes output.

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

The native engine consists of ten cooperating units:

1. **Domain compiler:** converts authoritative metadata into compact native version domains and dependency relations.
2. **Evidence ledger:** stores append-only facts with stable identities and manifest hashes.
3. **Proof-wave propagator:** applies forward and backward domain reductions to a fixed point.
4. **Reason arena:** records why every version was removed or package was forced.
5. **Certified-shell builder:** removes fully proven packages from the active quotient.
6. **Obstruction-quotient decomposer:** isolates unresolved components and their certified boundaries.
7. **Native knot solver:** performs reversible exact search only inside unresolved components.
8. **Objective-bound engine:** proves that no unexplored branch can improve the incumbent.
9. **Certificate verifier:** independently replays feasibility, decomposition, branch closure, and optimality.
10. **Shadow/fallback controller:** compares the native result with the old solver and controls rollout.

The normal data flow is:

```text
authoritative metadata
    -> append evidence
    -> compile/update native domains
    -> propagate proof waves
    -> collapse certified shell
    -> decompose obstruction quotient
    -> hydrate missing proof-support metadata when necessary
    -> solve exact obstruction knots
    -> combine component proofs
    -> verify global certificate
    -> emit lockfile
```

No stage may skip the independent verifier.

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

## 8. Append-only evidence ledger

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

## 10. Bidirectional proof waves

### 10.1 Forward wave

For each remaining source version, forward propagation computes which target versions support its required and peer edges. If a target package loses support, its domain shrinks.

### 10.2 Backward wave

When a target domain changes, backward propagation revisits only source versions whose support masks intersected the removed target versions. A source version with no remaining support for a required edge is eliminated.

### 10.3 Fixed point

Propagation continues until the work queue is empty. It may terminate with:

- a stable set of domains;
- a forced package;
- an empty domain and contradiction proof;
- a missing authoritative manifest request;
- a newly separable obstruction component.

### 10.4 Reason recording

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

## 13. Obstruction quotient and membrane

### 13.1 Quotient contents

The active obstruction quotient contains only packages that remain relevant because of:

- multiple viable versions;
- conflicting incoming ranges;
- peer cycles;
- optional-to-required interactions;
- unknown authoritative metadata;
- objective-sensitive alternatives;
- membership in a replayable topological obstruction support;
- connection between two unresolved components.

### 13.2 Membrane

Each unresolved component has a membrane describing its interface with the certified exterior:

- fixed boundary assignments;
- incoming authoritative ranges;
- peer obligations crossing the boundary;
- objective contribution already fixed outside;
- manifest/context hashes;
- proof identifiers supporting the boundary.

Two components may be solved independently only when all paths between them pass through certified membrane assignments and neither objective nor peer constraint crosses the separation unresolved.

### 13.3 Decomposition proof

Component independence is itself certified. The proof enumerates crossing edges and shows that each is discharged by a fixed boundary assignment. The verifier rejects recombination if it finds an unaccounted unresolved crossing constraint.

## 14. Four-layer contracts

### 14.1 Layer 0: structural influence

Layer 0 supplies deterministic structural evidence:

- dependency-component membership;
- certified boundaries;
- articulation-like separator candidates;
- high-impact edges;
- prior proof-capsule matches;
- obstruction-pressure features.

Layer 0 may choose work order and propose decomposition. It cannot remove a version without a replayable native-domain reason.

### 14.2 Layer 3: tropical feasibility

Layer 3 operates on the unresolved quotient, not the full certified graph. It may produce:

- admissible objective lower-bound hints;
- infeasible direction candidates;
- tropical slack features;
- compact candidate obstruction supports.

A Layer 3 reduction becomes authoritative only after translation into native-domain evidence and verifier replay. Expensive algebraic analysis remains optional and budgeted. A skipped Layer 3 analysis never changes correctness.

### 14.3 Layer 2: sheaf gluing

Layer 2 identifies where locally valid choices may fail to glue globally. Its production output is a proposed minimal obstruction support over packages and boundary edges.

The support determines which packages must remain together in an exact knot. It does not independently claim SAT or UNSAT. Full diagnostic cohomology remains available through explicit analysis mode.

### 14.4 Layer 1: native proof engine

Layer 1 becomes the sole production authority before independent verification. It consumes:

- authoritative finite domains;
- replayable reductions;
- certified membranes;
- validated component decompositions;
- exact objective ranks.

It performs native propagation, exact knot search, branch closure, component recombination, and global proof construction.

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

### 15.3 Version ordering

Candidate versions are tried in the exact deterministic preference order for the active objective. This finds a strong incumbent early but does not replace proof of optimality.

### 15.4 Branch lifecycle

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

### 15.5 Branch closure

A branch closes only through one of:

- replayable empty-domain contradiction;
- violated authoritative required edge;
- violated authoritative peer edge;
- certified decomposition component UNSAT;
- exact objective lower bound no better than the incumbent;
- verified memoized component proof under an identical membrane signature.

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

For every open branch or component, the engine computes a verified optimistic bound from remaining domains and mandatory closure. A complete assignment supplies a certified upper bound.

The engine may stop only when every unexplored branch has either:

- a contradiction proof; or
- a lower bound that cannot improve the incumbent.

### 16.3 Optimality certificate

The final proof contains the incumbent objective tuple plus a forest of branch closures demonstrating that no better tuple remains reachable. The verifier recomputes comparisons using the canonical objective definition.

## 17. Boundary resonance memo

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
2. Root requirement satisfaction.
3. Selected-version membership in authoritative domains.
4. Required dependency closure.
5. Peer dependency compatibility.
6. Optional dependency semantics.
7. Overrides, resolutions, platform, engine, and prerelease behavior.
8. Every domain-elimination reason used by the final proof.
9. Every certified-shell forcing proof.
10. Every component membrane and decomposition proof.
11. Every contradiction branch closure.
12. Every memoized proof used in the result.
13. The final objective tuple.
14. The lower-bound closure of all potentially better branches.

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

### Gate F: controlled promotion

Rollout proceeds:

```text
off -> shadow 100% -> native 1% -> 10% -> 50% -> default
```

Any correctness mismatch returns rollout to shadow. Repeated performance regression returns rollout to off while preserving diagnostic data.

## 29. Planned implementation phases

This section defines architecture order, not the detailed task-by-task implementation plan.

### Phase 0: preserve oracle and baselines

Freeze current binaries, exact fixture outputs, objective semantics, and benchmark commands. Add no native behavior.

### Phase 1: compile native domains

Introduce stable identifiers, version tables, domain bitsets, and compiled typed edges. Compare semantics without resolving.

### Phase 2: evidence and reasons

Introduce the append-only ledger, immutable reason arena, proof hashing, and independent primitive-reason replay.

### Phase 3: persistent propagation

Implement bidirectional proof waves, fixed-point queues, forced domains, contradictions, and reversible domain journals.

### Phase 4: certified shell and quotient

Implement shell eligibility, membrane boundaries, obstruction-quotient construction, component decomposition, and decomposition proof replay.

### Phase 5: native exact knots

Implement deterministic branching, exact candidate order, journaled backtracking, contradiction closure, component recombination, and objective-bound proofs.

### Phase 6: live hydration

Connect bounded parallel metadata hydration to the same persistent engine. Preserve identifiers, proofs, domains, and memo state across evidence generations.

### Phase 7: four-layer proof contracts

Translate validated Layer 0, Layer 3, and Layer 2 outputs into replayable native-domain reductions, decomposition proposals, or ordering hints. Keep unverifiable outputs advisory.

### Phase 8: verifier and proof capsules

Complete global certificate verification, component proof reuse, Merkle capsules, tamper rejection, and stale-context invalidation.

### Phase 9: shadow and fallback rollout

Run native and old paths together, classify mismatches, collect difficult cases, and enforce automatic fallback.

### Phase 10: measured promotion

Promote only through correctness and performance gates. Retain the old solver as an explicit fallback until long-term evidence supports further removal.

## 30. Alternatives rejected

### Persistent generic CDCL

Keeping CNF and improving incremental state would lower risk but retain representation expansion and generic solver overhead. It remains useful as fallback work, not the target production architecture.

### Greedy-only resolution

Propagation and preferred-version selection are extremely fast on easy graphs but cannot prove difficult peer, cycle, conflict, and objective cases. Greedy remains a fast path inside the native engine, followed by proof.

### Mathematical layers as direct authority

Allowing tropical or sheaf analysis to finalize resolution would couple correctness to approximate or budgeted analysis. This design preserves those layers while requiring translation to replayable native-domain proof.

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

Mitigation: exact worst cases cannot be eliminated honestly. Reduce them through certified shells, component decomposition, strong incumbents, exact bounds, memoized membranes, and fallback. Track knot size and fallback causes explicitly.

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

## 35. Final architectural decision

JAGR will not abandon its four-layer identity. It will change what the layers produce and how exactness is achieved.

The approved target is:

```text
native version domains
  + append-only evidence
  + bidirectional proof waves
  + certified shell
  + obstruction quotient and membranes
  + exact obstruction-knot search
  + objective proof forest
  + independent verification
```

Generic OL3P/CDCL moves out of the intended production hot path and remains temporarily as a shadow oracle, fallback, and difficult-case collector. Promotion occurs only after correctness and performance evidence satisfies the gates above.

