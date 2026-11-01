# Cold install: remove redundant registry-root requests

2026-10-04, macOS arm64. One scoped network-waste repair; no competitor source used.

## Change and causal check

HTTP/2 pool construction launched two background root GETs while real package requests started. Plain CLI startup also launched a root HEAD. Removed automatic probes; explicit prewarm APIs, pooled clients, bounded admission, retries and all download/extraction/integrity checks remain.

Local request-budget check: saved baseline plain install fetched one archive plus three root probes; native-only fetched one archive plus two probes. Candidate fetches only the one archive in both cases. Both preserve package load, executable link and locked SRI. The local server's short delay makes background requests observable; these cases are not speed benchmarks.

## Registry-cold pilot

Nine randomized interleaved rounds per variant; same valid locked React 18.2/react-dom 18.2/Vite 5 fixture. Fresh project, HOME and empty tool caches for every sample; harvest, prepared trees, daemon and JGrid reuse disabled. OS/CDN caches and host load uncontrolled. HTTP/install profiling enabled. Wall timing excludes quality and build commands; CPU fields include quality commands, RSS unavailable.

- Saved Jhol: median 1,099.39 ms, p95 1,695.49 ms.
- Candidate Jhol: median 1,085.20 ms, p95 1,451.54 ms.
- Bun 1.4.2: median 711.25 ms, p95 858.79 ms.
- Before/after ratio 1.0131, paired bootstrap 95% CI [0.8524, 1.2064]. **No established cold speed gain.** Retained as a request-budget fix, not a performance breakthrough.
- Bun/candidate ratio 0.6554, 95% CI [0.4637, 0.7340]. Bun is 1.53x faster by median; **4x gate red**. Point target on this workload would require Jhol at most 177.81 ms, with lower confidence bound also reaching four.
- All 27 installs pass exercised identity/path/bin/CJS/ESM/lock checks. Three representative Vite builds pass. Before/after Jhol full-tree digests, inventories and lock hashes match. Each Jhol run proves 16 HTTP/2 archive requests / 7,610,763 reported archive bytes.
- Median candidate fetch/extract stage 1,010 ms versus 1,031 ms baseline. Projection 21 ms; capsule/markers 20 ms. Stage aggregates do not isolate request wait versus extraction, and concurrent per-request times cannot be added as wall time.

## Verification and rejected work

Baseline and candidate HTTP tests: 25/25 each. Candidate full core: 535 passed, 0 failed, 2 ignored. Safe release build passed, peak system memory75%. CLI suite: 30/30 passed, peak system memory62%. Existing unrelated rustfmt wrap at HTTP truncation error remains; touched pool hunk introduces no formatting mismatch.

Rejected earlier 128 KiB gzip-buffer experiment: decoder read calls fell 270 to 5, but debug archive benchmark median worsened 654.27 to 664.88 ms. Reverted production change and archived experiment; read-call reduction is not a speed win. All three research-paper hashes preserved.

Scheduler inspection found rolling admission, not a four-package batch barrier. No scheduler change made. Existing rejected concurrency sweeps are not repeated.

Pre-change source/experiment snapshots: `archive/experiments/cold-root-probes-2026-10-04/`, with entries in `archive/MANIFEST.md`. Saved executable and backup manifest: `/tmp/jhol-cold-prewarm-backup-20261004-133028/`. Source changes uncommitted; installed user binary untouched.

Reproduce request-budget regression: `CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- python3 scripts/bench/check_cold_request_budget.py target/release/jhol`. Append `--expect-probes` with saved baseline to reproduce old traffic. Raw report below embeds complete cold runner and fixture inputs; reconstruction requires saved baseline and current candidate binary.

[Raw samples, hashes, fixture, request controls and runner](cold-root-request-ab-20261004-134150.json).
