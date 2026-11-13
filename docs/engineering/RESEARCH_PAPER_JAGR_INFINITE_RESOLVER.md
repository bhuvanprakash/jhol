---
title: 'JAGR-Infinite: Verifying Dependency Resolution in npm Layouts'
subtitle: 'From version bitmaps to witnesses for the package each consumer can see'
author: 'Bhuvan Prakash · Independent Researcher'
date: '13 November 2026 · Jhol Research Series IV'
---

## Abstract {.unnumbered}

A package resolver can choose versions that satisfy the manifest and still produce a layout in which a consumer sees the wrong dependency. This paper studies the boundary between selection and placement in Jhol's JAGR-Infinite resolver. I trace the chart solver, instance resolver and lockfile writer, and use native Rust probes to distinguish arithmetic defects from search and representation limits. The baseline disagreed with 26 of 81 bitmap intersections and missed a valid older release. A follow-up with corrected bitmap operations and release-conditioned search agreed with all 81 intersections and recovered the older release in both selectors. I propose a witness that records the first visible provider and the empty paths checked before it. In an exhaustive model of three lookup locations and two versions, reusing a witness while its checked prefix stayed unchanged produced no wrong verdicts across 2,187 layout-and-range comparisons. Checking only the old provider produced 26 false accepts. The model identifies a concrete way to verify and incrementally recheck a layout. It does not establish a general npm compatibility guarantee or a performance gain for the proposed verifier.

# Introduction

When I look at a Jhol lockfile, I want to answer a question a list of versions cannot answer: **which copy will this consumer actually load?** A project can need two releases of the same package. Both releases can appear in a manifest inventory while one consumer still resolves to the wrong copy.

Paper I studies what a resolver may discard from its input. This paper studies what it must prove about its output. It makes three contributions:

1. An implementation analysis of where JAGR-Infinite chooses a release, where Jhol places it, and where the installer checks it.
2. Native probes that expose failures hidden by aggregate install results, alongside an independent finite test of first-provider witness reuse.
3. A verification design that tracks both the provider used by an edge and the absence of closer providers. The design specifies which checks can be reused after a layout edit.

# Related Work

Arborist documents the ideal tree and installation layout for npm [1]. Its behavior is a useful compatibility baseline under matched inputs. PacSolve and MaxNPM formulate dependency consistency and optimization as constraints [2]. A comparison with them should use the intersection of their semantic model and Jhol's, rather than treating different but valid version choices as failures. PubGrub derives incompatibilities and uses conflict-driven backtracking [3]. It provides a useful solver-design comparison. Its default package identity model needs adaptation before comparing npm placement behavior. CUDF-based modular package managers separate solver inputs and objectives from materialization [4]. Here the verification object is a concrete path-keyed layout and the provider visible to each consumer.

# Implementation Under Study

## From the command to a layout

Jhol has several ways to satisfy an install request. An eligible prepared or lockfile layout can bypass new resolution. An unlocked native install gathers roots and metadata, expands a flat selection, and can ask JAGR-Infinite's chart for an alternative. The installer checks a chart candidate against the declared ranges of selected releases before accepting it. When the flat result cannot represent a conflict, the installer can attempt a single-pass nested layout from available metadata. If that route is ineligible or rejected, the instance resolver is tried. The ordinary path can fall back again. Policy-bound paths shown in the source fail rather than silently substituting a present-day solve.

The [CLI dispatch](../../src/main.rs), [installer](../../crates/jhol-core/src/install.rs) and [lockfile writer](../../crates/jhol-core/src/lockfile_write.rs) implement distinct parts of that decision. A successful install alone does not show which solver handled the graph. A resolver benchmark needs to record whether its candidate was selected, rejected, bypassed, or replaced by fallback.

## Candidate domains and selection

The [chart implementation](../../crates/jhol-core/src/jagr_infinite.rs) maps versions to local ordinals. Candidate masks are in the target package's ordinal space. Each bit needs its base offset to have a meaning. The production virtual machine propagates masks and searches under a budget. In the follow-up chart, each edge carries a second mask in the parent package’s ordinal space, marking the releases that declare it. The flat selection helper returns either a map keyed by package name or no candidate. Topological quantities are computed on a diagnostic branch and do not decide the ordinary virtual-machine selection.

