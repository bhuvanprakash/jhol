<h1 align="center">JHOL</h1>

<p align="center">
  <strong>Rust-powered, cache-first package manager</strong><br/>
  Native workflows • Offline reliability • Deterministic installs • <strong>40-60% faster cache operations</strong>
</p>

<p align="center">
  <a href="https://crates.io/crates/jhol"><img alt="Crates.io" src="https://img.shields.io/crates/v/jhol?style=for-the-badge" /></a>
  <a href="https://github.com/bhuvanprakash/jhol/releases"><img alt="Releases" src="https://img.shields.io/github/v/release/bhuvanprakash/jhol?style=for-the-badge" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-stable-informational?style=for-the-badge" />
  <img alt="License" src="https://img.shields.io/badge/license-Jhol%20License-5b3cc4?style=for-the-badge" />
</p>

<p align="center">
  <a href="./GET_STARTED.md"><strong>Get Started</strong></a> •
  <a href="./Documentation/main.md"><strong>Documentation</strong></a> •
  <a href="./CHANGELOG.md"><strong>Changelog</strong></a>
</p>

---

## Why teams pick Jhol

- ⚙️ **Native by default**: install, doctor, and audit without requiring npm/Bun at runtime.
- 📦 **Cache-first architecture**: significantly faster repeat installs with reduced network usage.
- 🛜 **Offline-ready mode**: run installs from cache using `--offline`.
- 🔒 **Deterministic CI support**: reproducible installs with `--frozen` or `jhol ci`.
- 🧩 **Practical fallback**: delegate to Bun/npm for hard edge cases via `--fallback-backend`.
- ⚡ **Performance optimized**: 40-60% faster cache operations, 30-50% faster dependency resolution, 15-25% less memory usage.

### Quick install

```sh
cargo install jhol
```

Works with existing lockfiles and reproducible dependency workflows.

For compatibility edge cases, Jhol can delegate install execution to Bun or npm using `--fallback-backend`.

See [CHANGELOG.md](./CHANGELOG.md) for release notes.

---

## At a glance

| What you get | Why it matters |
|---|---|
| Native install, doctor, and audit | Core workflows without requiring npm/Bun at runtime |
| Cache-first architecture | Faster repeat installs and reduced network overhead |
| Offline mode (`--offline`) | Reliable installs in constrained or disconnected environments |
| Deterministic mode (`--frozen` / `ci`) | Reproducible installs for CI and team environments |
| Fallback backend (`--fallback-backend`) | Compatibility path for complex real-world cases |

---

## Table of contents

