# JAGR-∞ — what it is, what shipped, what remains

JAGR-∞ is jhol's only resolver. Its design is deliberately not PubGrub/CDCL:
**no clause learning, no nogoods, no backjumping**. It narrows monotone ordinal
bitmaps to a fixpoint over a dirty queue, searches most-constrained-first under a
bounded budget, and repairs locally. Conflict explanations come from bitmap
intersections, not from a clause database — which is why they are cheap.

## Shipped in this pass

| item | state |
|---|---|
| Mask memoisation per `(target, spec)` | **done** — `compute_mask` + cache; a chart has thousands of edges and few distinct specs |
| Chart builder names packages | **done** — `build_full_chart` fills `name`; it was a doc-comment rule callers had to remember |
| Edges carry the raw spec | **done** — `DependencyEdge::spec` / `PeerEdge::spec`, the derivation a `why` answer needs |
| Proof obligations as tests | **done** — soundness (40 random worlds), determinism (insertion order + repeats), monotonicity + idempotence, perf corpus (400×12), memo bound |
| Two real defects found and pinned | see below |

### Measured

```
[perf] 400 packages x 12 versions, 3 edges each: chart+solve 188 ms, selected Some(400)
```

## Defects found by the new obligations (pinned, not hidden)

### 1. `VersionBitmap::intersect` ignores `ordinal_lo`  ✅ FIX WRITTEN, HELD BACK

Words are offsets from `ordinal_lo`, but `intersect` ANDs them positionally.
`from_interval({1..1})` anchors at ordinal 1; intersecting it with a domain based
at 0 selects ordinal **0** — a version the range excludes. It surfaced as a
spurious "no version of p1 satisfies every dependent" in
`completeness_against_brute_force` (currently `#[ignore]`d with this reason).

The fix is written and verified (base-aware `intersect`/`union`: fast path when
bases match, rebase onto the overlap otherwise, plus `mask_to_interval` at the
call sites). It is **held back** because with correct intersections, `jest`
stops resolving: its tree genuinely needs several majors of `chalk`/`semver`, and
the flat path requires one version to satisfy every dependent. The old bug was
silently accepting incompatible versions, and the nested-layout path masked the
result. Landing the fix requires the nested-layout rework below — the flat
solver cannot be correct on a multi-major tree by construction.

### 2. The lockfile does not record everything the installer materialises  ⚠️ OPEN

`express@latest`: 68 packages installed, **56** lockfile entries; the 12
omissions appear in no entry's `dependencies` either, so the lockfile looks
internally consistent while being wrong. Consequences: `jhol ci` is not
reproducible from the lockfile, and any verifier that trusts the lockfile as the
set of authorised packages will want to delete real code.

Mitigations landed: pruning is now **opt-in** (`JHOL_PRUNE_EXTRANEOUS=1`), an
unlisted package is reported as a warning, and the tree is not re-installed on
every invocation for a superset. `lockfile_is_complete` refuses to prune against
a lockfile that does not account for its own declared dependencies.

Fix: write the lockfile from the set actually materialised, not from a narrower
selection. Acceptance: `express@latest` installs N packages and writes N entries;
`jhol ci` from the lockfile reproduces the tree byte-identically.

## Remaining items, specified

### Item 2 — SCC decomposition + parallel components

* Build the condensation over dependency edges, with **peer edges treated as
  undirected** (a peer pair constrains both ways, so both belong in one
  component).
* Tarjan, iterative (no recursion: package graphs can be thousands deep).
* Resolve in condensation topological order, roots first: a component's
  constraints arrive only from its dependents, so once the dependents are fixed
  the component is independent.
* Parallelise across components that share no upstream constraints; merge
  results by **canonical component id** so output is byte-identical regardless of
  scheduling. Determinism is part of the contract, not a nice-to-have.
* Incremental: key each component's chosen versions by the component's hash, so
  `jhol add x` re-solves only components whose inputs changed — O(changed SCC)
  instead of O(tree).
* Acceptance: adding a package to a 400-package world re-solves < 5% of
  components; results identical to the unsplit solve on the corpus.

### Item 3 — Lockfile preference and minimal change

Two-phase, sound by construction:

1. Narrow each package's domain to the version the existing lockfile pins, when
   that version is still allowed by every applicable edge.
2. If the pin-first solve succeeds, it is minimal-change by construction. If it
   fails, re-solve without pinning and report the churn (`name old -> new`).

This is a *preference*, never a constraint: a locked version that violates a
requirement is dropped, not honoured. Acceptance: `jhol add lodash` on a
100-package lockfile changes exactly one entry; `jhol update x` changes only
x's component.

### Item 4 — Proof-carrying resolution (`why`, `why-not`, minimal cores)

Every edge now carries its raw spec, so a resolution can carry its derivation:

* `jhol why <pkg>` — answers from the lockfile alone (root specs + each entry's
  `dependencies`), no network, no re-resolution: "chosen because `app` needs
  `^1`, `lib` narrowed it to `<1.5`; 1.2.3 is the newest allowed".
* `jhol why-not <pkg>@<range>` — the **minimal unsat core**: gather incoming
  edge masks plus the requested range, intersect, and greedily shrink while the
  intersection stays empty. Report the smallest set of edges that conflict and
  the smallest relaxation that fixes it. No clause DB required — this is the
  payoff of the bitmap representation.
* Minimal cores also feed `explain_empty`, replacing the current "who demands
  what" dump with "these N requirements cannot hold together".

Acceptance: on a synthetic 3-edge conflict, the reported core is exactly those 3
edges; relaxing any one of them makes the resolution succeed (asserted in a
test).

### Item 5 — Proof obligations (remaining)

* Determinism of the **chart build** (the solver is proven pure; the builder is
  not — it iterates `HashMap`s).
* Completeness vs brute force, once defect 1 is fixed (test already written).
* Speed corpus in CI with recorded baselines, so a regression is attributable.
* Fuzz the spec grammar against node's own `semver` on a corpus of ranges.

## Sequencing

1. Lockfile writer records everything materialised (defect 2) — unblocks pruning
   and reproducibility.
2. Nested layout resolves per dependent (multiple versions per package) — then
   land defect 1's fix.
3. SCC + parallel + incremental (item 2).
4. Preference/minimal-change (item 3) — cheap once components exist.
5. `why`/`why-not`/cores (item 4) — cheap once derivation data exists (it does).
