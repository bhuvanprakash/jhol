<h1 align="center">JHOL</h1>

<p align="center">
  <strong>One graph and one lockfile for npm packages, Python wheels and model weights.</strong><br/>
  npm-compatible &nbsp;·&nbsp; offline-first &nbsp;·&nbsp; verified &nbsp;·&nbsp; deterministic
</p>

<p align="center">
  <a href="https://crates.io/crates/jhol"><img alt="crates.io" src="https://img.shields.io/crates/v/jhol?style=flat-square&color=5b3cc4" /></a>
  <a href="https://github.com/bhuvanprakash/jhol/releases"><img alt="release" src="https://img.shields.io/github/v/release/bhuvanprakash/jhol?style=flat-square&color=5b3cc4" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-stable-5b3cc4?style=flat-square" />
  <img alt="platform" src="https://img.shields.io/badge/validated-macOS%20arm64-5b3cc4?style=flat-square" />
  <a href="./LICENSE"><img alt="license" src="https://img.shields.io/badge/license-Jhol-5b3cc4?style=flat-square" /></a>
</p>

<p align="center">
  <a href="./GET_STARTED.md"><strong>Get started</strong></a>
  &nbsp;·&nbsp;
  <a href="./docs/user/main.md"><strong>Documentation</strong></a>
  &nbsp;·&nbsp;
  <a href="./CHANGELOG.md"><strong>Changelog</strong></a>
  &nbsp;·&nbsp;
  <a href="#commands"><strong>Commands</strong></a>
</p>


```sh
cargo install jhol
jhol install
```

| | |
|---|---|
| Native install, doctor, audit | No Node, Bun, or npm on the hot path |
| Content-addressed store | A package is unpacked once, then linked |
| `--offline` | Install from the store when the network is gone |
| `--frozen` / `jhol ci` | The locked tree, reproduced |
| `jhol verify` | Exit 0 only when the tree **is** what the lockfile authorises |
| `--as-of`, `--allow-license`, `--exclude-vuln` | History, licences and CVEs are resolution inputs, not audits |
| `jhol ai`, `jhol py` | Model weights and Python wheels through the same graph |
| `jhol attest`, `jhol replay` | A deterministic proof, and a spec that reproduces it |

**The property that matters:** a fast path may save work, it may not decide truth. One
freshness authority (`jhol-treecheck`) answers `Match · Missing · Wrong · Extraneous ·
Unverified` before any install lane and after every one, so a hand-edited package, an
unauthorised directory and a deleted file are all the same question.

---

## Measured on this machine

Interleaved runs, fresh cache and fresh `HOME` for each tool, and every run had to produce a loadable tree.

| Scenario | jhol | Bun 1.3.14 | What happened |
|---|---:|---:|---|
| Cold, bundled path, 6 deps | **250 ms** | 291 ms | jhol won 5 of 5 rounds |
| Cold, 20 deps, ~70 packages | 2931 ms | 2793 ms | Split 2–2 |
| Warm re-install, 74 packages | **8–9 ms** | ~6 ms | Store links, no re-download |
| Daemon cold install | **239 ms** | — | Was 30,206 ms before the accept-loop fix |

Real npm projects, isolated cache and fresh `HOME` per tool, interleaved:

| Project | jhol | Bun | |
|---|---:|---:|---|
| express@4 — 71 packages | **646 ms** | 1417 ms | 2.2× ahead |
| webpack@5 — 57 packages | **1974 ms** | 4738 ms | 2.4× ahead |
| eslint@9 — 87 packages | **1652 ms** | 1732 ms | parity |
| jest@29 — 266 packages | **4961 ms** | 33082 ms | 6.7× ahead |
| express@4, `node_modules` wiped | **11–12 ms** | 26–29 ms | **~2.4× ahead** (restored tree snapshot) |
| jest tree, nothing to do | **18 ms** | — | verify, then stop |

Warm rebuilds restore a verified tree snapshot by `rename` and rebuild the snapshot in a
detached child, so the restore is instant. Resolution is not the cost — a 275-package chart assembles in
**11.4 ms** and 344 cached packuments load in **15.2 ms** via a binary index (both were
141.6 ms and 426 ms before they were profiled). Payload transfer is network-bound: the same
jest manifest has taken 19 s and 54 s within one hour.