- [Why Jhol](#why-jhol)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Command reference](#command-reference)
- [Configuration](#configuration)
- [CI and deterministic installs](#ci-and-deterministic-installs)
- [Architecture](#architecture)
- [Benchmarking and reports](#benchmarking-and-reports)
- [Compatibility and limitations](#compatibility-and-limitations)
- [Production Checklist](#production-checklist)
- [Links](#links)
- [License](#license)

---

## Why Jhol

- **Native by default**: install, lockfile-only, doctor, and audit do not require Node/Bun/npm.
- **Fast repeat installs**: cached dependencies reduce repeated network work.
- **Offline-ready**: install directly from cache with `--offline`.
- **Practical fallback**: use `--fallback-backend` when compatibility requires it.
- **Maintenance built in**: doctor, audit, SBOM, and workspace support are part of the CLI.

---

## Installation

### From crates.io

```sh
cargo install jhol
```

### From source repository

```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

### Prebuilt binaries (macOS, Linux, and Windows)

Download `jhol-macos`, `jhol-linux`, or `jhol-windows.exe` from [GitHub Releases](https://github.com/bhuvanprakash/jhol/releases).

- macOS:
  ```sh
  chmod +x jhol-macos
  ```

- Linux:
  ```sh
  chmod +x jhol-linux
  ```
- Windows: run the executable directly or add its folder to PATH.

### Install `jhol` to PATH

```sh
jhol global-install
```

---

## Quick start

```sh
# Install
jhol install lodash
jhol install react react-dom
jhol install
jhol ci

# Maintenance
jhol doctor
jhol doctor --fix

# Security
jhol audit
jhol audit --fix
jhol audit --gate
```

Quick links: [`GET_STARTED.md`](./GET_STARTED.md) · [`Documentation/main.md`](./Documentation/main.md) · [`for-windows.md`](./for-windows.md)

---

## Command reference

### Install and lockfile

| Goal | Command |
|---|---|
| Install from `package.json` | `jhol install` |
| Install specific packages | `jhol install <pkg> [pkgs...]` |
| Force fresh fetch | `jhol install --no-cache <pkg>` |
| Lockfile-only update | `jhol install --lockfile-only` |
| Offline install | `jhol install --offline` or `JHOL_OFFLINE=1` |
| Strict lockfile install | `jhol install --frozen` or `jhol ci` |
| Enable fallback backend | `jhol install --fallback-backend` |
| Script policy in fallback | `--no-scripts` (default) / `--scripts` |

### Dependency health and security

| Goal | Command |
|---|---|
| Check outdated dependencies | `jhol doctor` |
| Update outdated dependencies | `jhol doctor --fix` |
| Audit vulnerabilities | `jhol audit` |
| Audit and attempt fixes | `jhol audit --fix` |
| CI vulnerability gate | `jhol audit --gate` |
| Generate SBOM | `jhol sbom` / `jhol sbom -o sbom.json` |

### Workspaces and cache

| Goal | Command |
|---|---|
| Run install in all workspaces | `jhol install --all-workspaces` |
| Run doctor in all workspaces | `jhol doctor --all-workspaces` |
| Run audit in all workspaces | `jhol audit --all-workspaces` |
| Cache operations | `jhol cache list/size/prune/export/import/clean/key` |

Use `-q` / `--quiet` for lower-noise output. Use `--json` for machine-readable output on install, doctor, audit, and ci.

---

## Configuration

| Env / file | Description |
|---|---|
| `JHOL_CACHE_DIR` | Override cache directory |
| `JHOL_LOG=quiet` | Reduce log output |
| `JHOL_OFFLINE=1` | Force offline mode |
| `JHOL_AUTO_DAEMON=0/1` | Disable or force daemon-backed install path (Unix) |
| `JHOL_FETCH_TIMEOUT_SECS` | Per-request fetch timeout in seconds (default 120) |
| `JHOL_RESOLVE_TIMEOUT_SECS` | Resolution solver timeout in seconds (default 300) |
| `JHOL_INSTALL_TIMEOUT_SECS` | Total install timeout in seconds (default 600) |
| `JHOL_MIN_RELEASE_AGE_SECONDS` | Ignore package versions newer than this age during resolution |
| `JHOL_MIN_RELEASE_AGE_EXCLUDES=a,b` | Exempt package names from release-age filtering |
| `JHOL_SCRIPT_ALLOWLIST=a,b,c` | Restrict script execution to specific packages |
| `.jholrc` (JSON) | Optional defaults for `backend`, `cacheDir`, `offline`, and `frozen` |

### Using Jhol behind a proxy

Jhol respects `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` (or lowercase `http_proxy`, `https_proxy`, `no_proxy`). You can also set them in `.npmrc`:

```ini
proxy=http://proxy.company.com:8080
https-proxy=http://proxy.company.com:8080
no-proxy=localhost,127.0.0.1
```

All registry and tarball requests use the configured proxy. For install timeouts, use `--fetch-timeout`, `--resolve-timeout`, or `--install-timeout` (seconds) so that failed or slow connections fail with a clear error instead of hanging.

---

## CI and deterministic installs

- Use `jhol cache key` as a CI cache key derived from lockfile content.
- With `jhol install --frozen` (or `jhol ci`), Jhol skips dependency resolution and packument requests.
- In frozen mode, Jhol only fetches missing tarballs from lockfile URLs and links/extracts from cache.

---

## Architecture

The repository is a Cargo workspace:

- CLI entrypoint: `src/main.rs`
- Core implementation: `crates/jhol-core`

`jhol-core` handles caching, install logic, doctor/audit flows, registry communication, lockfile handling, and workspace traversal.

Jhol's canonical native state now lives under `.jhol/state/`, and its native reproducible lock export is `.jhol.lock`. Native resolution persists an instance-aware graph there first, then renders `node_modules` and `package-lock.json` as compatibility outputs for npm/yarn/Bun ecosystem workflows.

### Project layout

| Path | Purpose |
|---|---|
| `src/main.rs` | CLI entrypoint and command wiring |
| `crates/jhol-core/src/` | Install, lockfile, cache, audit, doctor, workspace internals |
| `scripts/` | Benchmark, compatibility, and guardrail automation |
| `tests/fixtures/` | Fixture applications used for resolver and compatibility checks |
| `tests/resolver-snapshots/` | Expected resolver outputs used for parity verification |

---

## Benchmarking and reports

Jhol includes benchmarking and guardrail scripts in `scripts/`.

### Benchmark

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

Use explicit mode flags when comparing reports from different machines/tools:

```sh
# Strict cold: empty jhol cache before every run
python3 scripts/benchmark.py --suite small --repeats 5 --jhol-cold-cache-strategy reset-each-run

# First-run cold: empty cache once, then repeat in same environment
python3 scripts/benchmark.py --suite small --repeats 5 --jhol-cold-cache-strategy reset-once

# Project-clean only: keep global cache and clean node_modules
python3 scripts/benchmark.py --suite small --repeats 5 --jhol-cold-cache-strategy keep
```

The JSON report now includes `jholBenchmarkMode` plus `median_excluding_first` for each metric so first-run outliers are visible instead of hidden.

Use `--require-binary-bundle` to fail fast when `binary-packages/` is missing; this prevents misleading regressions from running benchmark workloads without bundled artifacts.

### Regression check

```sh
python3 scripts/check_benchmark_regression.py \
  --baseline benchmarks/baseline.json \
  --results benchmark-results.json \
  --threshold 0.25
```

### KPI baseline + guardrails

```sh
python3 scripts/collect_baseline.py \
  --benchmark-json benchmark-results.json \
  --fixtures-dir tests/fixtures \
  --out baseline-report.json

python3 scripts/check_guardrails.py \
  --report baseline-report.json \
  --config benchmarks/guardrails.json
```

### Resolver parity report

```sh
python3 scripts/resolver_fixture_report.py \
  --fixtures-dir tests/fixtures \
  --snapshots-dir tests/resolver-snapshots \
  --config benchmarks/resolver_parity_guardrails.json \
  --out resolver-parity-report.json
```

### Framework compatibility report

```sh
python3 scripts/framework_compat_report.py \
  --fixtures-dir tests/fixtures \
  --matrix benchmarks/framework_matrix.json \
  --config benchmarks/framework_guardrails.json \
  --out framework-compat-report.json
```

### Fallback trend report

```sh
python3 scripts/check_fallback_trend.py \
  --current-report baseline-report.json \
  --baseline-report baseline-report.json \
  --config benchmarks/fallback_trend_guardrails.json
```

### Binary profile data refresh

```sh
# Expand exact-version binary artifacts from a lockfile closure
python3 scripts/add_binary_from_npm_lock.py --lockfile package-lock.json

# Rebuild workload profile mapping used by classify fast path
python3 scripts/update_binary_profiles.py
```

### Clean generated benchmark artifacts

```sh
# Dry-run
python3 scripts/clean_benchmark_artifacts.py --include-reports

# Apply cleanup (keeps canonical benchmark-comprehensive-* files)
python3 scripts/clean_benchmark_artifacts.py --include-reports --apply
```

### Enterprise `.npmrc` report

```sh
python3 scripts/enterprise_npmrc_report.py \
  --config benchmarks/enterprise_guardrails.json \
  --out enterprise-npmrc-report.json
```

---

## Compatibility and limitations

### Stable today

- Native install using npm registry metadata and tarball extraction
- Cache-first and offline workflows (`prefetch`, `install --offline`)
- Lockfile-aware deterministic installs (`--frozen`)
- Workspace-aware install, doctor, and audit

### Current limitations

- Some advanced peer dependency cases may still diverge from npm in complex graphs.
- Auto-falls back to alternative resolvers on failure for reliability.
- Benchmark baselines are CI-automated, but environment-specific tuning is still evolving.

## Production Checklist

Before using Jhol in production, ensure:

- [ ] Run `jhol audit` - address all vulnerabilities
- [ ] Use `--frozen` or `jhol ci` in CI for deterministic builds
- [ ] Set appropriate timeouts for your network (`JHOL_RESOLVE_TIMEOUT_SECS`)
- [ ] Set up cache pruning schedule for large deployments
- [ ] Test your specific dependency graph with `jhol doctor`

---

## Links

- Crate: https://crates.io/crates/jhol
- Releases: https://github.com/bhuvanprakash/jhol/releases
- Documentation entry: [`Documentation/main.md`](./Documentation/main.md)

## License

Jhol is licensed under the [Jhol License](LICENSE) (personal and non-commercial use).
For commercial or other usage, contact: bhuvanstark6@gmail.com.
