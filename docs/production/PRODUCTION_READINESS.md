# Production readiness — 2026-10-03

Current continuation (2026-10-28): full-system readiness remains in progress.
Verified local fixes cover formatter semantic preservation, static field and
arrow-closure behavior, bounded Unicode lookahead, native-call dispatch, and
JAGR replay validation, CPU/ABI gates, executable-memory publication and arena
ownership/offset safety. Windows linking and Linux cross-checks pass; native
runtime platform runs remain open. Current observations and remaining acceptance gates are
kept in the [existing work state](../../.agents/work/performance/papers-to-runtime/state.json).
Promises/microtasks, complete generator delegation, Unicode identifier-property
validation, override/layout consistency, the platform matrix, and the full speed
target still require evidence. Historical checks below certify their stated
scope only.

Workstream W2 of the production-ready upgrade. Scope: the five blockers that
would embarrass a production user or reviewer today. Duplication is W1;
archive/record is W3. Nothing was deleted, no git write was performed, and the
working tree stays uncommitted.

Evidence for every row is a command that was actually run in this checkout.

---

## 1. Resolver flag honesty

**Before.** `src/main.rs` accepted
`--resolver {pubgrub-v2,pubgrub,jagr,jagr3,jagr-infinite,legacy}` with the help
text `"Resolver strategy for lockfile/dependency resolution"`, which reads as six
selectable strategies. The production wrapper `resolve_full_tree()`
(`crates/jhol-core/src/lockfile_write.rs:289`) routes **every** accepted value to
JAGR-Infinite and logs a note for non-default values. `ARCHITECTURE.md` §16.1 and
§24.1 recorded the mismatch but the CLI contradicted them.

**Decision: the honest-docs rung (ponytail rung 1).** Real dispatch was not
cheap: `--resolver` only sets `JHOL_RESOLVER` (one call site, `src/main.rs:4724`),
and the core's *documented contract* is the opposite — the doc comment on
`resolve_full_tree()` says "This is the only resolver. We don't expose PubGrub,
legacy SAT, or the older JAGR-1/JAGR-3 as user-selectable strategies". Making the
flags real would create strategies the resolver layer explicitly refuses to have,
and would need a test per branch. So: help text, runtime note, and
`ARCHITECTURE.md` now say the same fact.

**After.**

| Surface | Statement |
|---|---|
| `src/main.rs:1697` (`--resolver` help) | "Resolver name, kept as a compatibility alias: every value routes to JAGR-Infinite, the only primary resolver (a non-default value logs a note). If JAGR-Infinite fails, a PubGrub-named rescue runs only for manifests with <=25 direct dependencies" |
| Runtime note (unchanged, verified) | `[JAGR] note: --resolver=legacy is no longer a distinct path; JAGR-infinite is the only resolver and handles all cases.` |
| `ARCHITECTURE.md:886` (§16.1) | "`--resolver` values are compatibility aliases and the CLI help text now says so" |
| `ARCHITECTURE.md:1464` (§24.1) | rewritten from "concrete CLI/help mismatch / possible future resolution" to the settled fact, plus the 25-dependency fallback bound |

**Evidence.**

```sh
grep -n "compatibility alias" src/main.rs                     # -> 1697
grep -rn "is no longer a distinct path" crates/jhol-core/src/ # -> lockfile_write.rs:303
grep -n "MAX_FALLBACK_DEPS" crates/jhol-core/src/lockfile_write.rs  # -> 338, const = 25
```

Fallback bound verified live: `lockfile_write.rs:338`
`const MAX_FALLBACK_DEPS: usize = 25;` — over the bound the install fails fast
with the JAGR-Infinite error rather than paying per-candidate packument fetches.
The classification is unchanged in `ARCHITECTURE.md` §5.3 (do not read classic
PubGrub out of the name) and §24.2.

---

## 2. Documentation naming files that do not exist

A path-existence scan over `README.md`, `GET_STARTED.md`, `docs/user/**`,
`ARCHITECTURE.md` and `CHANGELOG.md` found **14 dead references**, all of the same
class: automation that was moved out of `scripts/` during this upgrade while the
docs (and one workflow) kept the old paths.

**Fixed.**