A cold run is bytes over about 10.5 MB/s, plus round trips times dependency depth, plus about 50 ms of local work. jhol sends about 1.1 requests per package. The rest is the registry. A nearer mirror is the lever that moves it: `JHOL_REGISTRY=https://<your-mirror>/`.

<details>
<summary>Tried, measured, and left out</summary>

HTTP/3 (the registry sends no `Alt-Svc`), parallel chunked download (0.94–0.96× on a link that is already full), connection prewarming (0.53×), brotli (larger than gzip on packuments), and HTTP/2 (no measurable difference).

</details>

---

## What is ours

| Piece | Role |
|---|---|
| JAGR-∞ | Resolver. Version bits in base-aware bitmaps, a propagated fixpoint, most-constrained-first search, and per-edge masks |
| Instance resolution | A requirement creates an instance; only incompatible ranges get a second. 249 packages need 270 |
| JAGR-S store v3 | Content-addressed artifacts. SIMD probes, co-clustered disk order, lock-free mmap slices |
| One spec grammar | `file:`, `link:`, `workspace:`, `npm:`, git, and `optional` classified once and honoured everywhere |
| Zero-parse fast paths | Capsule, expansion cache, store index, and a daemon that keeps the hot path warm |
| `jhol-treecheck` | The one freshness authority: what the lockfile authorises vs what is on disk |
| Policy as input | `--as-of`, `--allow-license`, `--exclude-vuln` are masks, so a refused version is never fetched |
| Explainability | `jhol why` (offline attribution) and `why-not` (the minimal unsat core) |
| AI artifacts | Safetensors headers read in 10 KB, wheels filtered by ABI, both under `jhol verify` |
| Proofs | `attest` produces the verifier's own verdict; `replay` re-runs the commands |

Native state lives in `.jhol/state/`. The native lock is `.jhol.lock`. `node_modules` and `package-lock.json` are compatibility renders of that graph.

<details>
<summary>Resolver and store, with the formulas</summary>

**JAGR-infinite**

- Cellular sheaf Laplacian and Fiedler partitioning ($\mathbf{v}_2 \perp \mathbf{1}$). Algebraic connectivity $\lambda_2$ splits a large graph ($V_1 \sqcup V_2$) across Rayon workers.
- Discrete Forman-Ricci curvature, $\mathrm{Ric}(u, v) = 4 - \deg(u) - \deg(v) + 3\Delta(u, v)$. Choke points bias SAT/VSIDS so structural conflicts land at decision level ≤ 3.
- Holographic Wilsonian RG decimation. Non-conflicting leaves fold into boundary operators and cut the search space by 60–80% in $O(1)$.
- Mayer-Vietoris. When $\dim(\mathrm{im}(\delta^*)) = 0$, local solutions glue with no backtrack.
- Tropical matrix exponentiation by squaring, $(I \oplus A)^{\otimes 2^m}$. Depth-16 semver envelopes in the max-plus semiring in about 12 μs.
- Markov-Ricci prefetch, $\Pi(p) = P \cdot \exp(2.5 \kappa)$. Choke-point packages are staged before the stall.

**JAGR-S store v3**

- SwissProbe-16 with SIMD H2 fingerprints. Collision lookup around 1.8 ns.
- Sheaf spectral co-clustering. Sequential placement, page-cache hit rate 98.4% on the recorded run.
- Broadword Elias-Fano selection. About 3.4 ns per monotone index query.
- Lock-free generational slices. `Arc<Mmap>` reads with a header-only flush.

The flat expander and the JAGR law layer are still two paths. Unifying them is the remaining structural change. The formulas and the store write-up live in [`docs/design/JAGRS_STORE_V3_BREAKTHROUGH_RECORD.md`](./docs/design/JAGRS_STORE_V3_BREAKTHROUGH_RECORD.md) and [`docs/engineering/JAGR_INFINITE_ARCHITECTURE.md`](./docs/engineering/JAGR_INFINITE_ARCHITECTURE.md).

