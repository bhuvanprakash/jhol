# Installed Jhol versus Bun: registry cold, 2026-10-04

30 randomized interleaved rounds per tool on macOS arm64. Fresh project, isolated empty HOME and tool cache each install. Locked React 18.2.0, react-dom 18.2.0, Vite 5.0.0. Daemon, prebuilt and harvest reuse disabled. OS and registry/CDN caches uncontrolled.

- Jhol 1.0.2: median **912.81 ms**, p95 **1379.36 ms**; median bootstrap 95% CI [850.66, 974.81] ms.
- Bun 1.4.2: median **573.76 ms**, p95 **786.89 ms**; median bootstrap 95% CI [530.78, 617.24] ms.
- Bun is **1.59x faster** by median ratio.
- Bun/Jhol ratio 0.6286; lower paired bootstrap 95% bound 0.5683. **4x gate red**; point target would require Jhol <= 143.44 ms, with confidence bound also passing.
- **60/60 exercised checks pass**: same 16 locked package identities/paths, five executable link names and runnable targets, required locked versions at exact paths, unchanged npm lock, CJS resolution/ESM imports, React server render, Vite production build. These checks do not prove complete byte equality, integrity implementation correctness, hostile-input safety, or other fixtures/platforms.
- Installed Jhol downloads 7,610,763 bytes each round. Bun bytes unavailable. Installed Jhol publishes L0 despite explicit JGRID disable; publication cost included. No pre-existing prepared tree used.
- Binary SHA256 unchanged during experiment: Jhol `8b186ca8a6ca045e82162321fc3cf6e7fe1841aaba2b5268f3c99dc40f79c518`; Bun `35d20dd0263e5c950194434b925454fdfa9ba6e4467da960410fa05b08a7a5b5`. Installed executable tested; pending source changes were not rebuilt or substituted. Build feature provenance unavailable.
- Install wall timing excludes post-install quality/build work. Matrix child CPU values include its import checks; do not interpret as pure install CPU. Memory watchdog finished exit 0, peak system memory used 68%.

[Raw observations, immutable fixture inputs, exact runner source and binary hashes](installed-jhol-vs-bun-registry-cold-20261004-011248.json). Research papers unchanged; no Git writes.
