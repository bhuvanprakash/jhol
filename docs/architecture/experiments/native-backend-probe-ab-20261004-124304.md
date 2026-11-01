# Native backend probe removal: focused evidence

One bottleneck: CLI launched `bun --version` before every native install even with fallback disabled. Native install, CI, and native daemon requests now select the unused enum without subprocess probing; enabled fallback retains detection. The lockfile-only path also reuses the selected backend instead of probing again.

## Results

- Fresh-project offline local-cache installs, five randomized interleaved rounds per arm: Jhol before 67.88 ms, after 42.18 ms, Bun 25.04 ms. Jhol median reduced 37.9%. Pilot, not a final confidence-bound claim.
- Registry cold, three randomized interleaved rounds per arm: Jhol before 526.86 ms, after 610.94 ms, Bun 754.34 ms. This pilot does **not** establish a registry-cold improvement: candidate was slower than saved baseline, while both were faster than Bun here. Network variation dominates; retain the conflicting historical 30-round result separately.
- All 24 installs passed existing route, installed identity/layout, executable-link, CJS/ESM, and lock-stability checks. Jhol before/after full-tree digest, full inventory, lock digest, and bin names match across both conditions.
- Final safe release build passed; CLI integration30/30 passed under two jobs and memory watchdog. Regression covers eight CLI/config cases, including lockfile-only.
- Causal check: saved baseline invoked fake Bun once with `--version`; candidate invoked it zero times. Probe regression test covers CLI flags/config, native-only precedence, native CI, and fallback detection.
- Integrity, extraction, resolution, and publication implementation unchanged by this fix. Three paper hashes remain unchanged. Four-times target unachieved.

## Reproduction

Run under the memory watchdog, with the saved baseline binary retained:

```sh
CARGO_BUILD_JOBS=2 python3 scripts/mem_guard.py -- python3 scripts/bench/native_backend_probe_ab.py --baseline /tmp/jhol-native-probe-backup-20261004-123916/target/release/jhol --candidate target/release/jhol --bun /Users/bhuvanprakash/.bun/bin/bun
```

Raw observations: [native-backend-probe-ab-20261004-124304.json](native-backend-probe-ab-20261004-124304.json). Measured binary hashes are in that JSON. Final binary rebuilt after the additional lockfile-only call-site repair; its hash is recorded separately. Timings use immediate subprocess completion; no RSS polling. CPU fields include post-timing quality subprocesses and are not install-only CPU. Both local caches were privately seeded from identical lockfile closure, then measured offline; registry-cold samples used fresh empty caches. OS/CDN caches uncontrolled.