</details>

---

## Install

**From crates.io**

```sh
cargo install jhol
```

**From this repository**

```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

**Prebuilt binaries** are `jhol-macos`, `jhol-linux`, and `jhol-windows.exe` on [GitHub Releases](https://github.com/bhuvanprakash/jhol/releases). On macOS and Linux, `chmod +x` the binary. On Windows, run it or put its folder on `PATH`.

```sh
jhol global-install
```

Windows notes: [`docs/user/for-windows.md`](./docs/user/for-windows.md). First project and npm migration: [`GET_STARTED.md`](./GET_STARTED.md).

---

## Commands

Quiet output is `-q`. Machine-readable output is `--json` on install, doctor, audit, and ci.

### Install and lockfile

| Goal | Command |
|---|---|
| Install from `package.json` | `jhol install` |
| Add packages | `jhol install <pkg> [pkgs...]` |
| Skip the cache | `jhol install --no-cache <pkg>` |
| Lockfile only | `jhol install --lockfile-only` |
| Offline | `jhol install --offline` |
| Reproduce the lock | `jhol ci` or `jhol install --frozen` |
| Delegate an edge case | `jhol install --fallback-backend` |
| Scripts in fallback | `--no-scripts` (default) or `--scripts` |

### Health and security

| Goal | Command |
|---|---|
| See what is outdated | `jhol doctor` |
| Apply those updates | `jhol doctor --fix` |
| Audit | `jhol audit` |
| Audit and attempt fixes | `jhol audit --fix` |
| Fail CI on vulnerabilities | `jhol audit --gate` |
| Write an SBOM | `jhol sbom -o sbom.json` |
| Prove the tree matches the lockfile | `jhol verify` (exit 0 only when it does) |
| Delete what the lockfile never authorised | `jhol verify --prune` |
| Deterministic proof, and re-check it | `jhol attest` · `jhol attest --verify` |
| Reproduce a published result | `jhol replay <spec.json> --execute` |

### Explain and choose

| Goal | Command |
|---|---|
| Why is this version here? | `jhol why <pkg>` — copies, ranges, nearest declaring ancestor |
| Why can this range not install? | `jhol why-not <pkg>@<range>` — minimal unsat core, offline |
| Install the tree as it stood on a date | `jhol install --as-of 2023-06-01` |
| Allow only some licences | `jhol install --allow-license MIT,BSD-3-Clause` |
| Never select a vulnerable release | `jhol install --exclude-vuln` |

### Models, wheels, environments

| Goal | Command |
|---|---|
| Describe a checkpoint without downloading it | `jhol ai inspect` (10,760 bytes, any size) |
| Pull weights into the content store | `jhol ai pull` — SHA-256, hardlinked, resumable |
| Which wheel fits this machine? | `jhol wheels torch@2.4.1 --python 3.12` |
| Resolve a Python graph | `jhol pysolve torch@2.4.1` |
| Install wheels, verified per file | `jhol py install requests` |
| Detect the axes every decision uses | `jhol axes` |
| Fork a tree for an experiment | `jhol env fork exp1` — copy-on-write, ~0 bytes |

### Agent tools

| Goal | Command |
|---|---|
| MCP server on stdin | `jhol mcp --stdio` |
| Speculate a resolve with no I/O | tool `jhol_resolve_speculation` |
| Heal a peer conflict | tool `jhol_auto_heal` |
| Typosquat check | tool `jhol_typosquat_check` |
| Audit report | tool `jhol_audit_report` |

### Git and workspaces

| Goal | Command |
|---|---|
| Install checkout and merge hooks | `jhol git-hook install` |
| Merge a lockfile three ways | `jhol merge-lock --base <base> --ours <ours> --theirs <theirs>` |
| Bisect a breaking upgrade | `jhol bisect-deps --test "<test-command>"` |
| Every workspace | `jhol install --all-workspaces` |
| Cache | `jhol cache list` · `size` · `prune` · `export` · `import` · `clean` · `key` |

Full reference: [`docs/user/main.md`](./docs/user/main.md).

---

## Configuration

| Name | Effect |
|---|---|
| `JHOL_CACHE_DIR` | Cache directory |
| `JHOL_REGISTRY` | Registry or mirror origin |
| `JHOL_LOG=quiet` | Less logging |
| `JHOL_OFFLINE=1` | Offline |
| `JHOL_AUTO_DAEMON=0/1` | Daemon-backed install on Unix |
| `JHOL_FETCH_TIMEOUT_SECS` | Per request. Default 120 |
| `JHOL_RESOLVE_TIMEOUT_SECS` | Solver. Default 300 |
| `JHOL_INSTALL_TIMEOUT_SECS` | Whole install. Default 600 |
| `JHOL_MIN_RELEASE_AGE_SECONDS` | Ignore versions younger than this |
| `JHOL_MIN_RELEASE_AGE_EXCLUDES` | Comma-separated names that skip the age filter |
| `JHOL_SCRIPT_ALLOWLIST` | Packages allowed to run scripts |
| `.jholrc` | JSON defaults for `backend`, `cacheDir`, `offline`, `frozen` |

The same timeouts exist as `--fetch-timeout`, `--resolve-timeout`, and `--install-timeout`.

Proxy variables `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` are honoured, including the lowercase forms, and so are `.npmrc` keys `proxy`, `https-proxy`, and `no-proxy`.

In CI, `jhol cache key` is the cache key. `jhol ci` skips resolution and packument fetches, then downloads only the tarballs the lockfile still needs.

---

## Workspace

| Path | Purpose |
|---|---|
| `src/main.rs` | CLI |
| `crates/jhol-core` | Install, lockfile, cache, registry, JAGR |
| `crates/jhol-runtime` | JavaScript runtime, JavaScriptCore on macOS |
| `crates/jhol-bundle` | TypeScript, JSX, and CSS bundler |
| `crates/jhol-vm` | Bytecode engine |
| `crates/jhol-graph` `jhol-policy` `jhol-receipt` `jhol-adapters` `jhol-task` `jhol-env` `jhol-diff` `jhol-ai` `jhol-db` | Foundation crates |
| `crates/jhol-cloud` | Offline control-plane domain model (workspace member, no server) |
| `crates/jhol-remote` | Optional remote store |
| `crates/jhol-trampoline` | Small release helper |
| `docs/user/` | Command reference |
| `scripts/` | Benchmarks and release helpers |

---

## Docs

| | |
|---|---|
| [Get started](./GET_STARTED.md) | Install, first project, moving from npm |
| [Command reference](./docs/user/main.md) | Every command |
| [Windows](./docs/user/for-windows.md) | Platform notes |
| [Lockfile](./docs/engineering/LOCKFILE_DESIGN.md) | Format and the same-tree guarantee |
| [Store v3](./docs/design/JAGRS_STORE_V3_BREAKTHROUGH_RECORD.md) | `.jagrs` layout |
| [Instance model](./docs/plans/e145-instance-model.md) | Nested versions |
| [Innovation log](./docs/internal/INNOVATION_LOG.md) | What was kept, and what was measured and dropped |
| [How We Won](./HOW-WE-WON.md) | 11/11 vs Bun: standings, method, caught bugs |
| [Technical report](./docs/REPORT-2026-11-07.md) | The current session end to end, with every measurement |
| [Resolver](./docs/RESOLVER-JAGR.md) | Bitmaps, bases, masks, instances, index, policy, obligations |
| [Install pipeline](./docs/INSTALL-PIPELINE.md) | argv to tree, lane by lane, with failure modes |
| [CLI reference](./docs/CLI-REFERENCE.md) | Every command, flag, exit code and env var |
| [Engine](./docs/ENGINE-JHOLVM.md) · [gaps](./docs/ENGINE-GAPS.md) | Lexer to JIT; each remaining gap and its acceptance test |
| [AI-first](./docs/AI-FIRST.md) | Models, wheels, axes, forks, attestation, replay |
| [Defect log](./docs/DEFECT-LOG.md) | Thirteen defects: symptom, cause, the rule each produced |
| [Operations](./docs/OPERATIONS.md) · [env audit](./docs/ENV-AUDIT.md) | Gates and landmines; 180 `JHOL_*` variables classified |
| [Changelog](./CHANGELOG.md) | Releases |

---

## Benchmarks

### JholVM engine vs Bun — 11/11

The JS engine inside `jhol run` beats Bun on **all 11** engine benchmarks in
a simultaneous same-seconds A/B: **suite 390 ms vs 552 ms (1.42x ahead)**,
with wins up to 2.7x (`gc_stress`). Correctness is pinned byte-for-byte
against node: **55/55 corpus** (JIT on and off), **127/127 unit tests**, both
quality gates PASS. `Promise` and a microtask queue landed most recently, and
their ordering is compared line for line with node — sync logs, `.then`
chains, a throw inside `then`, `queueMicrotask`, `setTimeout(0)` and a
thenable all interleave identically. The full record — method, per-fix
measurements, the bugs the gate caught, and the honest open items — is
**[HOW-WE-WON.md](./HOW-WE-WON.md)**.

```sh
cargo build --release --features jholvm
crates/jhol-vm/bench/run_bench.sh --jholvm --bun --runs 5
```

### Install / package-manager benchmarks

```sh
cargo build --release
python3 scripts/benchmark.py \
  --suite small \
  --repeats 10 \
  --compare-all \
  --jhol-daemon auto \
  --jhol-cold-cache-strategy reset-each-run \
  --require-binary-bundle \
  --json-out benchmark-comprehensive-small.json \
  --markdown-out benchmark-comprehensive-small.md
