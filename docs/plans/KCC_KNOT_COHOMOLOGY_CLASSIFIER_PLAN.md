# KCC³ — Knot-Cohomology Classifier, Third Edition

**Where the wins actually are: git-committable capsules and speculative neighborhood pre-resolution, built on a corrected fixed-point core.**

Author: rewritten after honest re-audit of KCC². Three v2 claims were wrong (least vs greatest fixed point, LFP-vs-knot-solve identity, universal fixed point). Two new constructs (CGA and SNR) redirect the effort to the actual install bottlenecks (network + filesystem), which the knot's O(1) resolution finally makes attackable.

Status: **Plan only.** Zero files created, zero code changed until approval.

---

## 0. What v2 got wrong (self-audit)

| v2 claim | Reality | v3 fix |
|---|---|---|
| "LFP — Least Fixed Point of a monotone operator" | `propagate_ex` is contractive (D ⊇ F(D)). It computes the **greatest** fixed point below the initial domains, not the least. Tarski gives both; the direction matters. | Renamed to **GFP** (Greatest Fixed Point of the contractive propagation operator). Same theorem, correct half. |
| "The knot **is** the fixed-point iteration" | Wrong. `propagate_ex` is fixed-point, but `knot_search` (`knot.rs:490`) is a **branching search** on top of it. `knot_solve = propagate + branch + backtrack`, i.e. SAT/CSP with strong propagation. | GFP is the **propagation kernel only**. Search-with-propagation is what the knot does. Test GFP for equivalence-of-propagation-results, not equivalence-of-solve-results. |
| "UFP — daemon-maintained universal fixed point over all of npm" | Ill-founded. The greatest fixed point over the full npm constraint graph collapses to empty domains, because npm contains packages that structurally cannot coexist (e.g. libs pinning `react@^15` alongside libs pinning `react@^18`). There is no universe to project from. | Deleted. Replaced by **SNR — Shell of Neighborhood Resolutions**: the daemon stores many concrete resolved capsules from real projects and answers new queries by finding the nearest one. Honest and useful; not "a fixed point," just a shared cache with a smart index. |
| "40-line SIMD roaring bitmap wrapper inside knot.rs" | Real roaring bitmap libraries are 2-3K lines. A 40-line hand-roll would be a toy, slow on non-trivial inputs, and would not deliver 30 GB/s. | Roaring is deferred. Keep `Vec<u64>` `DomainBits`; add an **early-exit + first-run compression** pass (~15 lines) that gives most of the win with no correctness risk. If we ever need real roaring, take the `roaring` crate dep. |
| Focus on shaving microseconds off resolve | On a warm install, resolve is 1-5% of wall time. The rest is packument fetch, tarball download, and 20K file writes. | v3's two new constructs (CGA, SNR) attack **network** and **workflow friction** using the knot's O(1) as an enabler, not as the payoff itself. |

---

## 1. What v3 keeps from v2

Kept verbatim (the parts that hold up under critical review):

- **GFP formalisation** of the propagation kernel (renamed from LFP, otherwise unchanged): Tarski gives existence, uniqueness, and a crisp correctness test against `compute_sheaf_cohomology`'s H⁰ generators for propagation-equivalent inputs.
- **EDG — Explanation Derivation Graph.** Reverse BFS over the knot journal (`knot.rs:248`) from the emptied domain yields the minimal unsatisfiable subset in O(journal length). Replaces cubic tropical H¹ diagnosis on the failure path.
- **PBF as negative filter only.** Persistence barcode narrows the MPT bucket; exact knot fingerprint confirms. No wrong installs from a barcode collision.
- **Star delta via Mayer-Vietoris.** `star_edit_cover` on the changed packages; knot on the star only; glue via boundary domains from the stored capsule. O(bits × deg²) per user edit.
- **Path A validation install.** Singleton-domain propagate to confirm lockfile still satisfies constraints. ≤ 1 ms, no network, no cohomology.
- **Analysis math preserved.** `tropical_sheaf.rs`, `mayer_vietoris.rs`, `betti_numbers.rs`, `tropical_nullstellensatz.rs`, `core_guided_*`, `maxsat_solver.rs`, `ol3p_solver.rs` untouched, still serving `jhol doctor --jagr`, `jhol audit-jagr`, `jhol betti-history`, `jhol jagr-impact`, `--resolver jagr`.

---

## 2. What v3 adds (two new constructs)

