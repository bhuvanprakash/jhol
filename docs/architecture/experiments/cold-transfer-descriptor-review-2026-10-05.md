# Cold transfer descriptors and discovery contention — 2026-10-05

Status: URL/SRI propagation repaired. Prefetch scheduling candidate remains opt-in.
4× cold acceptance remains NOT GREEN.

## Core issue and minimal repair

Discovery fetched exact package versions and their `dist.tarball`/`dist.integrity`.
Prefetch used them. Later foreground exact-pin transfers guessed canonical URLs
and supplied no SRI. New request-context verification correctly refused to join
those different requests. With custom registry URLs, foreground installs failed;
with canonical URLs and valid mutated archives, installs could falsely succeed.
Version-only lattice/binary hints were also treated as transfer descriptors.

Use existing `ResolvedFetch` and `ManifestThenPackumentStrategy` for every cold
no-lock transfer. Exact selected versions first reuse Arc-backed discovery
metadata or existing registry metadata cache. Missing descriptors use the
existing manifest/packument machinery. Remove the duplicate exact-pin branch
and version-hint URL shortcuts. Preserve existing authoritative lockfile entries;
when absent, lock writing reuses upstream metadata before deriving local hashes
or guessing URLs. No new dependency, pool, subsystem or competitor implementation.
Prechange sources archived; papers untouched.

## Runnable correctness evidence

- Baseline local registry,24 exact packages in six chains: both routes request
  guessed archive URLs and fail when prefetch disabled. Invalid timings retained
  in `/tmp/jhol-prefetch-frontier-before.jsonlog` and `-disabled.jsonlog`.
- Fixed custom-URL fixture: default, foreground-only and opt-in IO routes each
  request all24 advertised archives exactly once, execute every root consumer,
  and preserve every upstream URL/SRI in generated locks.
- Stronger canonical-URL counterexample: valid gzip/tar, package versions
  unchanged, JS leaf exports mutated, published SRI unchanged, no prefetch.
  Saved binary returns0 and writes success markers in both CLI modes. Fixed
  binary returns1 on integrity mismatch, no success marker. This demonstrates
  foreground SRI propagation, independently of archive-path or parser failure.
- Earlier appended poison failed gzip parsing, so it did not prove SRI rejection;
  replaced with valid mutated archives. Debug orchestration shadowed a Python
  builtin; initial Bun control assumed an npm lockfile. Both harness errors
  repaired, aborted runs excluded from timing evidence and retained under `/tmp`.

[Canonical SRI counterexample and exact runnable fixture](cold-transfer-descriptor-sri-20261005.json).

## Measured scheduling bottleneck

Archive prefetch uses default Rayon workers also needed for dependency discovery.
An archive job can block those workers on HTTP admission or transfer latency.
`JHOL_PREFETCH_IO_POOL=1` sends these jobs through Jhol's existing bounded IO pool;
`0` retains default behavior. Required validation and four transfer slots remain.
No change to pool sizing, archive verification, script policy or source selection.

Controlled registry:24 packages, four dependency levels, metadata20ms,
archive200ms, `RAYON_NUM_THREADS=4`, fresh project/HOME/tool cache per sample.
Five randomized interleaved rounds; both Jhol CLI modes per condition:

- Default prefetch median1901.34ms; final leaf metadata median1418.25ms.
- Existing IO pool median1307.81ms; leaf180.24ms.31.2% lower install median
  within this constrained fixture; source/correctness checks pass.
- Foreground-only median1495.63ms; leaf197.01ms.
- Bun control median814.55ms. Same24 advertised archives and consumers.
- All35 valid installs and four poisoned-archive checks pass their scoped gates.

This proves worker interference on this fixture. It does not establish universal
cold speed, optimal production scheduling, cross-platform benefit or4× Bun.
[Raw controlled samples, build hashes and exact fixture source](cold-transfer-descriptor-fixtures-20261005.json).

## Real registry diagnostic and limits

Five randomized interleaved rounds: React18.2.0, TypeScript5.7.3, Express4.21.2;
fresh projects/HOME/tool caches, reuse disabled. No owned build/test work during
timing. All20 scoped package/required-peer/CJS/ESM/bin checks pass.

Point medians: saved1450.12ms; fixed default2619.51ms; opt-in IO1394.32ms;
Bun1447.96ms. These do not establish performance improvement: only five rounds,
variable network service times and nonidentical legal layouts. Jhol has76 physical
instances, Bun73; Jhol hoisting varies across runs. All Jhol arms report76 archive
operations. The medium fixture executes nested layout, so it does not exercise
the newly consolidated flat foreground transfer strategy. Do not attribute its
latency to that strategy or claim a general47% improvement from default/IO points.

[Real registry observations and embedded runner/checker](cold-transfer-descriptor-diagnostic-20261005-010547.json).

## Verification and preserved inputs

- Before edit: scoped install23/23; baseline failed fixture recorded.
- Final HTTP/2 core546 passed,0failed,3ignored; watchdog peak62%.
- Final HTTP/1 core520 passed,0failed,3ignored; peak65%.
- Final CLI30/30; peak66%.
- First safe release1m57s,peak67%,two jobs,LTO disabled,16 codegen units.
  Binary `a4bf6a1aff16d7adac6c83114d25cc3cfb73dbe4328534b608565dfac0a9c92e`.
- Safe release build passed in2m09s, peak67%; two jobs, LTO disabled,16 codegen units. Subsequent source restoration touched only pre-existing rustfmt whitespace. Blueprint verify passed on an isolated copy; no repository example writes.
- Papers retain current hashes; installed binary remains8b186ca8a6ca045e.
-120 catalog documents hash-unchanged; authored changelog/archive deltas reviewed;
  claim ledger reread. Prior walkthrough/paper full reads retained. Referenced
  architecture chat reread; latest Antigravity full tool-corpus reasoning review
  remains open. No new full-reading claim.

## Next gate

Keep IO scheduling opt-in. Reproduce varying canonical hoisting on immutable
metadata; compare matched package identities before broad cold claims. Use
controlled stage measurements to separate remaining archive critical path from
metadata contention. Preserve every required-edge, SRI, containment and project
isolation check. Broad paper algorithm proofs, mixed-source matrix, workspace,
platform and30-round4× acceptance gates remain open.
