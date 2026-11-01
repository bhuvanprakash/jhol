# Request-bound tarball verification — 2026-10-05

Status: verification-context defect repaired; 4× cold target remains NOT GREEN.

## Reproduction and repair

Session reuse accepted a package/URL hash hint solely because tarball and
completed tree existed. A later caller's different expected SRI was ignored.
In-flight deduplication also keyed only package, URL and cache directory, so
callers requiring different hashes could share one verification result.

New regression failed before edits: wrong-SRI request returned `Ok(hash)`.
Use one typed key in the existing session map: legacy lookup hints retain their
public API; verified request entries bind absolute cache directory, package,
URL and exact expected SRI. In-flight entries use the same identity. Publish
verified session entry only after download/verification succeeds. Removed the
second inner session-hint bypass. No new dependency, production module, SRI
exception, backend algorithm, paper edit or competitor code.

Concurrent real-tarball fixture verifies different SRI requests independently;
wrong caller fails. Registry is then stopped: matching replay still succeeds
locally. This proves reuse without inferring request provenance from aggregate
counts. None/different SRI identities cannot coalesce. Persistent store mutation
and broader cache/capsule poisoning gates are not certified by these two checks.

## Verification

- Core with HTTP feature:546 passed,0 failed,3 ignored; watchdog peak66%.
- CLI30/30; peak66%.
- Two request-reuse checks pass with HTTP feature and default core HTTP/1 route;
  HTTP/1 scoped peak66%.
- Release plain/native SRI/CJS/bin checks2/2; nested cold/fresh-project reuse4/4.
- Safe final release1m59s,peak67%,two jobs,LTO disabled,16 codegen units.

Network fixture uses an isolated child with harvest/mirror disabled and owned
cache paths, setting policy before worker startup. Initial aggregate request
assertion counted transport retries as replay requests; corrected registry-off
check supersedes that assertion. Failures retained in raw evidence. Fixture
hash artifacts were not found in machine origin cache. Baseline source archived
under `archive/experiments/request-sri-2026-10-05/`; no existing caches removed.

Reproduce:

```sh
JHOL_NO_HARVEST=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- cargo test -p jhol-core --features http-reqwest --lib tarball_reuse
JHOL_NO_HARVEST=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- cargo test -p jhol-core --lib tarball_reuse
```

[Regression checks, preserved hashes and release fixtures](request-sri-regressions-20261005.json).

## Fresh cold diagnostic, not a performance acceptance claim

Three randomized interleaved triplets: fresh projects/HOME/tool caches, identical
react18.2.0/typescript5.7.3/express4.21.2 roots, reuse disabled, no owned builds or
correctness jobs during timing. All9 root/required-peer/CJS/ESM/bin checks pass.

Point medians: saved989.63ms, fixed905.63ms, Bun1645.06ms. Three rounds do not
establish a speed gain. Jhol installs76 instances; Bun73. `get-intrinsic` differs
(1.3.1 versus1.3.0), with three additional required packages in the Jhol closure.
Both satisfy declared ranges. Round1 Jhol roots/hoisting also differ despite
valid consumers. These are not identical closure/layout comparisons. Do not
claim1.8× or4× acceptance from these observations.

Each Jhol observation reports76 archive operations/5,065,926 body bytes. Fixed
archive-operation sums1725–1821ms; dividing by current four slots gives431–455ms
under measured service times. This exceeds Bun's point4× budget411ms even before
other serial work. It is a bound for this observed scheduling model, not a
universal network floor or evidence that increasing concurrency would win.
Prior8-slot locked-workload quality/performance rejection remains relevant.
Metadata/discovery stage includes overlapping prefetch, not just metadata RTT.
Recorded line arrival minus rounded duration estimates request lifetimes;
logging, retries and pipe delay prevent claims about precise wire/start order.

[Raw samples, identities, exact embedded runner/checker and request timeline](cold-request-sri-diagnostic-20261005-002825.json).

## Reading and limits

115 catalog documents unchanged on hash check. Re-read three changed experiment
reports fully; five changed work/changelog/archive/ledger documents reviewed by
prior text plus authored deltas. Papers unchanged. Prior walkthrough reviews
stand. Full latest Antigravity tool-output reasoning review remains incomplete.

Main,63 pre-existing deletions, zero staged files and installed binary preserved.
Repo candidate SHA256 `aa4234d350c7ce2c057b8330e49cc47c1d499a7d6cf55078e54ddfcc09731db6`.
Further work must reproduce discovery/prefetch interference and canonical
resolution/hoisting behavior before editing scheduling. General4×, full paper
claims, workspace-wide and cross-platform acceptance remain open.
