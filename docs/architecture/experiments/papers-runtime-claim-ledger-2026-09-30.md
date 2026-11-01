# Paper-to-runtime claim ledger (30 Sep 2026)

The three source papers are immutable inputs. This ledger records whether each claim is implemented and what evidence is still needed. Static source presence alone does not count as reproduction. Historical numbers below are claims from prior artifacts, not current certification.

## GRM and Instant Lock

| Claim | Current evidence | Status | Gate |
|---|---|---|---|
| Capsule is versioned, mmap-readable, and rejects malformed offsets | Capsule parser and corruption tests exist | Conditionally supported | Corrupt/truncated/oversized property corpus and full install replay |
| Warm no-op detects changed manifests and missing installed packages | Shared proof binds manifest, supported lockfile, config, platform, policy env, and installed lock paths; proof tests pass; C, Rust, and shell launchers execute `--version` | Conditionally supported | End-to-end invalidation through each shipped entrypoint and all lockfile formats |
| Prebuilt L0 is safe and instant | Unsafe shared-tree hardlink/symlink shortcut disabled; core capsule path remains independently validated | Unsupported; unsafe shortcut disabled | No throughput claim until capsule and project-isolation tests cover this path |
| Warm install 7.5–11 ms and daemon 30 ms | Paper values only | Missing current evidence | Matched 30-round matrix with exact binary, daemon, and no-op strata |
| ~300-package warm install 76–127 ms vs Bun 179–235 ms | Paper values only | Missing current evidence | Same project, package tree, and verified output on current builds |
| GRM ordinal records accelerate production install | GRM modules exist; install call path not demonstrated | Missing evidence | Trace production call graph and profile matched workload |

## JAGR-S and JGrid storage

| Claim | Current evidence | Status | Gate |
|---|---|---|---|
| SwissProbe/Elias–Fano microbench gains | Bench and implementation exist | Conditionally supported | Rebuild current release; pinned inputs, CPU, p50/p95, counters |
| JAGR-S lowers page misses and speeds batch install | Paper-only workload numbers | Missing evidence | Matched whole-install runs plus OS counters and artifact coverage |
| Harvest retrieves exact SRI-matched bytes | SHA-512 SRI lookup and verification paths exist | Conditionally supported | Poisoned sidecar/object and positive/negative provenance checks |
| JGrid materializes complete package closure | Focused suite 61/61; nested duplicate versions, symlink/mode preservation, completion markers, and cross-project writes covered | Conditionally supported | Concurrent-writer/interruption and mixed-source integration remain |
| JGrid helps cold install | Current React cold pilot observed no JGrid route; JGrid-enabled sample invalidated for route miss | Unsupported speed claim | Populate a verified JGrid seed and prove route, then compare matched arms |
| Batch ingest removes repeated store rewrites | Batch API and reopen/content test pass; single-package API remains a wrapper | Conditionally supported | Separate ingest benchmark and publication-count/counter evidence |

## Holographic resolution and FastStream

| Claim | Current evidence | Status | Gate |
|---|---|---|---|
| Leaf decimation preserves solver semantics | Current description uses degree-based leaf checks | Unsupported as general proof | Exhaustive comparison with exact resolver on small graphs, including peers and aliases |
| Ricci/spectral order yields zero backtracking | Ordering code and paper examples do not prove a general guarantee | Unsupported as universal claim | Record nodes/backtracks across adversarial pinned fixtures; describe only observed scope |
| Mayer–Vietoris gluing preserves solutions | Implementation is present; paper proof needs production boundary-state validation | Missing production evidence | Differential solve each partition and joined graph against exact reference |
| Tropical closure is 14.6× faster | Paper microbenchmark only; compatibility with SemVer constraints unproven | Missing evidence | Match semantics first, then benchmark identical closure inputs |
| FastStream removes staging and is 4.15× faster | Current registry route downloads a temporary compressed file, then verifies/extracts it through `StreamDirectProjector::project_reader`; it is not the paper's no-intermediate-file pipeline | Unsupported for current registry path | Validate SRI, truncation, archive containment, then paired extract and install measurements |
| Cold Next.js 38 ms / zero backtracks | Paper benchmark claim | Missing current evidence | Reproduce fixture/tool versions and assert complete tree and build |

## Current matrix pilot

`paper-matrix-registry-cold-smoke-final4-2026-09-30.json` is one randomized
React-app round on macOS arm64 (Jhol 1.0.2, Bun 1.4.2, Node 26.10.0). Only
the normal Jhol lockfile route and Bun passed all pilot checks (2/6 samples).
The normal Jhol arm took 1,479 ms and Bun 2,139 ms (1.45× for this single
sample); this is not a performance claim or confidence interval. JGrid and
JAGR-S samples were invalid because their selected routes did not execute.
Harvested, prepared-tree, and local-cache modes still need separate seeded
trials. Jhol measured 7,604,102 downloaded bytes for the passing registry-cold
sample. Earlier smoke reports remain as evidence of stale fixture state and
the first overly strict executable-link comparison.

