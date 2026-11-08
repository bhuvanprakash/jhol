# JAGR-∞ — the resolver

The only resolver in jhol. It is deliberately **not** a PubGrub/CDCL design:
no clause learning, no nogoods, no backjumping. It narrows monotone ordinal
bitmaps to a fixpoint over a dirty queue, searches most-constrained-first under a
bounded budget, and repairs locally. Explanations come from bitmap intersections.

That choice is not decoration — it is why conflict explanations cost milliseconds
(`why-not` returns a minimal unsat core offline) and why policy and history are
one more intersection rather than a second pass.

Source: `crates/jhol-core/src/jagr_infinite.rs` (~9k lines, containing the chart,
atlas, RVM, and research modules), `jagr_nest.rs` (instance resolver),
`jagr_index.rs` (binary metadata index), `why.rs`, `why_not.rs`.

---

## 1. Data model

### Versions are ordinals

For a package, its published versions are sorted by semver and numbered from 0:

```
express@4.17.1 -> VersionOrdinal(412)
```

`VersionOrdinal` is a `u16` with `MIN = 0`. Sort order is the *semver* order, so
"newer" is "higher ordinal" and choosing newest is choosing the highest set bit.

### Domains are bitmaps

```rust
struct VersionBitmap {
    words: Vec<u64>,      // 64 versions per word
    ordinal_lo: u16,      // base: bit 0 == ordinal_lo
    ordinal_hi: u16,
}
```

**The base matters.** Words are offsets from `ordinal_lo`, so two bitmaps with
different bases describing the same package are not word-comparable. `intersect`
and `union` are therefore base-aware:

* same base → word-wise AND/OR (fast path);
* different base → rebase onto the overlap by iterating the narrower support.

A defect in this area is documented in `DEFECT-LOG.md` §1; the fix is written and
held behind the instance resolver rework, and the test that catches it is
`proof_obligations::completeness_against_brute_force` (currently `#[ignore]`d with
the reason inline).

Useful operations: `new`/`empty`/`from_interval`/`set_interval`/`mask_to_interval`/
`set_ordinal`/`intersect`/`union`/`count`/`is_empty`/`iter_ordinals`/
`contains_ordinal`.

> `from_interval` anchors at `interval.lo`. To narrow an existing domain, clone it
> and use `mask_to_interval` — rebuilding on a foreign base is the bug above.

### The chart

```rust
struct ChartPackage {
    package_id: u32,
    name: String,          // filled by the builder since the contract fix
    ordinal_lo, ordinal_hi,
    allowed: VersionBitmap,
    versions: Vec<String>,          // index = ordinal - ordinal_lo
    deps: Vec<DependencyEdge>,      // sorted (target, spec, optional)
    peers: Vec<PeerEdge>,           // sorted (target, spec)
}

struct DependencyEdge {
    target_pkg: u32,
    spec: String,                        // raw spec — the derivation
    required: VersionInterval,           // generic scale
    required_mask: Option<VersionBitmap>,// target's ordinal space
    optional: bool,
}
```

`required_mask` is the load-bearing field: it is the dependent's requirement
expressed in the **target's** ordinal space, computed at chart-build by evaluating
the raw spec against the target's published versions. The generic `required`
interval cannot narrow anything precisely — for most plain specs it is the full
domain.

Edges are sorted, and the propagation queue is sorted, so a chart is a function of
its inputs. `extract_deps` used to return a `HashSet`-derived `Vec`, making edge
order hash-dependent; the equality test between a chart built from JSON and one
built from the binary index caught it.

---

## 2. Resolution

### Build

`build_full_chart(packuments, name_to_id)`:
1. extract sorted versions, dependency and peer declarations per package (rayon);
2. names are filled here — callers no longer have to remember;
3. `build_chart_from_constraints` computes one mask per distinct `(target, spec)`
   pair, memoised, using a **compiled spec** (see §3).

### Propagate

Fixpoint over a dirty queue. For each package, its domain is intersected with
every applicable dependent's mask. Invariants:

* **monotone** — domains only shrink;
* **idempotent** — propagating a fixpoint changes nothing;
* **order-independent** — incoming lists and the initial queue are sorted.

Peers narrow a domain when they overlap it and are *reported* when they do not;
they never empty a domain on their own (npm installs and warns;
`JHOL_STRICT_PEER_DEPS` escalates).

On an empty domain the engine returns a conflict that names the package, the
candidates that survived narrowing, and every dependent that demands it
(`explain_empty`).

