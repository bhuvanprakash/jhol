<p align="center">
  <img src="https://capsule-render.vercel.app/api?type=waving&height=220&color=0:5b3cc4,100:1f1c2c&text=JHOL&reversal=false&section=header&textBg=false&fontColor=ffffff&fontSize=78&animation=fadeIn&fontAlignY=36&desc=Cache-first%20JavaScript%20Package%20Manager&descAlignY=60&descSize=18" alt="JHOL banner" />
</p>

<h1 align="center">JHOL</h1>

<p align="center">
  <strong>Fast, cache-first package manager for JavaScript projects</strong><br/>
  Native workflows • Offline reliability • Deterministic installs
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

### Quick install

```sh
cargo install jhol
```

Works with existing `package.json` and lockfiles.

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

### Prebuilt binaries (Linux and Windows)

Download `jhol-linux` or `jhol-windows.exe` from [GitHub Releases](https://github.com/bhuvanprakash/jhol/releases).

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
| `JHOL_SCRIPT_ALLOWLIST=a,b,c` | Restrict script execution to specific packages |
| `.jholrc` (JSON) | Optional defaults for `backend`, `cacheDir`, `offline`, and `frozen` |

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
python3 scripts/benchmark.py --repeats 3 --json-out benchmark-results.json
```

Optional npm comparison:

```sh
python3 scripts/benchmark.py --repeats 3 --compare-npm --json-out benchmark-results.json
```

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
  --out week1-baseline-report.json

python3 scripts/check_guardrails.py \
  --report week1-baseline-report.json \
  --config benchmarks/week1_guardrails.json
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
  --current-report week1-baseline-report.json \
  --baseline-report week1-baseline-report.json \
  --config benchmarks/fallback_trend_guardrails.json
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

- Dependency resolution currently uses a greedy strategy and may diverge from npm in complex graphs.
- Some advanced peer dependency cases are still being expanded in parity testing.
- Benchmark baselines are CI-automated, but environment-specific tuning is still evolving.

If you hit an issue, open a GitHub issue with the failing dependency graph and lockfile.

---

## Links

- Crate: https://crates.io/crates/jhol
- Releases: https://github.com/bhuvanprakash/jhol/releases
- Documentation entry: [`Documentation/main.md`](./Documentation/main.md)

## License

Jhol is licensed under the [Jhol License](LICENSE) (personal and non-commercial use).
For commercial or other usage, contact: bhuvanstark6@gmail.com.