Let $D_p$ be the allowed releases of a parent slot $p$. For a child slot $c$, let $S_{p,c}(v)$ be the child values compatible with parent release $v$. A release without that edge imposes no restriction, so $S_{p,c}(v)=V_c$, the full child universe for the model. When several parent slots truly constrain the **same** child slot, a sound narrowing step has the form

$$
D_c' = D_c \cap \bigcap_p \left(\bigcup_{v\in D_p} S_{p,c}(v)\right).
$$

The inner union preserves alternatives. The outer intersection applies only to a shared child slot. This is a condition for the model, not a description of every existing propagation step. Once a release is selected, its exact declarations must be checked. Otherwise, a child supported by a different candidate parent release can survive. It is also necessary to represent a candidate release that has no edge at all.

The implementation now uses the mandatory-target part of this rule. It narrows a target only when the declaring releases cover the parent's current domain. Search activates a required edge when the chosen parent release declares it, then checks any already selected child against its own mask. Backtracking rolls back both active packages and narrowed domains. The acceptance check recomputes the required closure from the roots and rejects an incompatible, missing, or unreachable assignment. It applies to fresh results, continuation candidates and cached assignments.

## Instance construction and placement

The [nested resolver](../../crates/jhol-core/src/jagr_nest.rs) reads package metadata on demand, orders the worklist deterministically, and reuses an existing satisfying instance or selects a new one. It supports multiple major versions. The instance tracks both registry identity and the name under which it is installed. Its bounds, missing metadata, and absence of an eligible release have separate outcomes. Its local preference rule does not systematically backtrack to a different parent release when that parent's transitive requirements fail.

The [path layout code](../../crates/jhol-core/src/layout.rs) has a first-visible lookup and a required-edge check. A separate nested planning path chooses hoisted winners and assigns other instances under dependents. Its placement map keys by package and version in the examined source, even though resolver records can distinguish aliases. Its peer check can look at versions present anywhere in the instance set. Those are reasons to check alias collisions and peer visibility at concrete paths before claiming universal placement correctness. The present paper does not infer an end-to-end failure from those source risks.

The final lockfile carries paths and artifact provenance. Tarball integrity and the on-disk layout are separate checks. [Treecheck](../../crates/jhol-treecheck/src/lib.rs) compares the installed tree with the authorized layout. It does not hash every installed JavaScript file. The witness studied here verifies dependency-edge placement, before byte integrity or lifecycle effects enter the contract.

# First-Provider Witnesses

## Contract

Fix an immutable metadata snapshot, layout lookup rules, and the platform and policy inputs. A layout $L$ maps a canonical slot to either an identified package instance or absence. For a consumer at path $q$ and requested name $n$, let

$$
C(q,n)=(c_1,c_2,\ldots,c_k)
$$

be the candidate slots in lookup order. For a required edge $e$, validity means that the **first occupied** slot supplies the requested package identity and a version satisfying the selected consumer's declared range. An incompatible nearer provider shadows any farther compatible provider. For the formal result below, lookup order, the consumer declaration and the identity/version interpretation are fixed. Optional, peer, alias and workspace rules require their own explicit checks before expanding the contract.

## Proposition and proof

Let $j$ be the first occupied slot in $C(q,n)$, or $k$ when all slots are empty. The witness $W_e$ contains the facts $L(c_1),\ldots,L(c_j)$, along with the consumer declaration and policy context. If those inputs and all recorded slot facts are unchanged in $L'$, the edge verdict is unchanged.

**Proof.** When a provider exists, every recorded predecessor remains empty and the recorded provider remains the first occupied slot with the same identity and version. The declaration and rules used to judge it are unchanged, so the result is unchanged. When no provider exists, all candidate locations were recorded as empty and remain empty. This covers successful and failed required lookups. The proof also explains the need to record absence. A newly occupied nearer location can change the verdict while leaving the old provider untouched.

A proposed implementation indexes each recorded slot by the edges that depended on it. For changed slots $\Delta$ and changed declarations $E_{\mathrm{new}}$, recheck

$$
A(\Delta)=E_{\mathrm{new}}\;\cup\!
\bigcup_{c\in\Delta}\{e:c\in W_e\}.
$$