## Comparison protocol

Keep registry-cold (empty package-manager caches) separate from local-cache reuse (declared SRI/local artifact inventory). Keep prepared-tree replay and unchanged no-op separate from both. Record lockfile and resolver routes separately; a sample is invalid if requested route did not run. Preserve failures in raw JSON. Compare installed identity/path inventories, executable command names, lockfile stability, and CJS/ESM imports. The one-round pilot is not the 30-round acceptance run. No speed claim passes with a quality failure.

## Current cold evidence (2026-10-03)

- Accepted owner-updated JAGR-S paper as a new immutable input; current digest recorded in work progress. All three current papers have been read.
- Valid matched npm-lock React pilot on installed upgrade: Jhol 1,300.38 ms / Bun 603.87 ms, 6/6 valid samples. Supersedes the earlier malformed-fixture timing comparison for current evidence.
- Current source stage profile: fetch/extract 948–1,110 ms; index/store relinking 201–389 ms; projection 8–11 ms. Stage timers and exact binary snapshots retained.
- Candidate reuses Node's project ancestry for cloned authoritative lockfile trees, retaining store repair only for explicit symlink mode. Five-round A/B pilot: baseline 1,193.47 ms; candidate 693.73 ms; Bun 809.05 ms. 15/15 matching identities/paths and successful runtime/build checks. Short pilot does not establish a general advantage or 4x.
- Full 30-round comparison remains underway. Multi-source replay, verified seeded JGrid/JAGR-S comparisons, mathematical reference checks, and the multi-fixture/platform acceptance gates remain incomplete.

### 30-round update (2026-10-04)

`cold-clone-layout-ab-20261004-000000.json`: flat React lockfile route, candidate 949.87 ms / baseline 1,038.43 ms / Bun 644.61 ms median. Ninety samples pass exercised quality checks, with an exact required-path/version audit. 8.53% median reduction against saved baseline; Bun still 1.47x faster. 4x gate red. Counterexamples in nested/symlink publication reproduce on baseline; optimization narrowed to flat cloned layouts. This does not validate all paper claims or platform/workload gates.

### Installed upgrade and extraction updates (2026-10-04)

- Latest installed-binary registry-cold React/Vite comparison: 30 randomized rounds, 60/60 scoped correctness checks passed. Jhol median 967.56 ms / p95 1,176.38 ms; Bun 605.07 ms / p95 759.84 ms. Bun is 1.60× faster; 4× gate remains red. This supersedes the earlier installed-cold summary above for current numbers. Raw report: `installed-jhol-vs-bun-registry-cold-20261004-014958.json`.
- Current source five-round staged/streamed pilot: 918.94/950.58 ms median; all 10 samples passed scoped checks. A separate corrected three-round diagnostic measured 962.70/927.88 ms; all six passed. The medians conflict and streamed p95 was higher in the five-round pilot. Streaming stays opt-in; no speed claim.
- Fixed a profiler boundary error: the stream project timer had included the HTTP request while the staged timer began after download. New `[PROJECT-T]` scopes are explicitly different (`body-consume-and-project` versus `staged-file-project`). HTTP/2 `[HTTP-STREAM-T]` and `[HTTP-FILE-T]` records report 16 requests per run in the corrected diagnostic. Their summed durations overlap under concurrent requests and are not additive wall-time components.
- Telemetry repair compiled in `scripts/build_release_safe.sh`; `cache_recording_errors_prevent_extracted_tree_publication` passed 1/1. Source binary SHA-256: `6739eb1f845bc1e3187adda65408b224e7345ce150d7bafc2c77da0530202daf`. Raw extraction evidence: `stream-extract-source-ab-pilot-20261004-020313.json`, `cold-http-extract-stage-profile-20261004-021107.json`.

## ARM hashing implementation gap —2026-10-04

Installed RustCrypto sha2 0.10.9 requires its asm feature for ARM SHA256/SHA512 instructions. Core did not enable it. Opt-in hardware-sha Cargo feature enables runtime-detected SHA256 and SHA384/512 hashing, with portable fallback; current execution evidence covers Apple M4. Independent padding/chunk vectors and locked archive SRI checks validate identical outputs. Local release fixed-archive diagnostic: combined hash median31.482→8.461ms; full verified projection161.292→112.407ms. Registry results recorded separately; no4x cold claim inferred from this component.

This supports production acceleration of content and compressed-byte SRI hashing. It does not establish the paper's streaming-libdeflater pipeline, universal complexity guarantees, or uncompressed-byte SRI equation. npm lockfile SRI covers the compressed package bytes; production preserves that check. Papers unchanged.

