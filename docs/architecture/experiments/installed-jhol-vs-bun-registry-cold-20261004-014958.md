# Installed Jhol versus Bun: registry cold rerun, 2026-10-04

30 randomized interleaved rounds per tool. macOS arm64. Locked React 18.2.0, react-dom 18.2.0 and Vite 5.0.0; 16 installed package identities and five executable link names. Fresh projects and empty tool caches each sample. Daemon, harvest and prebuilt reuse disabled. Install wall timing excludes post-install quality/build checks. OS and registry/CDN caches uncontrolled.

- Installed Jhol 1.0.2: median **967.56 ms**, p95 **1176.38 ms**, median bootstrap 95% CI [718.48, 1036.15] ms.
- Bun 1.4.2: median **605.07 ms**, p95 **759.84 ms**, median bootstrap 95% CI [591.70, 655.35] ms.
- **Bun 1.60x faster** by median. Bun/Jhol ratio 0.6254; lower paired bootstrap 95% bound 0.5795. **4x gate red**. Point target here would require Jhol <= 151.27 ms; confidence bound must also reach four.
- **60/60 exercised checks pass**: required locked versions at exact paths, unchanged npm lock, same package inventory and executable link names, CJS resolution/ESM imports, React server rendering, Vite production build. No invalid samples or observed resolver drift. This does not prove complete byte equality, hostile-input integrity implementation or other platforms/workloads.
- Background Virtualization and StorageManagement CPU activity observed; host load changed during the run. No unrelated process stopped. Live-laptop result, not controlled idle-host evidence.
- Installed Jhol still publishes L0 despite explicit JGRID disable. Publication cost included; no prior prepared tree reused. Jhol downloads 7,610,763 bytes per round; Bun bytes unavailable.
- Installed Jhol hash `8b186ca8a6ca045e82162321fc3cf6e7fe1841aaba2b5268f3c99dc40f79c518`; Bun `35d20dd0263e5c950194434b925454fdfa9ba6e4467da960410fa05b08a7a5b5`. Hashes unchanged before/after and identical to prior installed comparison. Source candidate not substituted; installed build feature provenance unavailable.
- CPU counters include import checks. Memory watchdog exit 0; peak system memory used 73%.

[Raw observations, fixture, runner and host-load observation](installed-jhol-vs-bun-registry-cold-20261004-014958.json). Papers untouched. No Git writes.