Here $c\in W_e$ means the witness recorded a fact about that slot, including absence. Rule, platform or policy changes invalidate the relevant context. The reverse index must be updated on every recheck. Work for those lookups is proportional to the sum of their lookup lengths, plus change detection and index maintenance. Reading the entire tree to find changes can dominate the saved checks. This is a design to implement and evaluate in Jhol.

# Evaluation Methodology

The evaluation separates three evidence sources. The native probe calls the two Rust resolver APIs on in-memory synthetic package records and compares bitmap operations with ordinary integer sets. The witness check independently enumerates a finite placement space. The performance observations come from older archived loopback experiments that produced lockfiles without fetching tarballs. None of these experiments runs a real-project install for this paper.

# Results

## Native Differential Probes

I built the baseline example in release mode on macOS arm64 and ran it twice. The two runs produced identical JSON. It uses only synthetic packuments supplied in memory and does not invoke the CLI or the materializer.

| Probe | Reference expectation | Native result |
|:-------------------------------|:--------------------------|:-------------------------------|
| Same-base bitmap intersection | 81 exact set intersections | 26 disagreed |
| Wide mask intersected with first word | Ordinals 0 to 63, 64 values | Ordinals 0 to 127, 128 values |
| Conditional dependency | parent 1.0.0 is a valid complete choice | Flat helper declined. Nested helper chose parent 1.1.0 and reported Unsatisfiable for child ^2.0.0 |
| Two incompatible child majors | Four instances including two child versions | Flat helper declined. Nested helper returned all four instances |

: Native differential probes and a multi-major control.

**Bitmap boundary.** The probe takes all ordered pairs of intervals starting at ordinal zero with upper endpoints 0, 62, 63, 64, 65, 126, 127, 128 and 129. It compares membership with the ordinary intersection. The equal-base fast path intersects zipped words and leaves the remaining left-hand words unchanged when the right mask is shorter. The 64-value example shows exactly why that matters. A later validator can reject some bad selections, but that cannot restore a correct primitive or prove all callers catch the error. The probe does not show an incorrect package reaching disk.

**Conditional dependency.** The root admits parent 1.0.0 and 1.1.0. Version 1.0.0 has no child requirement. Version 1.1.0 requires child ^2.0.0, but the supplied snapshot has only child 1.0.0. The nested helper tries 1.1.0, reports a conflict for its child, and does not revisit the parent. The flat helper also returns no candidate. The conflict describes the explored branch. It cannot certify the root problem infeasible when parent 1.0.0 is a witness. The multi-major control shows a different outcome: the flat representation declines a layout that the nested instance representation can express.

## Follow-up Native Probes

| Probe | Baseline | Follow-up |
|:-------------------------------|:--------------------------|:-------------------------------|
| Bitmap intersections | 26 disagreements in 81 pairs | 0 disagreements in 81 pairs |
| Conditional dependency | Neither selector recovered the witness | Both returned parent 1.0.0 |
| Incompatible child majors | Four nested instances and no flat choice | Four nested instances and no flat choice |

: Results for identical synthetic inputs before and after the resolver changes.

The release-conditioned flat selector also agreed with exhaustive feasibility in 81 two-package worlds, including cycles, and each returned witness satisfied its active edges. The cached-assignment test changes a release declaration and checks that the previous assignment is rejected and the cache identity changes. These tests establish results for the tested flat model. The lookup witness below concerns placement after selection. [Follow-up native results](../architecture/experiments/jagr-infinite-native-probe-release-edges-2026-11-13.json)

## Native Placement and Frozen Replay

The baseline CLI exited successfully on a synthetic local-registry project while one root alias lacked a manifest. I reran the same fixture after keeping provider instance IDs through path assignment. The manifest names and versions matched at all six expected paths, including two root aliases of the same registry package and a nested alias whose registry target differs from the root alias. After removing the live tree, a frozen offline install recreated the same six identities with zero registry requests. The local fixture exercises publication and lockfile replay. It does not estimate latency or establish compatibility across arbitrary registries or filesystems. [Probe and recorded results](../../scripts/bench/jagr_nested_install_probe.py), [Raw observations](../architecture/experiments/jagr-nested-install-probe-2026-11-13.json)

## Exhaustive Witness Check

Each of three ordered slots can be empty or contain version 1 or 2. The reference checker enumerates all 27 layouts before and after an edit and tests ranges accepting version 1, version 2, or either. For each of the 2,187 combinations, it compares a full new lookup with two reuse rules. The first requires the entire checked prefix to remain unchanged. The second checks only whether the old provider remains in its old slot.