Final cold gate:30 rounds baseline763.58ms/hardware802.95ms/Bun624.88ms; no established gain (paired ratioCI0.8734–1.1222).90 scoped checks/3builds pass. Default enablement held; retain optional component improvement only. See hardware-sha-cold-gate-2026-10-04.md.

## HTTP/2 startup-window correction — 2026-10-04

Real wire check proved adaptive mode discarded configured2MiB/4MiB windows. Corrected source:30 registry-cold rounds, Jhol705.63→605.88ms; Bun707.86ms. Before/afterCI1.0510–1.2318; Bun/afterCI1.0694–1.2735.90 scoped installs/3builds pass;537core tests pass. Supports this locked React/Vite workload only.4× and universal paper claims remain unsupported. [Evidence](http2-initial-windows-cold-gate-2026-10-04.md). Papers unchanged.


## Fresh walkthrough review — 2026-10-04

Read both repository walkthroughs and all three Jhol-related Antigravity walkthroughs in full; followed their two linked engineering reports. Their timings describe different conditions (registry, reusable cache, prepared tree, or unchanged project). The historical production head-to-head harness installs different dependency sets for Jhol and Bun and reuses persistent cache state; its results cannot establish registry-cold superiority. The walkthrough explanation that `DefaultHasher::new()` necessarily changes identity between processes is not established; do not infer a zero cache-hit rate from that explanation. Fixed-size hashes do not establish mathematical injectivity, and directory cloning does not establish constant-time end-to-end installation.

Current executable checks reproduced two unsafe shortcuts: C timestamp proof returned success with a required locked package absent; JAGR lockfile replay accepted missing/incompatible required child edges. Both are repaired with regression checks. A stricter fresh diagnostic also found five invalid successful unlocked-medium installs: native single-pass layout dropped a conflicting required `ms` edge when manifest metadata described only its hoisted version. These elapsed times are invalid performance evidence. Separate counterexample: `fresh-single-pass-counterexample-2026-10-04.json`. Repairs expand only missing child domains and validate every declared required edge against actual ancestor layout; measured release verification follows in the fresh walkthrough report. No paper was edited. Universal resolver/complexity and 4× cold claims remain unsupported.


## Deeper required-edge/reuse review — 2026-10-04

All three papers reread in full. Reproduced intermediate-ancestor validation
errors, missing-required-child false success, and a cache reuse false success
in both CLI modes. Consolidated ancestor lookup and expansion cache ownership;
required errors now propagate, optional policy preserved, duplicate clone relink
removed. Core544/0/3ignored; CLI30/30; front proof6/6; release local fixtures14/14.
See [causal evidence](deep-layout-review-2026-10-04.md). These checks support
scoped exact-instance/required-edge obligations; universal algorithms and4× cold
claims remain unsupported. npm SRI verifies compressed tarball bytes; the paper's
uncompressed-file description is not the implemented npm contract. No paper edits.


### 2026-10-04 metadata scheduling qualification

Existing discovery machinery now overlaps full-domain escalation and shares
immutable metadata. Six controlled independent conflicts preserve both version
consumers; request span923→58–76ms. Required HTTP failures fail closed. Thirty
accepted real-registry triplets show no established general cold speed win:
1590.69→1583.50ms, baseline/fixed95%CI[0.9775,1.1013]. Bun1545.30ms;4×red.
This supports a specific scheduling repair, not universal topology/SemVer
correctness, complexity or4× claims. Papers unchanged. See
[coordinator evidence](metadata-coordinator-cold-gate-2026-10-04.md).


### 2026-10-05 caller-integrity qualification

Session tarball hints previously bypassed a later caller's SRI; reproduced and
repaired. Verified session/in-flight identity binds package, URL, cache and SRI;
wrong concurrent caller fails; same request reuses locally with registry stopped.
This supports that request-context obligation, not complete persistent-cache or
capsule poisoning protection. Fresh cold diagnostic9/9 valid; legal resolved
closures differ and three rounds do not establish speed gain.4× stays unsupported.
[Evidence](request-sri-cold-review-2026-10-05.md). Papers unchanged.


### 2026-10-05 cold transfer integrity and scheduler

Exact-pin foreground path discarded registry `dist.tarball` and `dist.integrity`.
Saved binary reproduced false success on valid mutated gzip/tar when registry SRI
stayed unchanged; fixed binary rejects. URL and SRI now share discovery metadata,
transfer identity and lock output. This supports end-to-end transfer provenance
for the exercised routes. Controlled24-node frontier fixture also reproduced
blocking prefetch on shared Rayon workers: existing bounded IO pool cut fixture
median1901→1308ms (5 interleaved rounds, all correctness checks passed). Keep
opt-in; five-real-round fixture differs in valid hoisting and does not execute the
changed flat-transfer branch. No 4× or universal speed claim. Papers unchanged.
[Full evidence](cold-transfer-descriptor-review-2026-10-05.md).