```

`--require-binary-bundle` refuses to run when `binary-packages/` is missing. The JSON report carries `jholBenchmarkMode` and `median_excluding_first`.

| Cold cache flag | Meaning |
|---|---|
| `reset-each-run` | Empty the jhol cache before every run |
| `reset-once` | Empty it once, then repeat in that environment |
| `keep` | Keep the global cache, clean `node_modules` only |

```sh
python3 scripts/bench/guardrails.py regression \
  --baseline benchmarks/baseline.json \
  --results benchmark-results.json \
  --threshold 0.25

python3 scripts/bench/guardrails.py collect \
  --benchmark-json benchmark-results.json \
  --fixtures-dir tests/fixtures \
  --metric-source medians \
  --out baseline-report.json
```

`scripts/bench/guardrails.py` is the live checker (`regression`, `collect`, `fallback-trend`, `clean-artifacts`). Older generators sit in `archive/scripts/`.

---

## Before you rely on it

npm lockfiles are verified, including nested versions, integrity, `ci`, `--frozen`, and `--offline`.
Ten real manifests — express, vite, esbuild, jest, next, eslint, webpack, prisma,
typescript, rollup — pass install, verify, idempotent re-install and `ci`+verify.

- Linux and Windows have not been run on this machine.
- Some complex peer graphs can still differ from npm; peers are advisory by default.
- The flat expander and the JAGR law layer are still two code paths.
- **Warm installs are ~2.5× behind Bun** — materialisation, not resolution. Measured, cause known, not yet fixed.
- **Engine gaps, named:** no `BigInt`, no regex lookaround/backrefs, no `eval`. `async`/`await`, the full Promise surface, microtasks, symbols, TDZ, destructuring and per-key enumerability are done and pinned to node (55 corpus pairs).
- Python install has no console-script rewriting and no uninstall; `pysolve` is resolution only.
- Pruning is opt-in (`JHOL_PRUNE_EXTRANEOUS=1`) because a lockfile can be an incomplete record.

- [ ] `jhol audit` is clean
- [ ] CI uses `jhol ci` or `--frozen`
- [ ] Timeouts match the network
- [ ] The cache has a prune schedule
- [ ] `jhol doctor` has seen this dependency graph

---

<p align="center">
  <a href="https://crates.io/crates/jhol">crates.io</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/bhuvanprakash/jhol/releases">Releases</a>
  &nbsp;·&nbsp;
  <a href="./docs/user/main.md">Docs</a>
</p>

<p align="center">
  <a href="./LICENSE">Jhol License</a> — personal and non-commercial use.<br/>
  Commercial use: bhuvanstark6@gmail.com
</p>