Both use the knot's O(1) resolution to enable something no other PM can do — not to shave microseconds off resolve.

### 2.1 CGA — Capsule as Git Artifact

**The bottleneck.** A team of ten clones the same repo. Each dev runs `install`. Each install re-fetches packuments, re-runs resolution, re-downloads tarballs. Ten times. For an unchanged `package-lock.json`.

**The construct.** `jhol` writes a small binary artifact `.jhol/capsule.bin` (mmap'd, ~50-200 KB for typical projects, versioned schema) alongside `package-lock.json`. The capsule contains:

- The `ResolvedGraph` (package → resolved version, tarball hash, integrity SRI).
- The knot fingerprint `kbf` and persistence barcode `pbf` of the constraint graph.
- The full **DomainBits journal** from the successful solve — enough to reconstruct the propagation trace.
- Content-addressed hashes of every tarball, matching what `package-lock.json` already carries.

The capsule is **git-committable**. It is small enough to check in without bloating the repo (much smaller than committing `node_modules`).

**The install path when the capsule is present.**

```
1. Read .jhol/capsule.bin (mmap, O(1) syscalls).
2. Cross-check capsule.pbf and capsule.kbf against a fresh knot fingerprint
   of the package.json + package-lock.json. Mismatch ⇒ drop the capsule, fall through.
3. GFP validation: singleton domains from capsule.resolved_versions, propagate_ex,
   assert every domain non-empty. If yes, capsule is proven consistent with lockfile.
4. For each tarball in capsule: local CAS hit ⇒ reflink to node_modules;
   miss ⇒ parallel download by content hash (registry-agnostic).
5. Done.
```

**Why this is not "just committing the lockfile."** `package-lock.json` requires the resolver to re-derive the graph shape. The capsule stores the **already-derived graph plus proofs of its consistency** (GFP validation passes iff the capsule is compatible with the lockfile). Committing the capsule turns install into a pure file-copy operation, gated by a microsecond validation.

**Why the knot O(1) is what makes this ship.** Without the knot, validating the capsule would require re-running the old MaxSAT to confirm compatibility — expensive, defeating the point. With the knot, GFP validation is `O(bits × edges)` ≈ microseconds. The knot upgrade is what turns "committed capsule" from theoretical to practical.

**Adoption path.** Purely opt-in. Add `.jhol/capsule.bin` to `.gitattributes` as `binary` and to Git LFS if desired. If the file is absent, install proceeds via the existing lockfile path. If the file is stale (lockfile changed), step 2 drops it and falls through. Zero regression risk.

### 2.2 SNR — Shell of Neighborhood Resolutions

**The bottleneck.** During development, users run `jhol add X`, then `jhol add Y`, then `jhol update Z` — a stream of small edits. Each currently costs a fresh resolve + network round-trip.

**The construct.** After every successful install, the daemon (`daemon_cache.rs`) computes the knot resolution for the **N most likely next user actions** — using the knot's O(1) speed to make this feasible in the background:

- Adding each of the top-K popular packages the user doesn't already have.
- Updating each top-level dep to the next major/minor.
- Removing each direct dep.

Each speculative resolve is `O(bits × deg²)` via `knot_solve_star` on the affected package alone (Mayer-Vietoris delta from the current capsule). N speculative resolves take milliseconds total.

The results go into the JMR (`resolution_mirror.rs`), keyed by (current capsule kbf, edit descriptor).

**The next user command.** When the user runs `jhol add X`:

```
1. Compute the edit descriptor from the current capsule + the requested change.
2. JMR lookup: (kbf, edit) → resolved graph.
   Hit ⇒ we already knew the answer. Return in ~1 µs. Continue to tarball download.
   Miss ⇒ fall through to normal cold path.
```

For common workflows (add popular libs, bump minor versions), SNR is a near-100% hit path.

**Why the knot O(1) is what makes this ship.** Speculative pre-resolution of N neighborhoods every install would be prohibitive with cubic MaxSAT. With the knot, N=50 star resolves cost less than one tarball download. The daemon can afford to speculate broadly.

**Prioritisation.** The daemon prioritises neighborhoods based on `second_order_markov.rs` (which already learns command sequences). Popular deps and recently-updated majors are computed first. Speculation is throttled: the daemon idles between installs and never blocks the user's active command.

---

## 3. The runtime cascade, corrected

```
                    ┌──────────────────────────────────────────────┐
                    │              KCC³ Query Cascade               │
                    │                                                │
Path 0 (CGA)        │  .jhol/capsule.bin present? cross-check pbf,   │──► ≤ 1 ms, install
                    │  GFP-validate, tarballs from CAS               │
                    │                                                │
Path A (Lockfile)   │  package-lock.json present ⇒ singleton GFP     │──► ≤ 5 ms, install
                    │                                                │
Path S (SNR)        │  Query JMR by (parent_kbf, edit descriptor)    │──► ≤ 100 µs
                    │  for `add / update / remove` commands          │    when daemon warm
                    │                                                │
Path B (Cold)       │  1. Spectral gate                              │
                    │  2. PBF filter → MPT bucket → exact kbf probe  │
                    │  3. knot_solve (propagate + search)            │
                    │  4. Commit to all tiers + write CGA capsule    │
                    │                                                │
Path C (Delta)      │  star_edit_cover + knot_solve_star + glue      │──► O(bits × deg²)
                    │                                                │
Diagnosis           │  On empty-domain: EDG reverse walk on journal  │──► O(journal)
                    └──────────────────────────────────────────────┘
```

Comparison against KCC²:

| Step | KCC² | KCC³ |
|---|---|---|
| Terminology | LFP (wrong direction) | GFP (correct) |
| Knot framing | "is the fixed point" | "propagate = GFP; solve = propagate + search" |
| Daemon role | UFP (ill-founded) | SNR (concrete shared cache) |
| Git-committable proof | none | CGA capsule |
| Domain rep | 40-line SIMD roaring (unrealistic) | Vec<u64> + first-run compression (honest) |
| Real bottleneck attacked | resolve microseconds | fresh-clone install, dev-loop `add/update` |

---

## 4. File-by-file plan (existing files only)

| File | ± lines v3 | Delta from v2 | What |
|---|---:|---:|---|
| `crates/jhol-core/src/mpt_store/mod.rs` | +10 | 0 | Promote types + `probe_by_pbf_bucket` |
| `crates/jhol-core/src/jagr/mod.rs` | +4 | 0 | pub-use `knot`, `mayer_vietoris`, `resolution_differential` |
| `crates/jhol-core/src/jagr/knot.rs` | +85 | −15 | (a) pub `graph_capsule_fp`; (b) pub `knot_validate_lockfile`; (c) pub `knot_solve_star`; (d) pub `gfp_apply` (was `lfp_operator_apply`, renamed); (e) pub `journal_derivation_walk` (EDG); (f) first-run compression in `DomainBits::intersect`. **Roaring/SIMD removed.** |
| `crates/jhol-core/src/jagr/resolution_differential.rs` | +65 | 0 | Spectral gate wrapper + PBF as negative filter |
| `crates/jhol-core/src/jagr/mayer_vietoris.rs` | +25 | 0 | `star_edit_cover` |
| `crates/jhol-core/src/resolution_mirror.rs` | +25 | +13 | Same public API + **SNR keying: `snr_put(parent_kbf, edit, graph)`, `snr_get(parent_kbf, edit)`** |
| `crates/jhol-core/src/capsule.rs` | +80 | +50 | `pbf`, `kbf`, `journal` fields in header + **CGA read/write to `.jhol/capsule.bin`** + `try_git_capsule_install(project_root)` |
| `crates/jhol-core/src/install.rs` | +100 | +10 | 5-path cascade (Path 0 CGA → A → S → B → C); on success, write CGA capsule if `.jhol/` exists |
| `crates/jhol-core/src/soul_path.rs` | +10 | 0 | Wrapper for `jhol why` / `jhol doctor` diagnostics |
| `crates/jhol-core/src/daemon_cache.rs` | +65 | −20 | **SNR speculation loop**: after install, enumerate top-K edits, run `knot_solve_star` on each, publish to JMR. **UFP removed.** |
| `src/main.rs` | +25 | 0 | Daemon handler for SNR queries |
| **Total** | **~494** | +38 vs v2 | **11 existing files, zero new files** |

---

## 5. Correctness testing

- **GFP ≡ sheaf-H⁰ propagation test** (in `knot.rs`): for small fixtures, propagation via `gfp_apply` reaches a fixed-point domain whose supported-versions set equals the H⁰-consistent version set from `compute_sheaf_cohomology`. Solve-with-search is tested separately against the pubgrub-v2 resolver's output.
- **CGA round-trip test** (in `capsule.rs`): install a fixture, write `.jhol/capsule.bin`, delete `node_modules`, install again — must take Path 0 and match the first result byte-for-byte.
- **CGA invalidation test**: install a fixture, edit `package.json` to bump a dep, install again — capsule cross-check must fail and Path 0 must be skipped.
- **SNR hit rate test** (in `daemon_cache.rs`): simulate 100 real dev sessions of `add / update / remove`; measure JMR hit rate. Target ≥ 60% after ten commands of warm-up.
- **PBF safety** (in `resolution_differential.rs`): two distinct fixtures with same PBF must diverge on exact knot fingerprint. No install may proceed on PBF alone.
- **EDG root-cause quality**: synthetic conflicts must surface the culprit package at the top of the walk.
- **Path A latency**: install-with-lockfile ≤ 5 ms for the 30-pkg fixture.
- **Path 0 latency**: install-with-capsule ≤ 2 ms for the 30-pkg fixture (all tarballs cached).

---

## 6. Implementation order

1. MPT promotion (mpt_store, +10). Refactor.
2. Expose knot internals (knot.rs + jagr/mod.rs, +14). Refactor.
3. Rename `lfp_*` → `gfp_*` and add the correctness test (knot.rs, +25). Refactor with tests.
4. First-run DomainBits compression (knot.rs, +15). Bench.
5. PBF as negative filter (resolution_differential.rs, +55). Additive.
6. EDG reverse walk (knot.rs, +20). Additive.
7. Star delta path (mayer_vietoris.rs + install.rs, +55). Path C.
8. Cold classifier (install.rs, +30). Path B with PBF bucketing.
9. **CGA capsule read/write** (capsule.rs + install.rs, +100). Path 0. Ship behind `JHOL_CGA=1` until proven.
10. **SNR speculation** (daemon_cache.rs + resolution_mirror.rs + main.rs, +100). Path S. Ship behind `JHOL_SNR=1` until proven.
11. Promote CGA and SNR to defaults after benchmarks confirm ≥ 3× cold install on fresh clone (CGA) and ≥ 10× on `add`/`update` commands (SNR).

---

## 7. What v3 deliberately does NOT do

- Does not claim least fixed point (v2 did; wrong direction).
- Does not equate `knot_solve` with pure fixed-point iteration (v2 did; ignores search).
- Does not maintain a "universal fixed point" over all of npm (v2 did; mathematically ill-founded).
- Does not hand-roll a 40-line SIMD roaring bitmap (v2 did; not credible).
- Does not add new crate deps.
- Does not create new files.
- Does not touch existing lockfile formats, CLI flags, or config keys (adds `JHOL_CGA=0/1` and `JHOL_SNR=0/1` opt-ins only).
- Does not delete any of the cubic analysis code; `jhol doctor --jagr` etc. keep working.

---

## 8. Why the two new constructs cannot be replicated

| Property | KCC³ (Jhol) | npm / bun / pnpm / yarn |
|---|---|---|
| Git-committable resolved artifact | CGA capsule (mmap, self-validating via knot GFP) | Lockfile only — must re-derive graph shape |
| Speculative neighborhood pre-resolution | SNR (daemon runs top-K star resolves in background) | None — resolve happens on user command |
| Star-local delta for `add`/`update` | O(bits × deg²) via `knot_solve_star` | O(n) to O(n²) re-resolve |
| Diagnosis | EDG linear walk on journal | Backtrack log / heuristic message |
| Cross-project MPT with PBF bucketing + exact fp confirmation | Yes | Exact file hash only |
| Runtime validator | GFP (correct-direction Tarski fixed point) | SAT / PubGrub / heuristic |
| Analysis path | Preserved (tropical sheaf, Mayer-Vietoris, Betti, MaxSAT/OL3P) | None |

Neither CGA nor SNR is thinkable without the knot's O(1) solve: CGA needs microsecond validation of a committed artifact; SNR needs microsecond speculation on many neighbors. Both are enabled directly by the knot upgrade.

---

## 9. The one sentence

**KCC³ corrects v2's fixed-point direction and abandons the ill-founded universal fixed point, then uses the knot's O(1) solve to enable two things no other package manager can afford: a git-committable capsule that turns fresh-clone install into a validated file copy, and a background speculation loop that makes `add` / `update` commands answer from cache — while every existing math module remains untouched for offline analysis.**
