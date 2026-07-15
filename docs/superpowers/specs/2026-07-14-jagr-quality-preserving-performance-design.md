# JAGR Quality-Preserving Performance Design

## Goal

Improve Jhol's four-layer native JAGR resolver without removing mathematics,
changing successful selections, weakening error explanations, or regressing
current install performance.

## Scope

This phase implements only changes that can be protected by direct regression
tests and parity checks:

1. Correct resolver-cache identity.
2. Compute Layer 2 sheaf cohomology once per slow-path resolution.
3. Remove avoidable request, max-atom-system, and greedy-candidate cloning.
4. Add measurements that reject performance regressions.

Integer-ID graph conversion and obstruction-frontier resolution are excluded
from this phase. They remain promising, but their wider representation and
mathematical-composition risk requires a separate experimental design.

## Preserved JAGR architecture

The resolver retains every existing layer:

- Layer 0: spectral resolution differential and SCC certificate.
- Layer 3: tropical Nullstellensatz and max-atom feasibility.
- Layer 2: tropical sheaf cohomology, Betti numbers, SNF, and Mayer-Vietoris.
- Layer 1: exact core-guided MaxSAT.

Fast-path verification, slow-path fallbacks, derivation graphs, explanations,
lockfile preference, resolver configuration, and final dependency verification
remain functional.

## Design

### Cache identity

Replace the request-only `u64` cache key with a typed key covering all inputs
that can affect resolution:

- Canonical request graph.
- Full lockfile package/version contents.
- `use_lp`, `skip_layer2`, `skip_layer3`, `run_all_layers`.
- Timeout and maximum-iteration settings.
- An explicit cache-format/algorithm version.

Hash data incrementally into xxh3 rather than constructing a `Vec<String>` and
joining it. Diagnostic `run_all_layers` requests continue to bypass the cache.

Regression tests must first demonstrate that different lockfiles and different
resolver configurations do not share cached results.

### Single Layer 2 analysis

Build the tropical sheaf once and compute one `SheafCohomology`. Derive Betti
numbers, SNF interpretation, local/global obstruction fields, explanations,
and Mayer-Vietoris inputs from that shared result. Existing public helpers stay
available for callers outside the resolver.

The resolver must not call independent helpers that recompute cohomology from
the same sheaf. A test-only counter will prove that the resolver slow path uses
one primary cohomology computation before any explicitly distinct cover or
intersection computations required by Mayer-Vietoris.

### Allocation reductions

- Avoid cloning `ResolveRequest` solely to fill ordinals. Build a local ordinal
  view or make ordinal preparation return owned ordinal data while borrowing the
  request graph.
- Make `TropicalMaxAtomSolver` borrow a `MaxAtomSystem` when mutation of the
  system is unnecessary.
- In greedy resolution, select the best compatible version in one iterator pass
  without collecting a temporary `Vec<String>`.
- Keep strings and current public types at boundaries so diagnostics and API
  compatibility do not change.

Each reduction is implemented separately and retained only when benchmarks are
non-inferior.

## Correctness gates

Every production change follows red-green TDD. Required checks:

- Existing focused JAGR suite: 121 current tests remain green.
- Two different lockfiles cannot reuse one cached selection.
- Resolver configurations cannot reuse incompatible cached results.
- SAT/UNSAT, selected versions, cost, Betti numbers, SNF result, strategy, and
  derivation/explanation fields remain equivalent where behavior is unchanged.
- Resolver snapshot and compatibility fixtures retain parity.
- Full `cargo test -p jhol-core jagr --lib` passes after each logical change.

## Performance gates

Capture a release-mode baseline before production edits. Compare repeated
medians for chain, fan-out, layer profiles, conflict-heavy slow paths, and the
existing Jhol-versus-Bun install scenarios.

A change is retained only when:

- No workload has a repeatable regression greater than 3%.
- Cache-cold and cache-hot measurements remain separated.
- Layer-profile cases run with isolated caches.
- Resolver parity and install correctness remain green.

Any change that fails correctness is reverted. Any change that repeatedly
regresses performance beyond the guardrail is reverted or redesigned.

## Files expected to change

- `crates/jhol-core/src/jagr/jagr_resolver.rs`: cache identity, orchestration,
  ordinal preparation, greedy allocation removal, regression tests.
- `crates/jhol-core/src/jagr/tropical_sheaf.rs`: reusable analysis from an
  existing cohomology result and test instrumentation.
- `crates/jhol-core/src/jagr/betti_numbers.rs`: derive Betti data from the shared
  cohomology result without resolver-side recomputation.
- `crates/jhol-core/src/jagr/simplicial_normal_form.rs`: derive SNF analysis from
  shared Betti/cohomology data.
- `crates/jhol-core/src/jagr/tropical_max_atom.rs`: borrowing solver input if the
  implementation confirms the system is read-only.
- `crates/jhol-core/src/jagr/mod.rs`: isolated, reliable layer benchmarks.

## Non-goals

- Removing or bypassing a JAGR layer beyond existing verified fast paths.
- Copying another package manager's resolver or mathematical model.
- Changing lockfile semantics, dependency compatibility, lifecycle behavior,
  or error-reporting quality.
- Large graph-representation rewrites in the same change set.

## Approved 2026-07-16 extension: certified lazy JAGR

JAGR keeps its name and four layers. The resolver adds a monotone knowledge
lattice around them so metadata work follows proof demand instead of eagerly
expanding every candidate version.

### Monotone knowledge lattice

Each package-version advances through `Unknown`, `VersionKnown`,
`ManifestKnown`, and `ConstraintCertified`. Transitions only move forward.
An abbreviated packument creates the version domain; a version manifest adds
authoritative dependency, optional-dependency, peer, distribution, and
integrity data. Unknown metadata is never interpreted as an empty dependency
set.

### Dual-bound closure

An optimistic problem, in which unknown manifests impose no undiscovered
constraints, supplies a lower objective bound. A completely hydrated and
certificate-valid assignment supplies an upper bound. Optimized resolution may
finish only when the bounds meet and the existing dependency certificate
passes. This protects feasibility and preference quality. UNSAT is final only
when its proof depends exclusively on authoritative constraints.

### Obstruction quotient and information scheduler

Certified graph regions collapse out of the unresolved quotient. Layer 0
supplies spectral influence, Layer 3 supplies tropical slack, Layer 2 supplies
relative obstruction support, and Layer 1 supplies conflict/core frequency.
These values rank hydration work but never decide correctness. Ranking is
deterministic and byte-aware.

### Persistent exact refinement

Refinement rounds retain the exact formula, learned information, objective
bounds, and deterministic phase choices. New authoritative constraints are
monotonic additions guarded by activation state. The first implementation may
retain only reusable formula/bound state when safely retaining the internal
CDCL trail is not yet valid; telemetry must distinguish both cases.

### Rollout and rollback

`JHOL_JAGR_REFINEMENT=off|shadow|on` controls the experiment. `off` preserves
the current resolver. `shadow` computes and compares the candidate while the
current resolver remains authoritative. `on` may return the candidate only
after parity and performance gates pass. Every phase is independently
revertible. Any correctness difference fails immediately; repeated median
regression above 3% or P95 regression above 5% restores the previous green
phase.
