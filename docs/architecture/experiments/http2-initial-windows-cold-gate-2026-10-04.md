# HTTP/2 initial windows: verified cold improvement, 4× target unmet

## Cause and narrow fix

Jhol configured a 2 MiB stream window and 4 MiB connection window, then enabled adaptive flow control. Installed reqwest 0.12.28 and hyper 1.11.1 override both explicit limits in adaptive mode. Actual client SETTINGS/WINDOW_UPDATE frames reproduced `(65535, 65535)` before the fix, and `(2097152, 4194304)` afterward. Production now disables adaptive window sizing; explicit bounded windows apply from the first response. TLS/ALPN, authentication, proxies, retries, request concurrency, SRI, gzip/archive checks, EOF validation, cache publication, and exact package placement retain their existing paths.

Production source: `crates/jhol-core/src/http2_download.rs`. Builder construction is shared with a loopback wire regression; only the test uses cleartext HTTP/2 prior knowledge. No production protocol forcing, IP forcing, new dependency, or competitor implementation was added. No scheduling change was made. Earlier completion logs establish finish order only, not request start order.

## Registry-cold result

One 30-round randomized interleaved comparison, 90 total timed installs. Existing pinned React 18.2/react-dom 18.2/Vite 5 lockfile, 16 host-platform packages and five executable links. Each sample gets a fresh project, HOME and tool cache. Harvest, JGrid/L0, JAGR-S mirror, prebuilt/prepared reuse, daemon, and streaming extraction disabled. Every Jhol sample recorded exactly 16 HTTP/2 archive transfers and 7,610,763 body bytes. Download bodies were not seeded. CDN/OS caches and unrelated host load were uncontrolled. No owned builds ran during the timed span.

- Saved Jhol: median **705.63 ms**, p95 **931.81 ms**.
- Corrected Jhol: median **605.88 ms**, p95 **749.23 ms**.
- Bun 1.4.2: median **707.86 ms**, p95 **971.07 ms**.
- Jhol median elapsed time falls **14.14%**. Before/after ratio **1.1646**, paired bootstrap 95% interval **[1.0510, 1.2318]**.
- Bun/corrected Jhol ratio **1.1683**, paired bootstrap 95% interval **[1.0694, 1.2735]**.
- **4× gate red.** This run's Bun median would require Jhol ≤ **176.97 ms**, plus lower 95% confidence bound ≥4.

10,000 paired bootstrap resamples of round IDs; statistic is ratio of medians. Exploratory within-run evidence, without correction for the historical sequence of rejected hypotheses. These findings support this locked flat workload on this macOS machine; unlocked resolution, Next.js, nested/workspace performance, other networks/platforms, and universal research claims remain unverified.

## Attribution

- Fetch/extract stage median: **627.5 → 547 ms**.
- Largest archive, esbuild 4,036,141 bytes: total transfer median **260 → 195.5 ms**; body-reader median **156 → 116 ms**; TTFB **39.5 → 40.5 ms**.
- Sum of overlapping per-package projection timers: **585.5 → 589.5 ms**. This is not install wall time; no extraction CPU gain asserted.
- Final project projection stage median: **14.5 → 12 ms**.

The wire reproduction identifies the configuration conflict; transfer-stage measurements support a benefit from correcting it. DNS/IP-family forcing was rejected: a single curl pair measured IPv4 709.204 ms and IPv6 638.442 ms; this does not establish a family-specific defect. No network-wide impossibility claim follows from these observations.

## Quality and scope

- **90/90 installs pass** locked identities, CJS/ESM consumers, executable links, and lock stability.
- **3/3 representative Vite builds pass**, outside timed spans.
- Before/after Jhol package trees, inventories and lockfile hashes identical.
- Baseline HTTP checks **25/25**; corrected checks **26/26**, including truncated body handling and actual wire settings.
- Corrected core **537 passed, 0 failed, 3 ignored**; CLI integration **30/30 passed**.
- Safe release build **2m16s**, memory watchdog peak **65%**; benchmark watchdog peak **61%**. These percentages describe whole-machine memory, not per-process peak RSS. Per-process RSS not measured; raw CPU fields include untimed quality subprocesses.
- Plain and native-only cold request-budget/SRI/CJS/bin checks **2/2**.
- Three paper SHA256 hashes unchanged. Latest Antigravity visible chat/checkpoint review remains current; its tool-result corpus was not fully reviewed. Recent authored Markdown hash delta zero before this new record.
- Installed user binary preserved. Corrected repo release retained after the within-run improvement and quality gates passed. Default hardware-SHA feature remains off. No Git writes.

## Reproduction and raw evidence

[Raw observations, exact fixture inputs, runner, analysis source, source diff, binary hashes and quality outputs](cold-h2-window-ab-20261004-162230.json).

Baseline binary SHA256: `9d7a6a78886ea41d5fbce294408ec5ab00e543e87f3a5ab328bc7b17d8aa6c67`.
Candidate binary SHA256: `9ec1bcbcdf2ea3aa1b2f2ae58cd9204c38029cf82675a4c7b611617fd1d736f7`.
Bun SHA256: `35d20dd0263e5c950194434b925454fdfa9ba6e4467da960410fa05b08a7a5b5`.

Saved current source/binary: `/tmp/jhol-h2-window-backup-20261004-161330/`. Immediate sample journal: `/tmp/jhol-h2-window-samples.jsonl`. Exact runner and analysis are embedded in raw evidence; temporary project paths are historical after runner cleanup. Scoped regression command:

```sh
CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- cargo test -p jhol-core --lib --features http-reqwest http2_download::tests
```

Workspace/blueprint and cross-platform gates were not rerun for this narrow pass. CLI integration passed after the timed comparison.