### Search

Most-constrained-first (fewest candidates), tie-broken by package id — a resolver
must be a function. Candidates are tried highest-first; each tentative choice is
propagated into its dependents before descending, with the narrowed child domains
saved and restored on backtrack. `JAGR_SEARCH_BUDGET` (default 20 000) bounds the
steps, and exhaustion is reported rather than hidden.

### Flat vs nested

`select_versions(packuments, deps)` answers "one version per package". That model
**cannot represent an npm tree**, where `chalk@4` and `chalk@2` coexist because
`node_modules` is a directory tree. Historically this was masked: the
base-misaligned `intersect` silently accepted versions that violated requirements,
so `jest` "resolved" by being wrong.

The correct model is instances (`jagr_nest`, §4).

---

## 3. Why chart build got 12× faster

Measured on 275 packuments / 6,715 edges, before any tuning:

| phase | ms |
|---|---:|
| extract versions + deps + peers | 2.4 |
| **assemble masks** | **141.6** |

Two wrong hypotheses were measured and discarded first (semver parsing of
versions: 148→140 ms; JSON extraction: 2.4 ms of 144 ms). The actual cause:
`version_satisfies_parsed` is written for one-off checks — it trims, probes for an
exact pin, and takes a **mutex-guarded global cache lock per call** — while
selecting a version asks it once per published version of the target.

`registry::compile_spec` decides the variant once:

```rust
enum CompiledSpec { Any, Exact(Version), Alternatives(Vec<VersionReq>), Unsupported }
```

* `Any` — `""`/`*`: stable versions only (npm excludes prereleases);
* `Exact` — a bare pin is equality, *not* caret;
* `Alternatives` — one `VersionReq`, or several joined by `||`;
* `Unsupported` — matches nothing, as before.

Matching becomes a pure comparison. Published version strings are also parsed once
per package rather than once per `(spec, version)` pair.

| phase | before | after |
|---|---:|---:|
| mask assembly | 141.6 ms | **11.4 ms** |
| chart total | 144.0 ms | **14.5 ms** |

Semantics are unchanged by construction and pinned by `compiled_spec_tests`
(25 spec shapes × 16 versions: pins, wildcards, prereleases, `||`, hyphen ranges,
junk).

---

## 4. Instances — the multi-major resolver

`jagr_nest::resolve(packuments, roots)`:

* a requirement creates an **instance** of the target;
* a second instance is created only when an existing one's version cannot satisfy
  the new requirement;
* every instance records `required_by: Vec<(dependent, spec)>` — its derivation.

```
jest@29.7.0: 270 instances across 249 packages, 19 packages with >1 version, ~103 ms
  ansi-styles     6.2.3, 5.2.0, 4.3.0
  ansi-regex      6.5.0, 5.0.1
  brace-expansion 5.0.12, 2.1.7
```

Properties (all tested):

* **determinism** — sorted roots, sorted child expansion, newest-satisfying;
* **explanation** — `required_by` answers "why is chalk@2 here?";
* **bounds** — instance and step caps fail loudly;
* **three failure kinds**, because they need different responses:
  `MissingPackument` (fetch and retry), `Unsatisfiable` (chain + published
  versions), `Bounded`, plus `MissingDates` for `--as-of`.

### Placement (tree building)

`lockfile_write::resolve_full_tree_nested` turns instances into a node-keyed tree:

1. **winner per package** = most dependents, ties → highest version → `node_modules/<name>`;
2. every other instance nests under the first of its dependents whose path is
   already known → `<parent>/node_modules/<name>`;
3. anything unplaced (only possible with a cycle, which npm trees are not) goes to
   the top level rather than going missing;
4. each instance is expanded to `ResolvedPackage` with tarball URL and integrity.

### `npm:` aliases

`"@jest/react-is-18": "npm:react-is@^18.2.0"` installs **react-is** under the alias
name. The resolver resolves the real package and records
`Instance::installed_as` for the tree, and the availability check expands the alias
before deciding a fetch is needed. Without this, a real jest tree stops at HTTP 404.

### Fetch-and-retry

Missing metadata is a *request*, not a failure: the resolve loop reports which
package it needs and the caller provides it. A bounded prefetch (`missing_packages_bounded`)
fetches the frontier 16 at a time with no peers, a name cap, capped rounds and a
20 s ceiling; whatever remains is fetched singly. The first unbounded attempt hung
a cold install to a 600 s timeout — that is why every bound exists.

---

## 5. Policy and history as inputs