| Reuse rule | Verdicts reused | Wrong reused verdicts | False accepts |
|:------------------------------|----------------:|-----------------:|-------------:|
| Checked prefix, including absence | 549 | 0 | 0 |
| Previously present provider only | 702 | 52 | 26 |

: Exhaustive results for the three-slot, two-version model.

The 26 false accepts occur when the old lookup was valid and a change makes the new lookup invalid while the old provider remains. The other 26 wrong reuses go in the opposite direction. The two reuse rules have different eligibility, including how they handle an originally absent provider. The counts measure this finite enumeration, not the rate of shadowing in npm projects.

## Archived Performance Observations

| Loopback lockfile workload | Jhol median | Bun median | Scope |
|:--------------------------|-------------:|-----------:|:-----------------------|
| 1,200 independent roots | 264.7 ms | 84.3 ms | Three runs per tool, direct selections checked |
| 1,200-package chain | 947.7 ms | 311.3 ms | Separate fixture, three runs per tool |

: Metadata-only command timings from the archived experiment. Includes startup and lockfile writing.

A separate diagnostic on the chain recorded 498.3 ms fetching and expanding metadata, 7.1 ms building the chart, and 5.5 ms solving plus initial materialization. The full resolver path was 531.2 ms in that run. If fetch and expansion remain unchanged on its critical path, making all other resolver work free could improve that diagnostic by at most $531.2/498.3\approx1.066$. This is a bound for that stage split, not a speedup measurement. It motivates profiling metadata and allocation before optimizing solver arithmetic for this workload. The [experiment record](../research/JAGR_INFINITE_MATH_ONLY_BENCHMARK_RECORD.md) gives the historical versions, conditions, scheduler trials, and limitations.

# Threats to Validity

The native fixtures are deliberately small. They expose a counterexample or validate a control, and they cannot measure defect prevalence. The proposed verifier's proof holds for a fixed snapshot, declarations and lookup semantics. The executable model uses abstract slots, without filesystem symlinks, npm peer rules or scripts. The historical performance record has three repetitions per comparison and uses generated metadata on loopback. An end-to-end evaluation needs a frozen corpus, route labels, path-level checks and a second platform.

# Conclusion

JAGR-Infinite already gives Jhol a bitmap chart, bounded search, and a way to resolve multiple releases into instances. The research opportunity is to make the step from those choices to the layout independently checkable. The key fact for each required edge is the first provider the consumer can reach. Its witness must remember what was absent on the way there. That gives Jhol a concrete condition under which it can reuse a verdict after an edit.

# Artifact Availability {.unnumbered}

The [native probe](../../crates/jhol-core/examples/jagr_paper_iv_probe.rs), [native observations](../architecture/experiments/jagr-infinite-native-probe-2026-11-13.json), [follow-up native observations](../architecture/experiments/jagr-infinite-native-probe-release-edges-2026-11-13.json), and [finite witness checker](../../scripts/bench/jagr_iv_witness.py) accompany this manuscript. The Rust probe runs with Cargo's locked, offline release build of the jhol-core example, with the http-reqwest feature and selection caching disabled. The witness checker requires Python 3 and no packages. See the [repository license](../../LICENSE) for the software terms.

# References {.unnumbered}

1. npm CLI. “Arborist: Build Ideal Tree.” [Design documentation](https://github.com/npm/cli/blob/latest/workspaces/arborist/docs/ideal-tree.md).
2. D. Pinckney, F. Cassano, A. Guha, J. Bell, M. Culpo, and T. Gamblin. “Flexible and Optimal Dependency Management via Max-SMT.” *IEEE/ACM International Conference on Software Engineering*, 2023. [Paper and preprint](https://arxiv.org/abs/2203.13737).
3. Dart pub team. “PubGrub Version Solving Algorithm.” [Design documentation](https://dart.googlesource.com/pub.git/+/f27dcfdb/doc/solver.md).
4. P. Abate, R. Di Cosmo, R. Treinen, and S. Zacchiroli. “A Modular Package Manager Architecture.” *Information and Software Technology*, 2013. [Paper](https://www.dicosmo.org/Articles/2013-AbateDiCosmoTreinenZacchiroli-Ist.pdf).
