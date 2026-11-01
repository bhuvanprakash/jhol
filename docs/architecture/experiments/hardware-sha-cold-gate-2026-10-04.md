# Hardware SHA: CPU gap repaired, registry-cold gate not passed

## Measured gap and implementation

Installed sha2 0.10.9 uses software SHA on ARM unless its asm feature is enabled. Apple M4 supports SHA256/SHA512 instructions. Reuse RustCrypto runtime dispatch; preserve software fallback. Root/core optional Cargo feature: `hardware-sha`. No download, resolver, extraction or publication algorithm changed. One transitive backend package sha2-asm0.6.4; no new direct dependency.

The initial measured build enabled this backend for ARM by default. Its exact binaryeecfeb6e400fd92a0dcd926aa878dfd84553f26c023afeca9842ad61b4b63653 is retained in the saved input backup; measured manifest archived under `archive/experiments/arm-hash-default-2026-10-04/`. Final packaging makes it opt-in. Default binary/source behavior preserved. Final opt-in packaging build gets separate hash/smoke evidence; these timings are not assigned to a different binary.

## Fixed-byte local diagnostic

Verified esbuild archive4,036,141 compressed bytes;9,760,768 expanded bytes. Lockfile SRI independently verified with Python hashlib. Release-mode15 hashing/inflate samples,5 complete projection samples per build; sequential profiles, host noise retained. This is not registry cold.

- SHA256+SHA512 median31.482→8.461ms,3.72x component ratio.
- Full verified projection161.292→112.407ms,30.3% lower point estimate.
- Flate2 inflation55.146→50.140ms; libdeflate reference22.866→19.426ms. Control variation shows host effects; no native decompression change selected.
- Diagnostic retained at `fast_stream::ssdp::tests::profile_cold_archive_costs`. Its libdeflate reference only handles this verified single-member input; production keeps MultiGzDecoder and compressed-byte SRI.

## One30-round interleaved registry-cold comparison

Fresh projects, HOME and tool caches; identical React18.2/react-dom18.2/Vite5 lock; reuse/daemon/prebuilt/JGrid disabled; staged extraction; no builds during timing. OS/CDN caching and host activity uncontrolled. Both Jhol arms prove16 transfers and7,610,763 reported archive bytes per run. CPU fields include quality commands; install-only CPU and RSS unavailable.

| Binary | Median ms | p95 ms | sample p99 ms | Scoped checks |
|---|---:|---:|---:|---:|
| before | 763.58 | 1229.96 | 1375.53 | 30/30 |
| after | 802.95 | 1363.46 | 1445.10 | 30/30 |
| bun | 624.88 | 1129.06 | 1241.01 | 30/30 |

- Baseline/candidate median ratio0.9510; paired-bootstrap95%CI[0.8734271251394562, 1.1222080189039325]. No established cold gain; candidate median5.16% higher.
- Bun/candidate ratio0.7782;95%CI[0.6953707863476984, 1.242842680803492].4x gate red. Bun is1.28x faster by median. Point4x target here≤156.22ms, plus lower confidence bound≥4.
-90/90 scoped identity/layout/bin/CJS/ESM/lock checks;3/3 Vite builds. Jhol before/after trees, inventory and lock digests identical.
- Candidate aggregate fetch/extract median716ms; placement13ms. Fixed CPU gain does not remove the measured network critical path. Summed per-request/project times overlap and are not wall-stage decomposition.

## Quality and publication decision

- Baseline SSDP8 passed/2ignored; hardware core536 passed/3ignored;CLI30 passed. Independent Python hash padding/block/chunk vectors112 combinations passed.
- Final opt-in vector check1/1; safe build with `--features hardware-sha` passed; default feature graph excludes asm and explicit feature graph includes it. Final smoke/default core result added below.
- No cross-platform execution claim. Optional backend only verified on Apple M4. No4x cold claim, default enablement, paper edits, Git writes or competitor code copying.
- Keep this measured CPU improvement opt-in. Do not present hardware hashing as the demonstrated dominant cold-install bottleneck. Do not repeat rejected scheduling/window/buffer/range trials.

Reproduce opt-in build: `scripts/build_release_safe.sh --features hardware-sha`. Reproduce local diagnostic under two-job memory guard: `JHOL_PROFILE_ARCHIVE=/path/to/verified.tgz CARGO_BUILD_JOBS=2 CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 python3 scripts/mem_guard.py -- cargo test -p jhol-core --features http-reqwest,hardware-sha --release profile_cold_archive_costs --lib -- --ignored --nocapture`.

[Full raw samples, fixture, hashes, profile, embedded runner and build checks](cold-hardware-sha-ab-20261004-153232.json).

Final packaging checks: default core536 passed/3ignored, final hardware vector1/1 and two cold request-budget/CJS/bin/SRI smoke cases pass. Opt-in packaged binary `target/release/jhol-hardware-sha` SHA256`c4d3b16c13538be28c11aabc3f80d4fd7178549a2335b2a4a73b49fb699fb545` retained locally. Default release restored9d7a6a78; installed binary untouched. Packaged opt-in binary was smoke checked, not assigned the earlier measured binary timings.