### `--as-of <date>` — `resolve_with_options(.. cutoff)`

Only versions published at or before the cutoff are eligible. Fail-safe by
construction: a version with no publish date cannot be shown to have existed then,
so it is **excluded**, and a package with no dates reports `MissingDates`.

Verified: express 4.17.1 (2020-06-01, 50 pkgs), 4.18.2 (2023-06-01, 58 pkgs),
4.22.3 (today, 71 pkgs) from one manifest. A dated install never falls back to the
flat resolver — it exits 1 with zero packages installed rather than installing
today.

### `--allow-license <list>` — `ResolveOptions::allowed_licenses`

Only versions whose declared license is in the set. Unknown licenses are excluded
by default (`--license-allow-unknown` is the explicit escape hatch). Both npm forms
are read (`"MIT"`, `{"type": "Apache-2.0"}`), SPDX `OR` is treated as a
disjunction, matching is case-insensitive.

Verified on express: `MIT` refuses at `qs` (BSD-3-Clause); `MIT,BSD-3-Clause`
refuses at `setprototypeof` (ISC); `MIT,BSD-3-Clause,ISC,Apache-2.0` installs 71
packages with **0 violations, 0 unknown** on an independent audit of the tree.

---

## 6. The metadata index

`jagr_index.rs` stores exactly what the resolver reads — sorted versions and each
declaration with the releases that declare it — as a flat little-endian blob
(`JHOLJAG1`), beside each cached packument as `<file>.json.idx`.

| reading 344 packuments | time |
|---|---:|
| gunzip + JSON + prune | 426 ms |
| cache view via index | **15.2 ms** |

* validation is **stat-only** (length + mtime in the header): hashing the packument
  would spend what the index saves;
* stale, truncated, foreign-magic and trailing-byte blobs fall back to JSON — a bad
  index costs one parse, never a wrong answer;
* writes are atomic (temp + rename);
* the loader is **self-healing**: anything it had to parse gets an index written, so
  first read installs the accelerator.

---

## 7. Explainability

* `jhol why <pkg>` (`why.rs`) — every installed copy, its version, path, and the
  range each dependent demanded, from the lockfile alone (no registry, no
  re-resolution). Attribution is nearest-declaring-ancestor; an undeclared copy is
  reported as such.
* `jhol why-not <pkg>@<range>` (`why_not.rs`) — the **minimal unsat core**: intersect
  the request with every installed requirement, then shrink greedily (drop one; if
  the intersection is still empty it was never needed). Three outcomes are kept
  distinct: already installed, published-but-forbidden, nothing published.

---

## 8. Knobs

| variable | effect |
|---|---|
| `JAGR_SEARCH_BUDGET` | backtracking step cap (default 20 000) |
| `JAGR_RECONCILE_STEPS` | legacy reconcile bound |
| `JAGR_PROFILE_CHART=1` | per-phase chart timings |
| `JAGR_NO_INDEX=1` | disable reading/writing the binary index |
| `JHOL_NO_SELECTION_CACHE=1` | disable the verified-selection cache |
| `JHOL_NEST_RESOLVER=0` | use the previous nested resolver |
| `JHOL_AS_OF` | cut-off timestamp (set by `--as-of`) |
| `JHOL_ALLOW_LICENSE`, `JHOL_LICENSE_ALLOW_UNKNOWN` | policy |
| `JHOL_STRICT_PEER_DEPS=1` | peers become failures instead of warnings |
| `JHOL_JAGR_MAX_DEPS` | nested-analysis budget by manifest size |

---

## 9. Obligations

| obligation | test |
|---|---|
| soundness (selection satisfies every edge) | `proof_obligations::soundness_every_selection_satisfies_every_edge` |
| determinism under reordering | `determinism_independent_of_map_insertion_order`, `determinism_proofs::propagation_is_independent_of_input_order` |
| monotonicity + idempotence | `propagation_only_narrows_and_is_idempotent` |
| completeness vs brute force | `completeness_against_brute_force` (**ignored**, §1 of the defect log) |
| mask memo bound + builder naming | `builder_names_packages_and_masks_are_memoised` |
| compiled spec ≡ one-off predicate | `compiled_spec_agrees_with_the_one_off_predicate` |
| multi-major instances | `jagr_nest::tests::*` |
| policy / history | `license_policy_tests`, `as_of_tests` |
| chart from index ≡ chart from JSON | `jagr_index::tests::chart_from_index_equals_chart_from_packuments` |