| File | Was | Now |
|---|---|---|
| `README.md:91` | `INNOVATION_LOG.md` | `docs/internal/INNOVATION_LOG.md` (exists) |
| `README.md:376` | `scripts/check_benchmark_regression.py` (deleted) | `scripts/bench/guardrails.py regression` |
| `README.md:385` | `scripts/collect_baseline.py` (deleted) | `scripts/bench/guardrails.py collect` |
| `README.md:390` | `scripts/check_guardrails.py` + the nonexistent `benchmarks/guardrails.json` | removed; config does not exist, `benchmarks/week1_guardrails.json` does |
| `README.md:398,408,418,428,431,438,441,447` | 6 more deleted `scripts/*.py` invocations | one "Report generators that are not currently automated" section naming `archive/scripts/` and the `cp` restore command |
| `CHANGELOG.md:619` | `scripts/bench/scales.py` | `scripts/bench/scales.py` (exists) |
| `CHANGELOG.md:572` | `tests/capsule_adversarial.rs` (never existed there) | `crates/jhol-core/tests/capsule_adversarial.rs` (exists) |
| `ARCHITECTURE.md` | `scripts/bench/scales.py` (deleted) | `scripts/bench/scales.py` (exists) |

**Evidence** (same scan, after the edits):

```sh
# every scripts|examples|benchmarks|tests|docs path referenced in README / GET_STARTED /
# ARCHITECTURE / CHANGELOG / docs/user / docs/production
missing refs: 0
```

Doc-internal example artifacts are deliberately excluded from that scan:
`baseline-report.json`, `benchmark-results.json`, `sbom.json`, `jhol.toml`,
`jhol.project.lock`, `before/after.receipt.json`, `models/...` are names a user
*may* create or that live under `examples/blueprint/`; they are not repo paths the
docs claim exist.

**Not fixed here (W3's deliverable).** `archive/scripts/` is the reason these
fixes are restore commands rather than deletions — 21 preserved generators there
already carry 15 of the deleted `scripts/*.py` names, which W3 is populating
against its MANIFEST.

---

## 3. Formerly empty `crates/jhol-cloud/` (resolved 2026-10-24)

**Before.** `crates/jhol-cloud/` was on disk but absent from `Cargo.toml`
workspace members, so it was never built or tested. It contained no files.

**Resolved.** The crate now has a manifest, `src/lib.rs`, and a workspace
`members` entry. It implements an offline tenant/token/quota/routing domain
model and request/response envelopes; 12 targeted tests pass.

**Still not delivered.** No server, persistence, transport security, live
authorization, provider integration, or deployed `jhol cloud` service exists.
Its token generator is explicitly not a CSPRNG and must be replaced before
deployment.

**Historical note.** `docs/archive-record/UNWIRED_BUT_KEPT.md` C1 keeps the
original empty-directory finding and now records the resolution.

---

## 4. CHANGELOG completeness for this upgrade

**Before.** `[Unreleased]` (lines 26–1584) is a large, detailed account of the
upgrade, but had three defects against the work logs:

1. **A stale claim contradicted the manifest.** Lines 610–616 stated
   "`http-reqwest` is no longer a default feature (opt in with
   `--features http-reqwest`)", while `Cargo.toml:45` still reads
   `default = ["runtime", "bundle", "http-reqwest"]`. A reader following the
   changelog would build a different binary than the one shipped.
2. **The one-origin content work was missing.** `crates/jhol-core/src/content.rs`
   (durable-origin write-through, one shared root list for read and write) is
   covered only as a *measurable effect* ("metadata requests 8 -> 0") with no
   `content.rs`/one-origin entry, which is the root cause the owner asked to be
   recorded. The durable-origin `Added` entry at line ~109 describes the symptom.
3. **`INNOVATION_LOG.md` and `scripts/bench/scales.py` paths** (fixed in §2).

**After.** An appendix `## Upgrade notes — production-readiness pass (2026-10-03)`
at the end of the file records: the resolver compatibility aliases, the
`http-reqwest` correction (marked as superseding the earlier entry), and the CI /
docs path relocation. The stale claim is *corrected in place by the appendix*
rather than rewritten in the historical block — the changelog keeps its
chronological integrity, and the appendix is the current truth.

Why an appendix and not a rewrite: `[Unreleased]` is a chronological record of
many sessions, and per the owner's rule the record is not edited to look better
than it was; the correction is additive and dated. The same reasoning applies to
the existing duplicated `## [Unreleased]` headings (one at line 26, one at line
1810, both pointing at the same compare link) — **flagged for the owner**, not
merged: consolidating them would rewrite two sections that belong to different
work periods, which is a documentation-structure decision, not a W2 blocker.

**Covered and verified as already complete in `[Unreleased]`** (cross-checked
against the `AGENTS.md` work logs): nested/multi-version by-range resolution
(E145, fixed), `npm:` alias install + spec-grammar gate, `--lockfile-only`
platform variants, `JHOL_OS`/`JHOL_CPU`/`JHOL_LIBC`, JGrid L0 + L1-store,
durable local origin, peer dependencies end to end, SRI integrity, nested partial
install (os error 22), persistent store index (warm install O(entries × store)),
`JHOL_STRICT_PEER_DEPS` / `--strict-peer-deps`, and the CI gate. No inflated
claims found: every performance number in the section cites its protocol
(median-of-N, cache state, fixture).

---

## 5. CI gate reality check

**Before.** `.github/workflows/ci.yml` (4.5 KB, previously 0 bytes) and
`.github/workflows/benchmark.yml`. `ci.yml`'s references all exist and its
commands are all runnable. `benchmark.yml` referenced **5 scripts that no longer
exist** at those paths — the job would fail on the first missing one.

**Fixed.** `.github/workflows/benchmark.yml` now invokes the preserved generators
from `archive/scripts/` (`check_guardrails.py`, `resolver_fixture_report.py`,
`framework_compat_report.py`, `check_fallback_trend.py`,
`enterprise_npmrc_report.py`). Every flag is unchanged; each was checked against
the script's own parser.

**Runnable check for every referenced path and tool.**

```sh
for f in examples/blueprint/verify.sh examples/blueprint/verify_online.py \
         scripts/mem_guard.py scripts/benchmark.py scripts/bench/guardrails.py \
         archive/scripts/*.py; do [ -e "$f" ] || echo "MISSING $f"; done   # -> nothing
python3 scripts/bench/guardrails.py --help                    # -> Modes: regression | collect | guardrails | fallback-trend | clean-artifacts | dashboard
python3 archive/scripts/check_guardrails.py --help            # -> Check Week-1 KPI guardrails
python3 archive/scripts/resolver_fixture_report.py --help     # -> Resolver fixture parity report
```

`ci.yml` cross-check: `cargo fmt --all --check`, `cargo clippy --all-targets`,
the foundation `-D warnings` clippy, foundation `--lib` tests,
`cargo build --bin jhol`, `examples/blueprint/verify.sh`, the CLI-help/dispatch
loop over `plan diff sync snapshot restore db ai dev`, the `uv`-driven
`verify_online.py` adapter smoke, `cargo test`, and `cargo build --release --bin
jhol` + `--version` smoke. All exist; the CLI-help loop's subcommands are all
defined in `src/main.rs` (1392–1418). No new gate was added.

**Known limitation — the local workflows are not hosted.** `.github/workflows/**`
is untracked in this repo by owner design: those paths were deliberately stripped
from the GitHub remote (2026-09-30) and must not be committed nor added to
`.gitignore` (see the owner's record file at the repo root). GitHub therefore
never sees `ci.yml` or `benchmark.yml`. What is verified here is that the
workflows are *correct and runnable as written*, not that any GitHub job has ever
run. Hosting them is an owner decision; W2 did not commit or ignore anything.

---

## Verification (this workstream)

```sh
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets   # exit 0
CARGO_BUILD_JOBS=2 cargo test --test commands              # 29 passed; 0 failed
cargo fmt --all                                            # applied to the Rust file touched
```

Only one Rust file was touched (`src/main.rs`, one help string), so the workspace
check is the meaningful gate; the full-suite baseline (704 passed) is owned by
the orchestrator's final reconciliation.

---

## Still open — needs an owner decision

1. **Hosted CI results.** The workflows are correct but nothing runs them on
   GitHub (paths untracked by design). Someone must decide whether these paths
   return to the remote.
2. **Feature/platform matrix.** No release, `--all-features`, Linux or Windows
   verification ran in this checkout; `ARCHITECTURE.md` §24.3 stands. The 12
   ignored tests (8 stress + 1 perf atlas + 2 core + 1 doctest) remain unrun by
   default.
3. **Report automation is archived, not wired.** After this pass, resolver
   parity / framework compat / fallback trend / enterprise `.npmrc` reporting
   runs from `archive/scripts/` in CI but is not part of the live `scripts/`
   surface. Restoring them into `scripts/` (and deciding their ownership) is a
   real decision the owner should make; the W2 fix is honest-docs only.
4. **`crates/jhol-cloud/`** — resolved as a buildable workspace member with an
   offline domain model and 12 tests. Deployment remains open; there is no live
   service to restore or verify.
5. **Two `## [Unreleased]` sections** in `CHANGELOG.md` (line 26 and line 1810)
   with identical compare links. Left as-is deliberately (see §4).
6. **`--resolver` real dispatch.** If the owner wants six genuinely distinct,
   tested resolver strategies, that is a resolver-layer project (the core's
   documented contract currently forbids it) — not a CLI change, and not in W2.
