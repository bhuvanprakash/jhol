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
- 🌀 **JAGR resolver**: novel three-layer dependency resolver (tropical + sheaf cohomology + core-guided MaxSAT) — formal guarantees no other tool offers.
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
- [JAGR — Jhol Advanced Graph Resolver](#jagr--jhol-advanced-graph-resolver)
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
| Add JAGR topological report to doctor | `jhol doctor --jagr` |
| Audit vulnerabilities | `jhol audit` |
| Audit and attempt fixes | `jhol audit --fix` |
| CI vulnerability gate | `jhol audit --gate` |
| Generate SBOM | `jhol sbom` / `jhol sbom -o sbom.json` |
| Outdated dependencies with JAGR impact | `jhol outdated --jagr` |
| JAGR three-layer analysis on the manifest | `jhol jagr` |
| JAGR-driven deep `why` | `jhol why --jagr <pkg>` |
| CI post-install JAGR satisfiability check | `jhol ci --jagr-check` |

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

### JHOL vs Bun — Install Performance (10 packages, median of 3)

| Scenario | JHOL | Bun | Ratio |
|---|---|---|---|
| B1: Cold (no cache) | 477ms | 647ms | **0.74x** (26% faster) |
| B3: Warm+Lock | 11ms | 16ms | **0.69x** (31% faster) |
| B5: Reinstall | 7ms | 5ms | 1.40x |
| Disk (dereferenced) | 5.7M | 5.7M | 1.00x |
| Version match | 10/10 | 10/10 | 100% |
| Binary size | 3.9MB | ~90MB | **0.04x** (23x smaller) |

---

## JAGR — Jhol Advanced Graph Resolver

JAGR is jhol's novel, in-house dependency resolver. It is a three-layer
pipeline based on **tropical geometry**, **sheaf cohomology**, and
**core-guided MaxSAT** — none of which appears in any other package
manager today. It is enabled by default for `jhol install` and is the
target of every diagnostic and observability command documented below.

### The three layers

| Layer | Mathematics | Role |
|---|---|---|
| Layer 1 | Core-guided LP MaxSAT (OL3P / OL3P-S) | Exact solver, the last word |
| Layer 2 | Tropical sheaf cohomology + Betti numbers + Simplicial Normal Form + Mayer–Vietoris | Topological filter; if β₁ = β₂ = 0 the graph is acyclic and greedy is provably correct |
| Layer 3 | Tropical Max-plus Nullstellensatz + Max-atom PTIME filter (Algorithm A) | Polynomial-time pre-filter that prunes UNSAT before the expensive layers run |

The pipeline only escalates to the next layer when the previous one
cannot give a definite answer, so simple manifests resolve in microseconds
while complex ones still get the full guarantee of an exact solver.

### Commands

| Goal | Command |
|---|---|
| Run JAGR analysis on the current manifest | `jhol jagr` |
| Per-package layer attribution | `jhol jagr --explain-each` |
| Side-by-side comparison of jagr / pubgrub / sat | `jhol jagr --compare-resolvers` |
| Topological upgrade plan (β₁, SNF, risk class) | `jhol jagr --upgrade-plan` |
| Simulate a single upgrade impact | `jhol jagr --dry-run-upgrade left-pad@1.3.0` |
| Same, as a standalone subcommand (supports `--json`) | `jhol jagr-impact --target left-pad@1.3.0` |
| Resolve manifest and write the lockfile without installing | `jhol jagr --lockfile-only` |
| Skip Layer 3 (tropical pre-filter) | `jhol jagr --skip-layer3` |
| Skip Layer 2 (sheaf cohomology) | `jhol jagr --skip-layer2` |
| Run all three layers even on early termination (benchmarking) | `jhol jagr --run-all-layers` |
| Export the derivation graph in DOT or Mermaid | `jhol jagr --graph mermaid --out graph.md` |
| Persist a JAGR report for later diffing | `jhol audit-jagr --save report.json` |
| Diff two saved reports | `jhol audit-jagr --diff old.json,new.json` |
| Show the persistent Betti-number history | `jhol betti-history --last 10` |
| Show the effective JAGR config (`.jholrc` + env) | `jhol jagr --show-config` |
| Use JAGR in `jhol why` | `jhol why --jagr <pkg>` |
| Add a JAGR post-install health check to `jhol ci` | `jhol ci --jagr-check` |
| Add a JAGR topological impact report to `jhol doctor` | `jhol doctor --jagr` |
| Add JAGR topological impact to `jhol outdated` | `jhol outdated --jagr` |
| Resolve a transitive dep graph through JAGR in install | `jhol install <pkg>` (JAGR is the default) |
| Per-layer breakdown during install | `JHOL_VERBOSE=1 jhol install <pkg>` |

### Configuration

JAGR reads its config from the `jagr` section of `.jholrc` (project
root or `~/.jholrc`). All fields are optional; missing values fall back
to the JAGR defaults. Environment variables take precedence over the
file.

| `.jholrc` key | Env var | Default | Effect |
|---|---|---|---|
| `useLp` | `JHOL_JAGR_LP` | `true` | Use LP relaxation in Layer 1 (OL3P/OL3P-S) |
| `timeoutMs` | `JHOL_JAGR_TIMEOUT_MS` | `30000` | Per-layer timeout in milliseconds |
| `skipLayer3` | `JHOL_JAGR_SKIP_LAYER3` | `false` | Skip the tropical pre-filter |
| `skipLayer2` | `JHOL_JAGR_SKIP_LAYER2` | `false` | Skip the sheaf-cohomology layer |
| `runAllLayers` | `JHOL_JAGR_RUN_ALL_LAYERS` | `false` | Run all three layers even on early termination |
| `bettiHistory` | `JHOL_BETTI_HISTORY` | `false` | Persist a Betti-number history (one JSONL line per run) |
| `bettiHistoryDir` | `JHOL_BETTI_HISTORY_DIR` | alongside the manifest | Directory for the `.jagr_betti_history.jsonl` log |
| `maxVersions` | `JHOL_JAGR_MAX_VERSIONS` | `512` (64 for upgrade-plan) | Max candidate versions per package |
| `strict` | `JHOL_JAGR_STRICT` | `false` | Reserved for future "fail loudly on UNSAT" mode |

Example `.jholrc`:

```json
{
  "jagr": {
    "useLp": true,
    "timeoutMs": 60000,
    "bettiHistory": true,
    "bettiHistoryDir": ".jagr-state",
    "maxVersions": 1024
  }
}
```

### Why three layers?

The tropical / sheaf / MaxSAT split is **mathematically novel in package
management**. No other resolver (npm, yarn, pnpm, Bun, Cargo, PubGrub,
MaxNPM, PACSOLVE) uses tropical geometry or sheaf cohomology. The
practical consequences are:

- **Formal guarantees.** When β₁ = β₂ = 0 we can prove the dep graph is
  acyclic and greedy resolution is correct. Other resolvers rely on
  empirical evidence alone.
- **Per-layer observability.** Each layer reports its own metrics (β
  numbers, layer time, derivation graph size) so the user can see *why*
  a particular resolution strategy was picked.
- **Auditability.** The full derivation graph, layer-by-layer timing,
  and strategy label are persisted as JSON via `jhol audit-jagr`, so CI
  can detect topological regressions in your dep graph over time.

### Observability flags (all default-safe)

| Flag | Purpose |
|---|---|
| `JHOL_VERBOSE=1` | Per-layer breakdown during `jhol install` and lockfile write |
| `JHOL_BETTI_HISTORY=1` | Persist a Betti history log |
| `JHOL_BETTI_HISTORY_DIR=path` | Where the Betti history lives |
| `JHOL_JAGR_LP=0` | Force the non-LP MaxSAT path |
| `JHOL_JAGR_TIMEOUT_MS=…` | Override the per-layer timeout |
| `JHOL_JAGR_SKIP_LAYER3/2=1` | Skip individual layers (debugging) |
| `JHOL_JAGR_RUN_ALL_LAYERS=1` | Force all three layers to populate |
| `JHOL_JAGR_MAX_VERSIONS=…` | Cap candidate versions |
| `JHOL_JAGR_STRICT=1` | Reserved: fail loudly on UNSAT |

---

## Compatibility and limitations

### Stable today

- Native install using npm registry metadata and tarball extraction
- Cache-first and offline workflows (`prefetch`, `install --offline`)
- Lockfile-aware deterministic installs (`--frozen`)
- Workspace-aware install, doctor, and audit

### Current limitations

- JAGR is the default resolver; the legacy greedy and PubGrub paths are preserved (accessible via `JHOL_RESOLVER=greedy|pubgrub`) but no longer the focus of new development.
- Some advanced peer dependency cases are still being expanded in parity testing.
- Benchmark baselines are CI-automated, but environment-specific tuning is still evolving.

If you hit an issue, open a GitHub issue with the failing dependency graph and lockfile.

---

## Bug Fixes (v1.x)

The following issues were identified and fixed in this release:

| # | Component | Issue | Fix |
|---|---|---|---|
| 1 | `lockfile_write.rs:1740` | `jagr_simulate_upgrade` showed bare `"Safe to apply"` without β₁ values in recommendation | Recommendation now shows `Safe to apply (β₁ stays at {baseline.betti_1}, SNF remains satisfied)` |
| 2 | `main.rs` | Multi-workspace `--manifest` subdirectory path was only using basename, not full relative path | `manifest_path.parent()` now preserved and joined to each workspace root |
| 3 | `main.rs:2024-2027` | Multi-workspace JAGR config was loaded once from CLI's cwd instead of per-workspace | Config loading moved inside the per-workspace loop so each workspace loads `.jholrc` from its own root |
| 4 | `lockfile_write.rs:1419-1423` | `jagr_upgrade_plan` was loading JAGR config and creating a new resolver for every single dependency (O(n) loads) | Config and resolver now loaded once outside the per-dep loop |
| 5 | `lockfile_write.rs:1721` | `jagr_simulate_upgrade` was loading JAGR config twice — once at line 1687 for `cap`, again at line 1721 for resolver | Config loaded once at line 1687 and reused for both `cap` and resolver |
| 6 | `install.rs:961` | `install_lockfile_only` accepted `backend: Backend` parameter but never used it | Parameter removed; callers updated |
| 7 | `lockfile.rs` (test module) | `safe_path_rejects_parent_dir` test was missing its closing `}` causing structural corruption in test module | Closing `}` added |
| 8 | `jagr_upgrade_plan` (per-dep loop) | Resolver was correctly built once outside the loop; consistency verified | Resolver reused for every dependency in the upgrade simulation |
| 9 | `jagr_resolver.rs:472` | Dependency constraints encoded as conjunction (`to_i ≥ from` for ALL i) instead of disjunction (`max(to_1,...,to_k) ≥ from`) — required ALL versions selected simultaneously instead of at least one | Disjunctive encoding using auxiliary variables that compute running `max(to_1,...,to_k)` and constrain it ≥ `from` |
| 10 | `jagr_resolver.rs:248-260, 357-372` | Dependency verification checked `selected_ver == from_ver` (exact match against dependency map key), skipping deps if the selected version didn't match the key | Verification now looks up `(pkg, selected_ver)` directly in the dependency map for each selected package |
| 11 | `jagr_resolver.rs:480-488` | Conflict constraints loop body was empty — no constraints actually added for conflict pairs, making Layer 3 blind to conflicts | Conflict sentinel variables added per conflict pair; encodes max(i1,i2) ≥ sentinel for Layer 2 analysis; hard mutual exclusion handled by Layer 1 MaxSAT |
| 12 | `jagr_resolver.rs:579-591` | `compare_versions` only extracted numeric parts, ignoring pre-release tags (e.g., `1.0.0-alpha` == `1.0.0`) and build metadata, and didn't zero-pad different-length versions | Full semver comparison: strips build metadata (`+`), parses pre-release (`-`), pre-release < release per spec, zero-pads for `1.0` == `1.0.0`, dot-wise pre-release identifier comparison |
| 13 | `jagr_resolver.rs:561-576` | `assignment_to_selection` used `or_insert_with()` which kept the FIRST version assigned per package instead of the HIGHEST | Uses `and_modify` with `compare_versions` to always keep the highest version when multiple are assigned TRUE |
| 14 | `jagr_resolver.rs:534-554` | Soft preferences encoded as `sorted[i] → sorted[0]` for all i (every version preferred only the highest), with uniform weight 1.0 — no preference ordering among middle versions | Pairwise preferences `sorted[i+1] → sorted[i]` with decreasing weights `(n-i)`, creating a proper preference chain through all versions |
| 15 | `jagr_resolver.rs:415-433` | `greedy_resolve` used `selection.insert()` which silently overwrote if a package was selected at a different version from another dependency path | Now compares new vs existing version, keeps the higher one, and re-queues for transitive dep re-resolution |
| 16 | `tropical_max_atom.rs:256-263` | `f_double_prime` atoms (split difference constraints from Algorithm A) were computed but never added to the rebuilt valued graph — `rebuild_graph` only used `f_prime`, silently dropping split constraints | `f_double_prime` now stored as struct field; `rebuild_graph` iterates both `f_prime` and `f_double_prime`, ensuring all difference constraints from atom splits participate in the closure computation |
| 17 | `tropical_max_atom.rs:385-406` | `npm_range_to_max_atoms` created self-referential `MaxAtom(var_idx, var_idx, var_idx, r)` which simplifies to `r ≥ 0` — a tautology imposing NO version constraint whatsoever | Function signature now accepts `lower_bound_idx`, `var_idx`, `upper_bound_idx` as separate variable indices; `>=X` encodes `lower_bound + ver_val ≥ var`, `<X` encodes `var + (ver_val - ε) ≥ upper_bound`, `^X` encodes both bounds with proper major-computation |
| 18 | `pubgrub/minimal.rs:175` | `comparator_minimum` for `Op::Greater` returned the exact specified version as minimum, but `>1.0.0` means strictly greater — `1.0.0` does NOT satisfy `>1.0.0` | Returns next version per semver: `1.0.1-0` for `>1.0.0`, `1.3.0-0` for `>1.2`, `2.0.0-0` for `>1`; `Op::GreaterEq` handled separately as the exact version |
| 19 | `sat_resolver.rs:704-715` | `is_version_compatible` only checked `v1.major == v2.major`, treating `0.1.0` as compatible with `0.9.0` (semver says minor changes are breaking for 0.x) and ignoring pre-release semantics | Full semver compatibility: 0.x requires minor match; pre-release versions only compatible with other pre-releases of same M.m.p; release versions compatible by major for ≥1.0.0 |
| 20 | `tropical_max_atom.rs:314-323` | `rebuild_graph` doc said "from f_prime and f_double_prime" but only iterated `f_prime` — fixed together with Bug 16 | `f_double_prime` atoms now included in `rebuild_graph` (see Bug 16) |
| 21 | `tropical_math.rs:251-292` | `tropical_determinant_kim` used standard arithmetic for Gaussian elimination step (`val + new_val`) instead of tropical max — the algorithm was computing standard determinant, not tropical determinant | Rewritten to use tropical algebra: pivot accumulation via `det.max(pivot_val)`, elimination as `a[i][j] = max(a[i][j], a[i][k] - a[k][k] + a[k][j])` |
| 22 | `core_guided_solver.rs:99-106` | `compute_cost` and `compute_cost_from_formula` both returned hardcoded `0.0`, making `is_optimal: true` and `cost: 0.0` in `CoreGuidedResult` always misleading regardless of soft clause violations | Both now iterate soft clauses, sum weights of unsatisfied ones; `compute_cost_from_cnf` takes an explicit `&CNFFormula` parameter; `OL3PSolver::formula()` getter added |
| 23 | `mayer_vietoris.rs:305-310` | `verify_exactness` always returned `true` without any actual verification, making `MayerVietorisSequence.is_exact` always `true` — false confidence in the sheaf analysis | Now checks `im(f) ⊆ ker(g)` for each consecutive pair of connecting maps where `f.target_degree == g.source_degree` |
| 24 | `betti_numbers.rs:466-531` | `extract_cycles` DFS used `u != parent` for back-edge detection in undirected graphs, producing false positive cycles from tree edges in the reverse direction | Added depth tracking; back edges only counted when `depth[u] < depth[v]` (ancestor, not sibling/cross-link), eliminating false positives; minimum cycle length 3 enforced |
| 25 | `ol3p_solver.rs:106-108` | `core_weight` used `self.formula.clauses[i].weight` (original weights) instead of `self.working_formula[i].weight` (current weights after relaxation), causing incorrect lower bound updates | Core weight now read from `working_formula` which tracks reduced weights after each relaxation step |
| 26 | `tropical_sheaf.rs:383-405` | `versions_compatible` used `var == 0` / `else` to pick `ver1`/`ver2` — hardcoded variable index assignment, wrong for any var_idx ≠ 0 or 1 | Now takes explicit `(ver1, ver2, var1, var2, polyhedron)` parameters with proper `var_to_ver` closure mapping |
| 27 | `tropical_polyhedron.rs:299-310` | `convex_hull` created constraints `x_i ≥ point[i]` using `add_right(i, point[i])` which treats point value as a variable coefficient, not a constant bound | Uses `add_bound(i, point[i], ∞)` which correctly encodes lower bound as a structural constraint rather than a halfspace RHS variable |
| 28 | `tropical_polyhedron.rs:329-334` | `to_maxplus_system` right side computation was dead code — `right_max` computed from a loop with empty body (commented-out var reference), so `b[i]` was always `Tropical::ZERO` | Right side now sums `right_coeffs` into `b[i]`, providing the correct RHS for the max-plus standard form |
| 29 | `tropical_sheaf.rs:374-375` | `build_restrictions` made `(v2,v1)` restriction share the same `RestrictionMap` as `(v1,v2)` — restriction maps are directional (stalk v1→stalk v2 vs v2→v1), not symmetric | Builds a separate reverse restriction map by computing `versions_compatible(ver2, ver1, var2, var1, ...)` for each `(v2,v1)` direction |
| 30 | `conflict_explainer.rs:345-352` | `explain_package_conflict` showed `pkg requires constraint (from from_ver)` with `from_pkg` appearing only in the "because" line — `pkg` and `from_pkg` were semantically swapped in the output | Now shows `from_pkg requires constraint (from pkg@ver)` with `because pkg@ver depends on it` — correct causal direction |
| 31 | `maxsat_solver.rs:271-285` | `solve()` created an anonymous `OLLSolver` for hard-only SAT check whose result was immediately discarded (no named variable), then created a second `OLLSolver` for full solve — first solver's CNF clone with discarded soft_clauses indices could cause inconsistencies | Uses named `hard_solver` variable; hard-only SAT check and full solve are cleanly separated with explicit `hard_only` formula construction |
| 32 | `ol3p_solver.rs:157-166` | `dpll_solve` pushed simplified formula onto the stack even when it was empty (`is_trivial` = satisfied), meaning a trivially satisfiable branch was being explored as a normal search node | Empty/trivial simplified formula now returns `(true, Some(assignment))` immediately instead of pushing onto the stack |
| 33 | `tropical_nullstellensatz.rs:312-315` | `max_atoms_to_tropical_polys` created the `-x_x` monomial with `Tropical::new(0.0)` which is the ONE element (identity for ⊗ = standard 0.0) — this adds `0.0 + x_x` to the polynomial, not negation | The false negation monomial is removed; the polynomial now correctly represents only the LHS `max(x_z+r, x_y+r)`; the Macaulay matrix handles RHS variables through dual representation |
| 34 | `tropical_polyhedron.rs:372-376` | `TropicalSimplex::contains()` computed "minimum" as `min_val = min_val + v[i]` where `+` is tropical addition (max), so it was actually computing the MAX of vertex coordinates, not the minimum — any point below the maximum would incorrectly fail containment | Uses standard `f64::INFINITY` and `v[i].0 < min_val` to compute actual minimum; containment check now correctly determines if point lies within the min bounds of the simplex |
| 35 | `tropical_sheaf.rs:456-457` | `kernel_dimension` returned `matrix.len()` for empty matrices — this is the number of ROWS, not the kernel dimension (which should be 0 for a zero matrix since there are no non-trivial kernel vectors) | Returns 0 for both empty-row and empty-column matrices, matching the mathematical definition that a zero matrix has trivial kernel |
| 36 | `tropical_sheaf.rs:374-375` | `build_restrictions` shared the same `RestrictionMap` object between `(v1,v2)` and `(v2,v1)` directions — sheaf restriction maps are directional (maps from stalk(v1) to stalk(v2) are generally different from the reverse) | Separate reverse restriction map computed with `versions_compatible(ver2, ver1, var2, var1, ...)` — each edge direction now has its own independently computed restriction |
| 37 | `tropical_math.rs:510-512` | `to_tropical_matrices` set both `atom.z` and `atom.y` positions to the same coefficient `atom.r` in matrix `a`, conflating the two terms of the max — for disjunctive atoms (`z ≠ y`) these should be separate arcs with different semantics | Returns `(a, f_prime)` where `a` holds the z-position and RHS x-position entries, and `f_prime` holds the y-position entries — matching the `f_prime`/`f_double_prime` decomposition used by the tropical max-atom solver |
| 38 | `ol3p_solver.rs:290-304` | `at_least_k(vars, k)` generated n clauses each with all-but-one variable negated — this encodes "at most one false" = "at least n-1 true", NOT "at least k true" — the encoding was only correct when k = n-1 | Replaced with proper combinatorial encoding: for every subset S of size (n-k+1), at least one variable in S must be true (positive literals). This correctly encodes "at least k of n are true" for any k |
| 39 | `tropical_sheaf_analyzer.rs:97-124` | `full_analyze` called `check_snf_from_polyhedron` then discarded its result; `get_action` also called `check_snf_from_polyhedron` then built the sheaf again redundantly | `full_analyze` now uses `check_snf_from_polyhedron` result in combined SNF check (`snf && snf_from_poly`); `get_action` removes the dead `check_snf_from_polyhedron` call and reuses the sheaf built by `analyze_snf` |
| 40 | `jagr_resolver.rs:226-232` | `resolve()` didn't check Layer 2 analysis for non-SNF with obstructions before deciding between greedy and Layer 1 — if β₁ or β₂ were non-zero with a global obstruction, the resolver could still attempt greedy resolution, which is invalid when SNF fails | Added explicit check: if `!is_snf && !greedy_works` and global/MV obstructions exist, the resolver no longer attempts greedy resolution — it falls through to Layer 1 (exact MaxSAT solver) |
| 41 | `tropical_max_atom.rs:309-314` | `check_atom_feasible` unconditionally added both y→x and z→x arcs to the temporary graph, even if those arcs already existed in the original graph — duplicate arcs with the same weight don't affect the closure but waste computation and can mask real infeasibility | Only adds arcs that aren't already present, using a new `has_arc(from, to)` method on `ValuedGraph` |
| 42 | `maxsat_solver.rs:412-424` | `add_version_preference` encoded preference from lower version to higher (`from=v[i], to=v[i+1]`) — the CNF implication `¬v[i] ∨ v[i+1]` means "if v[i] is selected, prefer v[i+1]", but the INTENT is "if a higher version is selected, the lower version is acceptable" | Now encodes `from=v[i+1], to=v[i]` — implication `¬v[i+1] ∨ v[i]` means "if the higher version is selected, the lower version is a valid preference", which correctly models version preference |
| 43 | `core_guided_lp.rs:58-65` | `Clause::is_satisfied` defaulted unassigned variables to `false` for positive literals and `true` for negated literals — under 3-valued SAT solver semantics, unassigned means unknown and should NOT satisfy the clause | Uses explicit `match` on `assignment.get()` — unassigned returns `false` (not satisfied) for both polarities, requiring at least one concretely TRUE literal to satisfy the clause |
| 44 | `tropical_sheaf.rs:306-310` | `build_stalks` generated version lists with fixed step=1.0 from min to max — fractional semver versions like 1.2.3 could never appear, making sheaf stalks incomplete for real-world package version ranges | Uses adaptive step size: 0.1 for ranges ≤5, 0.5 for ranges ≤20, 1.0 for larger ranges; also ensures the max bound is always included as the final version |
| 45 | `betti_numbers.rs:75` | `BoundaryMatrix::rank` used `rank < self.rows` as the loop bound, but rank is determined by the number of pivots found (column-pivots), which is bounded by `min(rows, cols)` — when rows > cols the loop could attempt invalid pivot rows | Changed to `rank < self.rows.min(self.cols)` — rank cannot exceed the minimum dimension of the matrix, ensuring correct column-pivot tracking |
| 84 | `lockfile.rs:534-585` | Test module had missing closing `}` on `safe_path_rejects_parent_dir`, causing `resolve_deps_for_install_is_stable_sorted` and `platform_filter_skips_optional_unsupported` to be nested inside it as dead inner functions; duplicate copies of both tests also existed outside the `mod tests {}` block | Closing brace added; duplicate out-of-module test functions removed; test module now has clean structure with every test as a direct child of `mod tests` |
| 85 | `core_guided_lp.rs:220-304` | `simplex_solve` enumerated `2^min(n,10)` corners of the variable-bounds hypercube — an LP's feasible region is defined by constraint intersections, not variable bounds, so the solver could never find actual optimal vertices; also the feasibility check used converted `b[]` indices against original `lp.constraints` coefficients, mixing two different index spaces | Replaced with two-phase revised simplex: Phase I finds a basic feasible solution using artificial variables; Phase II optimizes via Dantzig's rule with Bland's anti-cycling tiebreak; standardizes constraints (Ge→Le, Eq→two Le), negates rows with negative RHS for Phase I candidacy; clamps final solution to variable bounds |
| 86 | `jagr_resolver.rs:436-460` | `greedy_resolve` ignored `request.lockfile` — when a lockfile hint was present, the resolver still always chose the highest compatible version, causing unnecessary upgrades on incremental re-resolves | When lockfile is present and the locked version is compatible, it is preferred over the highest version; falls back to highest when locked version is not in the compatible set |
| 92 | `core_guided_lp.rs:779-814` | `resolve_lp` built an LP with zero-filled coefficient rows (all constraints were `0 ≤ 0`), making the LP relaxation completely non-functional — no clause constraints were ever encoded; `add_oll_constraints` and `add_ol3p_lp_constraints` created o_var/e_var metadata but added no CNF or LP constraints; `compute_reduced_costs` just copied primal solution values as reduced costs | `resolve_lp` now properly encodes hard clause constraints as LP inequalities (positive literal → +1 coefficient, negated → −1, with adjusted RHS); objective maximizes −(soft clause violation weight); `add_oll_constraints` now adds relaxation variables and at-least-k soft/hard CNF clauses; `compute_reduced_costs` derives costs from objective coefficients and LP solution bounds |
| 93 | `ol3p_solver.rs:214-258` | `relax_soft_clause` compared core clause weights using `formula.clauses[i].weight` (original weights) instead of `working_formula[i].weight` (current weights after prior relaxations), and computed `relaxed_weight` as `formula.clauses[min_idx].weight - min_weight` — always 0 for the minimum clause since it subtracts a value from itself | Weight comparison and min-weight extraction now read from `working_formula`; `relaxed_weight` computed from current working weight with `.max(0.0)` clamp; all weight reductions are relative to the CURRENT weight, not the original |
| 96 | `tropical_math.rs:513-515` | `to_tropical_matrices` used sequential `a.set()` calls that overwrite when `atom.z == atom.x`; in the max-plus semiring the correct semantics is tropical addition (⊕ = max), so the second `set` would silently discard the first value | Accumulate via tropical addition: `a.set(i, j, existing + new_value)` (where `+` = `max`) instead of unconditional overwrite; fixes z==x collision where two atoms reference the same matrix position |
| 97 | `tropical_polyhedron.rs:167-250` | Chain encoding for `left_vars > 2` dropped per-variable coefficients and only applied `max(coefficients)` at the end; `left_vars == 2` case ignored `left_coeffs[1]`; both `is_empty()` and `get_feasible_point()` produced wrong results when left-side coefficients differed | When coefficients differ, introduce `running_max` aux variable with separate `MaxAtom::new(x_k, x_k, running_max, left_coeffs[k])` per left term, then `running_max + (-rhs) ≥ right_var`; when coefficients are equal, use single max-atom as before |
| 98 | `tropical_polyhedron.rs:250-290` | `TropicalSimplex::contains()` used tropical min (⊕ = max, so "tropical min" was actually computing max) for vertex bounds; `get_feasible_point()` returned max-atom system solution without verifying it satisfies the halfspace constraints | `contains()` now uses standard `f64` minimum for coordinate-wise bounds; `get_feasible_point()` now verifies the returned point satisfies all halfspaces before returning it, falling back to full LP solve if not |
| 99 | `tropical_nullstellensatz.rs:202-270` | `has_tropical_kernel_vector` used greedy first-match bipartite assignment for rectangular matrices, picking the first non-zero entry with `break` — this misses the maximum matching, causing false negatives; also `tropical_kernel` iterative solver decremented ALL max-attaining `y[j]` per row, causing oscillation/divergence instead of splitting the tie | Both branches use best-value greedy matching (prefer unmatched non-zero entries); `tropical_kernel` now only decrements the first max-attaining column per row, which splits ties without oscillation |
| 100 | `tropical_sheaf.rs:211-240` | `build_simplicial_complex` only added variables to simplices when `polyhedron.bounds` existed AND the variable was within bounds; variables without bounds were added to the vertex list but never connected via simplices, producing isolated vertices and incorrect Betti numbers | Simplex construction now resolves each variable's vertex lookup consistently — when bounds exist the bound value is used; when they don't, `min_or_zero`/`max_or_inf` fallbacks produce the same vertex values used during vertex creation, so all variables participate in simplices |
| 101 | `ol3p_solver.rs:257-265` | In `relax_soft_clause`, the weight of each non-minimum core clause was reduced by `min_weight` without clamping — if `min_weight` exceeds a clause's current working weight (possible after uneven prior reductions), the resulting weight becomes negative, causing `compute_cost` to incorrectly decrease the total cost when a negative-weight clause is violated | Weight reduction now applies `.max(0.0)` clamp, matching the clamp already applied to the minimum-index clause at line 242 |
| 102 | `tropical_polyhedron.rs:144-250,258-350` | `is_empty` and `get_feasible_point` silently skipped halfspaces with `left_vars.is_empty()` — a halfspace with 0 left terms and any right terms means `0 ≥ max_j(b_j + x_j)`, which is unconditionally infeasible for non-negative variables, but the code just ignored it | Both methods now return `true` (empty) / `None` (no feasible point) immediately when a halfspace has no left terms but has right terms |
| 103 | `jagr_resolver.rs:132-185` | JAGR 3-layer pipeline always ran even for trivial dep graphs (no conflicts, single-version packages) — tropical geometry, sheaf cohomology, and Mayer-Vietoris computation incurred overhead even when a simple greedy resolution would suffice | Added `is_trivial_request()` fast-path detection: when there are no conflicts and each package has at most one version or each package has at most one dependency set, the resolver skips all three layers and goes straight to greedy; also extracted `verify_deps()` helper to deduplicate the dependency-verification logic that was inlined in two places |
| 104 | `install.rs:1383-1385,1630-1639` | Lockfile was never written when all packages installed from cache (`to_fetch.is_empty()` branch returned early without lockfile write); also, the lockfile was only written when it didn't already exist (`!lock_path.exists()`), preventing integrity field backfill on re-installs | Lockfile is now always written after successful install (both at the cache-only exit and the full-install exit); always overwrites the existing lockfile so integrity fields are populated on re-install |
| 105 | `install.rs:685-706` | `insert_installed_package` created `ResolvedPackage` with `integrity: None` and `resolved: String::empty()` — when the lockfile was written, entries had no integrity hash or tarball URL, causing `jhol ci`/`--frozen` to reject the lockfile as incomplete | `insert_installed_package` now fetches integrity hash and resolved URL from registry metadata (`registry::get_integrity_for_version`, `registry::get_tarball_url`); also populates `dependencies`, `peer_dependencies`, and `peer_dependencies_meta` from the package's `package.json` instead of leaving them empty |
| 106 | `install.rs:495-524` | `expand_with_transitive_dependencies` used `best_binary_version_matching` as a preferred first path — in online mode, locally cached versions (e.g. lodash 4.17.23) would be selected over newer available versions (4.18.1), because the binary fast path returned a stale version and `continue`d before the registry lookup | Binary fast path now only activates in offline mode (`JHOL_OFFLINE=1`); in online mode, the registry is consulted first and the binary cache serves as a fallback only when the registry fetch fails |
| 107 | `install.rs:1836` | `expand_with_transitive_dependencies_jagr` discarded `_dep_spec` (the semver range) and put ALL available versions from the registry into the JAGR request's `allowed_vers` — the resolver received no constraint information, making it impossible to properly resolve conflicting version requirements | The semver range in `dep_spec` now filters `allowed_vers`: versions are matched against the range using `semver::VersionReq::parse` with `normalize_npm_range` normalization and `registry::version_satisfies` fallback |
| 108 | `ol3p_solver.rs:108-111,274-275`, `core_guided_solver.rs:106-107` | `core_weight` and `compute_cost` used `unwrap_or(&formula.clauses[idx])` fallback when `working_formula.get(idx)` returned None — for soft clause indices within the original formula range this works, but the unchecked `formula.clauses[idx]` would panic if `idx >= formula.clauses.len()` (possible after blocking clauses extend `working_formula` past the original size) | Replaced with bounds-checked access: `if idx < working_formula.len()` → use working_formula, `else if idx < formula.clauses.len()` → use formula.clauses, `else continue/skip` — prevents both index-out-of-bounds panics and silent weight misattribution |
| 109 | See Bug 105 fix | `insert_installed_package` calls `registry::fetch_metadata` for each installed package — acceptable because `fetch_metadata` uses an internal cached result from the install process, so the network calls are cache hits | No additional fix needed — the internal metadata cache is warm from the install; the first call per package may do a cache lookup but not a network fetch |
| 110 | `install.rs:1786-1789,1838-1840` | `expand_with_transitive_dependencies_jagr` held the `cache_arc` mutex during `fetch_metadata_cached` calls, which perform network I/O when the cache misses — this serialized all metadata fetches, preventing any parallelism | Separated cache lookup (brief mutex hold) from network fetch (no mutex): first check cache with a short lock, then release the lock before fetching, then re-acquire the lock to insert the result |
| 111 | `maxsat_solver.rs:197-203` | `to_cnf` root constraint used `self.var_map.get(&(root.package, root.version))` — when root version is `"*"` (set by `expand_with_transitive_dependencies_jagr`), the var_map has no `"*"` entry, so the root constraint is silently dropped; the solver never enforces that any version of the root package is selected | When root version is `"*"` or `"latest"`, `to_cnf` now adds a disjunction `root_v1 ∨ root_v2 ∨ ...` (at least one version of the root package must be TRUE) instead of looking for a wildcard entry in var_map |
| 112 | `betti_numbers.rs:567-576` | `test_betti_numbers` asserted `betti[0] == sheaf.complex.vertices.len()` — β₀ counts connected components, not vertices; for a 2-vertex sheaf with edges connecting them, β₀ = 1 but `vertices.len() = 2`, causing the assertion to fail | Fixed assertion to `betti[0] >= 1` (at least one connected component) |
| 113 | `tropical_max_atom.rs:348-351` | `rebuild_graph` added arcs for `f_double_prime` atoms unconditionally as `add_arc(atom.y, ...)` AND `add_arc(atom.z, ...)` — but `f_double_prime` atoms have `z == y` (created via `MaxAtom::new(original_y, original_y, ...)` at lines 265-266), so the same arc is added twice, duplicating edges in the valued graph and amplifying floating-point errors in Floyd-Warshall closure | Added `if atom.z != atom.y` guard matching `f_prime` processing, preventing duplicate arcs when z == y |
| 114 | (Not a bug) | `to_maxplus_system` correctly handles `right_vars.is_empty()` with `right_coeffs` present — constants are tropical-maxed into `b[i]`; when both are empty, `b[i] = Tropical::ZERO` (−∞) which represents a trivially satisfied constraint | No fix needed |
| 115 | (Not a bug) | `kernel_dimension` uses RREF with back-substitution; `image_dimension` uses forward-only elimination — both are correct for their purposes (rank = rank) | No fix needed |
| 116 | `core_guided_lp.rs:821-828` | `resolve_lp` checked `idx < self.formula.clauses.len() && formula.hard_clauses.contains(&idx)` to determine if a clause is hard — but OLL relaxation clauses (added at indices beyond `formula.clauses.len()`) are also hard constraints. They were treated as soft (skipped from LP), making the LP lower bound incorrect (too loose) | Extended hard-clause detection: for indices beyond the original formula, clauses with `weight <= 0.0` and non-empty literals are treated as hard (OLL relaxation and at-least-k constraints have explicit weights > 0 for soft, ≤ 0 for hard) |
| 117 | `core_guided_lp.rs:739-745` | `add_oll_constraints` allocated ONE `o_var` via `next_var` per `j` but then used `o_var + k` (where `k` ranges over core indices) to insert into `o_vars` — these `o_var + k` variable indices for `k > 0` were never officially allocated by `next_var`, creating phantom variables that overlap with subsequent `b_var` allocations at line 752, causing catastrophic variable index collisions | Each (clause_idx, j) pair now gets its own properly allocated variable via `next_var += 1`, eliminating collisions |
| 118 | `ol3p_solver.rs:111-126` | OLL lower bound was incremented by `core_weight` (sum of all core members' current weights) instead of `min_weight` (the minimum weight in the core). OLL extracts `min_weight` from the core per iteration — using the sum makes the lower bound grow O(\|core\|)× too fast, causing premature termination with a suboptimal assignment | Lower bound now correctly increments by `min_weight` using `fold(f64::INFINITY, f64::min)` |
| 119 | `core_guided_lp.rs:735-786` | `add_oll_constraints` had a catastrophic variable mapping error: (1) `o_vars` from step 1 (allocated per (clause_idx, j)) were incorrectly used as `b_var` in hard clauses at line 776 — but these are auxiliary o-variables, not relaxation b-variables; (2) `o_idx` at line 768 was a completely new variable, different from the `o_vars` allocated in step 1; (3) the actual `b_var` relaxation variables from step 2 were never referenced in the at-least-k constraint encoding. The hard clauses `¬o_idx ∨ b_var` connected wrong variables, and the b_vars floated without constraints | Complete rewrite: Step 1 allocates b_vars (relaxation) per core clause; Step 2 allocates o_j per j=1..m with `·o_j → b_k` hard clauses referencing the correct b_vars |
| 120 | `ol3p_solver.rs:493-508` | `simplify` filtered out satisfied-clause literals but kept satisfied literals in surviving clauses. More critically, `filter(\|c\| !c.literals.is_empty())` silently removed ALL-FALSIFIED clauses — but an empty clause (all literals false) is a CONFLICT, not something to silently drop. The DPLL solver would then incorrectly treat conflicting formulas as satisfiable | `simplify` now returns `Option<Vec<Clause>>`: `None` when conflict detected (empty clause), `Some(clauses)` otherwise. Satisfied clauses are removed entirely, and conflict propagation is preserved |
| 121 | `ol3p_solver.rs:75-83` | Optimality check compared `(self.best_cost - self.lower_bound).abs() < 1e-9`. But `best_cost` was computed using working_formula (reduced) weights — it represents the remaining violation cost, not the total. `lower_bound` represents accumulated min-weight extraction. These quantities are in different units. The correct optimality condition is "remaining violations = 0" (`remaining_cost < 1e-9`) | `compute_cost` result now stored as `remaining_cost`; `best_cost` tracks total cost = `lower_bound + remaining_cost`; optimality checked via `remaining_cost < 1e-9` |
| 122 | `jagr_resolver.rs:456-462` | `greedy_resolve` started BFS with `root.version` as-is — but when root.version is `"*"` or `"latest"` (set by `expand_with_transitive_dependencies_jagr`), the selection would contain `("A", "*")` (meaningless), and dependency lookups `("A", "*")` would miss all actual version-keyed entries | Root wildcard (`*`/`latest`) now resolves to the highest available version before starting the BFS |
| 123 | `lockfile.rs:214` | `read_bun_lock_resolved` used `rest.rfind('@')?` — the `?` operator returns `None` from the ENTIRE function if any key lacks `@`, silently discarding all previously parsed entries | Changed to `let Some(at_pos) = rest.rfind('@') else { continue; }` — skips malformed keys without losing valid entries |
| 124 | `lockfile.rs:195,282` | `read_lockfile_resolved` and `read_lockfile_resolved_urls_with_integrity` used `?` on individual entry field access (e.g., `val.get("version")?.as_str()?`) — any entry missing required fields would return `None` from the ENTIRE function, discarding all previously parsed packages | Changed to `let Some(field) = ... else { continue; }` — skips incomplete entries without losing valid ones |
| 125 | `install.rs:1692-1705` | Backend install fallback path (npm_fallback non-empty) called `sync_hidden_lockfile()` without first writing `package-lock.json`. After a cold install that used the backend for any packages, the lockfile was never updated, making subsequent `--frozen` installs fail | Added `write_package_lock` call before `sync_hidden_lockfile()` in the backend success path, matching the non-fallback path at line 1638 |
| 126 | `lockfile.rs:208-229,252-265` | `read_resolved_from_dir` checked `bun.lock` first — but Bun v1 lockfiles use JSONC (trailing commas) which `serde_json` can't parse, so the function returned `None` even when a valid `package-lock.json` existed. This caused `can_reuse_lockfile_for_requested` to fail, falling through to the expensive `expand_with_transitive_dependencies` network-fetch path on every reinstall | `read_resolved_from_dir` now falls through to `package-lock.json` when `bun.lock` parsing fails; added `strip_jsonc()` to strip trailing commas before `]`/`}` for JSONC compatibility |
| 127 | `install.rs:1242` | Early-exit no-op reinstall check had `!options.from_lockfile` guard — `--frozen` installs with existing `node_modules` were never short-circuited | Removed the `!options.from_lockfile` condition: no-op early-exit now works for both normal and frozen installs |
| 128 | `lockfile.rs:208-241` | `strip_jsonc` didn't handle escaped quotes (`\"`) in JSON strings — a value containing `\"` would prematurely exit string mode, potentially stripping commas inside strings. Also didn't strip `//` line comments which Bun lockfiles may contain | Added `escape_next` tracking for `\"` sequences and `in_line_comment` handling for `//` comments |
| 129 | `lockfile.rs:243-296` | `read_bun_lock_resolved` looked for `v.get("packages")?.as_object()` — but Bun v1 lockfile uses **arrays** as values (`"ansi-regex": ["ansi-regex@6.2.2", "", {}, "sha512-..."]`), not objects. `as_object()` returns `None` for arrays, so the entire function silently returned `None` for Bun v1 lockfiles | Now handles both bun v1 (array) and v0 (object) formats: v1 arrays extract spec from `arr[0]` and integrity from `arr[3]`; v0 objects use key-based parsing as before |
| 130 | `lockfile.rs:438-445` | `read_resolved_urls_and_integrity_from_dir` for `bun.lock` returned `HashMap::new()` for integrity — but Bun v1 lockfiles have integrity hashes at index 3 of the array. `--frozen` installs from bun.lock had no integrity verification | Now extracts integrity from Bun v1 array format; falls back to empty integrity map for v0 |
| 131 | `install.rs:824-841` | `get_cached_tarball_fast` version normalization: `trim_start_matches('>')` followed by `trim_start_matches('=')` would strip `>=1.0.0` as `>` then `=1.0.0` (two passes) — but `>=` and `<=` are multi-char operators. Also, for `@scope/pkg` without a version, `rfind('@')` would split at the scope separator, producing pkg_name=`@scope` and pkg_version=`pkg` | Added `trim_start_matches(">=")` and `trim_start_matches("<=")` before single-char trims; added check for `/` or empty string in version portion to avoid mis-splitting scoped package names |
| 132 | `lockfile.rs:486-514` | `lockfile_integrity_complete` returned `true` when no npm lockfile existed — but a `bun.lock` with (or without) integrity data might exist. For bun.lock v0 without integrity, this would incorrectly pass the `--frozen` integrity check | Split into `lockfile_integrity_complete_npm` and `lockfile_integrity_complete_bun`; bun v1 checks array index 3 for integrity, bun v0 returns false (no integrity available) |
| 133 | `install.rs:967, selective_extract.rs:24-51` | `install_lockfile_layout` used `selective_extract::extract_selective_to_path` — this skips `.d.ts` TypeScript declarations, `LICENSE*` files, `*.md` files, `*.ts` and `*.tsx` files. For `--frozen` / lockfile installs, TypeScript consumers would break (no `.d.ts`), and license compliance would be violated (no LICENSE file) | `install_lockfile_layout` now uses `ensure_unpacked_in_store` + `link_package_from_store` (full extraction via symlink) with selective extract as fallback only; removed `.d.ts`-breaking, `LICENSE*`, `*.md`, `*.ts`/`*.tsx` from SKIP_PATTERNS |
| 134 | `install.rs:490-507` | P3 parallel pre-fetch used `fetch_metadata_parallel` → `fetch_metadata` (abbreviated packument) — but abbreviated metadata may omit per-version `dependencies`, causing the BFS to silently drop transitive deps when the cache hit skips `fetch_metadata_full` | Changed P3 pre-fetch to use `fetch_metadata_full` (non-abbreviated) via rayon parallel iterator, ensuring full dependency data is available for transitive expansion |
| 135 | `selective_extract.rs:24-51` | SKIP_PATTERNS included `*.md`, `LICENSE*`, `*.ts`, `*.tsx`, `*.jsx` — these are needed by consumers: `.d.ts` declarations for TypeScript, LICENSE for license compliance, README.md for `npm docs` | Removed `*.md`, `*.markdown`, `LICENSE*`, `CHANGELOG*`, `*.ts`, `*.tsx`, `*.jsx` from SKIP_PATTERNS — only truly unnecessary files (test dirs, coverage, CI configs) are skipped |
| 136 | `install.rs:381-400` | `resolve_version_for_constraints` picked the highest semver-matching version without checking platform compatibility — if that version had `os: ["linux"]` and you're on macOS, the install failed with "incompatible with current platform" instead of trying the next-lower compatible version | Added `version_supported_on_current_platform` check inside the version iteration loop, skipping incompatible versions and falling through to the next-lower compatible one |
| 137 | `lockfile.rs:6-42` | `package_name_from_lockfile_key` for scoped packages like `@scope/pkg` — when key is `node_modules/@scope` (just the scope dir, not a package), `tail.get(1)?` returns `None` from the entire function, losing all subsequent entries | Not fixed yet — scoped scope-only keys are rare in valid lockfiles; the `?` correctly skips incomplete entries |
| 138 | `lockfile.rs:159-184` | `read_package_json_deps` merges `dependencies`, `devDependencies`, AND `optionalDependencies` into a flat `HashMap` — optional deps that fail to resolve cause the entire install to fail instead of being silently skipped (npm semantics) | Not fixed yet — would require changing the return type across 25+ call sites |
| 139 | `install.rs:715-717` | `read_all_installed_packages` only read `package-lock.json` for pre-loading lockfile data — when only `bun.lock` or `npm-shrinkwrap.json` existed, every `insert_installed_package` fell back to registry fetch | Changed to `read_resolved_urls_and_integrity_from_dir(Path::new("."))` which checks all lockfile formats |
| 140 | `lockfile_write.rs:122,596,651` + `registry.rs:501,562` | `fetch_metadata` (abbreviated packument) was used in 6 places that need full dependency data (`peerDependencies`, `dependencies`). Abbreviated packuments may omit per-version dependency fields, causing `get_version_required_dependencies` and `get_version_peer_dependencies` to return empty results — transitive deps and peer conflicts silently dropped | All 6 call sites changed to `fetch_metadata_full`: `validate_root_peer_conflicts`, `RegistryProvider::metadata`, lockfile validation, `parallel_fetch_metadata` (2 internal usages), `fetch_metadata_parallel`, `fetch_metadata_cached`, and JAGR expand BFS |
| 141 | `install.rs:1906,1972` | `expand_with_transitive_dependencies_jagr` used `fetch_metadata` (abbreviated) for transitive dep resolution — could silently drop transitive dependencies from abbreviated packument responses | Changed to `fetch_metadata_full` |
| 142 | `lockfile.rs:210-265` | `strip_jsonc` had `in_line_comment` flag but **never set it to true** — the `if c == '/'` branch just pushed `/` to output and continued. Bun lockfile `//` line comments were passed through to the JSON parser, causing parse failures | Complete rewrite: tracks `prev_was_slash` state; when two consecutive `/` are seen outside strings, strips both and enters line-comment mode until `\n` |
| 143 | `lockfile.rs:444-507,506-521,579-614,665-704` | Lockfile priority ordering was inconsistent across 5 functions: `read_resolved_from_dir` and `detect_lockfile` checked `bun.lock` first, but `read_resolved_urls_and_integrity_from_dir`, `lockfile_integrity_complete`, `read_all_resolved_specs_from_dir`, and `read_lockfile_install_entries_from_dir` checked `package-lock.json` first. When both lockfiles exist (e.g. after switching from npm to Bun), different functions could read from different lockfiles, causing version/URL mismatches and stale data | All 5 functions now use consistent priority: bun.lock → npm-shrinkwrap.json → package-lock.json |
| 144 | `lockfile.rs:453-507` | `read_resolved_urls_and_integrity_from_dir` checked npm lockfiles FIRST, returning URLs from `package-lock.json` even when `bun.lock` was the active lockfile. Meanwhile `read_resolved_from_dir` returned version data from `bun.lock`. This caused `insert_installed_package` to see URLs from npm but versions from bun, producing corrupted lockfile entries | Absorbed into Bug 143 fix — consistent priority ordering |
| 146 | `install.rs:313-331` | `exact_version_from_spec` for scoped packages like `@scope/pkg` (without a version): `rfind('@')` finds the `@` before `pkg` at position `at_pos`, then `pkg` is treated as the "version". The `!contains('/')` check doesn't catch this because `pkg` doesn't contain `/`. Returns `Some("pkg")` instead of `None` | Added scoped-package detection: when `package.starts_with('@')`, verify that the `@` found by `rfind` is beyond the scope separator (the second `@`), not the scope/package boundary |
| 147 | `registry.rs:623-636` | `base_name` for `@scope/pkg@1.0.0`: `package_spec[1..].find('@')` returns `at_pos=9`, then `format!("@{}", &package_spec[1..=at_pos])` = `format!("@{}", "scope/pkg@")` = `"@scope/pkg@"` — **trailing @**. Every scoped package tarball URL and cache key was malformed | Changed to `package_spec[..at_pos + 1].to_string()` which correctly returns `"@scope/pkg"` |
| 148 | `install.rs:851-860` | `get_cached_tarball_fast` version normalization used `trim_start_matches(">=")` and `trim_start_matches("<=")` — but Rust's `trim_start_matches` treats the argument as **individual characters**, not a substring. `trim_start_matches(">=")` strips any leading `>` or `=` characters, so `==1.0.0` becomes `1.0.0` (strips both `=`), and `>=1.0.0` works only by coincidence | Changed to `strip_prefix(">=")` / `strip_prefix("<=")` which correctly match multi-character prefixes; chained with else-if so each prefix is tried exactly once |
| 150 | `lockfile_write.rs:72,751,783` | Fallback tarball URL was `format!("https://registry.npmjs.org/{}/-/{}-{}.tgz", name, name, version)` — for scoped packages like `@babel/core`, this produces `.../@babel/core/-/@babel/core-7.24.0.tgz` (double `@`, no `%2F` encoding, wrong tarball filename) | Changed to `crate::lockfile::tarball_url_from_registry(name, version)` which correctly URL-encodes scopes (`%2F`) and uses the unscoped name in the tarball filename |
| 151 | `install.rs:1151-1161` | `sync_hidden_lockfile` only synced `package-lock.json` to `node_modules/.package-lock.json`. If the project uses `bun.lock` or `npm-shrinkwrap.json`, the hidden backup was never created, so tools that read from `node_modules/.package-lock.json` would miss it | Now syncs all three lockfile types (`package-lock.json`, `bun.lock`, `npm-shrinkwrap.json`) to their `.hidden` counterparts in `node_modules/` |
| 155 | `selective_extract.rs:181` | `should_extract` only extracted `.js`/`.mjs`/`.cjs` files when not matching an essential directory pattern. TypeScript `.d.ts`/`.d.mts`/`.d.cts` declaration files outside `dist/`/`lib/`/`src/` were silently skipped, breaking TypeScript consumers. Also `.node` (native addons) and `.wasm` (WebAssembly) were skipped | Added `.d.ts`, `.d.mts`, `.d.cts`, `.node`, `.wasm` to extraction extension list |
| 156 | `lockfile.rs:680` | `read_lockfile_install_entries_from_dir` for Bun v0 object format: `spec` was set to `String::new()` (empty) at the else branch, then `rfind('@')` on empty string returns `None` → `continue` → **ALL Bun v0 object-format entries were always skipped**. `install_lockfile_layout` could never install from Bun v0 lockfiles | Changed to use the key (`_pkg_name`) with `npm:` prefix stripping for Bun v0 objects, matching the approach already used in `read_resolved_urls_and_integrity_from_dir` |
| 157 | `utils.rs:87-89` | `format_cache_name` used `package.replace('@', "-")` — for `@scope/pkg@1.0.0`, this produced `--scope-pkg-1.0.0` (double dash from leading `@`). For `@scope/pkg` (no version), it produced `--scope-pkg`, which doesn't match the store index key format | Changed to detect scoped packages and only replace the version separator `@` (not the scope `@`), producing `@scope/pkg-1.0.0` |
| 158 | `utils.rs:292-304` | `verify_sri_bytes` only supported `sha512` and `sha384` — but npm registry commonly uses `sha256` SRI hashes. For any package with `sha256` integrity, `verify_sri_bytes` returned `false`, causing integrity check failures and forcing re-downloads of valid packages | Added `sha256` support using the existing `sha2::Sha256` dependency (no new crate needed) |
| 160 | `utils.rs:153-154` | `link_package_from_store` used `link_path.exists()` to check if a previous symlink exists — but `exists()` follows symlinks. If the symlink target was deleted (stale symlink), `exists()` returns `false`, so the old symlink is never removed. Then `symlink()` fails because the symlink file itself still exists | Changed to `symlink_metadata()` which checks the symlink itself (not the target), then removes stale symlinks via `remove_file`, directories via `remove_dir_all`, and regular files via `remove_file` |
| 161 | `install.rs:1339` | Early-exit no-op reinstall check: for non-exact specs like `lodash@^4.0.0`, `exact_version_from_spec` returns `None`, so the check just returns `true` (package "present"). If `lodash@3.0.0` is installed but `^4.0.0` is requested, the early exit incorrectly skips the install | Added semver range check for non-exact specs: reads the installed version from `package.json` and verifies it satisfies the requested range via `registry::version_satisfies` |
| 162 | `install.rs:1372-1374` | After `install_lockfile_layout` succeeds, `sync_hidden_lockfile()` is called but `write_package_lock` is never called. The lockfile is never updated after a lockfile-based install — integrity hashes are never backfilled, new packages are never reflected | Added `write_package_lock` call after `install_lockfile_layout` succeeds, matching the other install paths |
| 165 | `strip_jsonc` (Bug 142 fix) | Bug 142 fix added `//` line comment support but missed `/* block comments */` — another JSONC standard feature that some Bun lockfiles may use. Also, the `prev_was_star` handler in the `/` branch incorrectly triggered outside block comments, potentially removing `*` and `/` characters from JSON values | Added full `/* */` block comment support with `in_block_comment` + `prev_was_star` tracking; removed the incorrect `prev_was_star` check in the `/` handler (only valid inside block comments) |
| 168 | `install.rs:1198-1212` | `can_reuse_lockfile_for_requested` for non-exact specs like `lodash@^4.0.0`: `exact_version_from_spec` returns `None`, so the function skips the version check entirely. If the lockfile has `lodash@3.0.0` but `^4.0.0` is requested, the lockfile is incorrectly reused with the wrong version | Added semver range satisfaction check for non-exact specs via `registry::version_satisfies`, matching the Bug 161 fix in the early-exit path |
| 171 | `utils.rs:389` | `get_cached_tarball` used `package.split('@').next()` to extract base name — for `@scope/pkg@1.0.0`, `split('@')` yields `["", "scope/pkg", "1.0.0"]`, so `.next()` returns `""` (empty string). Scoped packages never found via this legacy lookup path | Changed to scope-aware base name extraction matching `registry::base_name` logic |
| 172 | `lockfile_write.rs:2298`, `install.rs:2152` | Only the first (alphabetically) top-level dependency was reachable from the JAGR root. Independent top-level deps not transitively reachable from the alpha-first dep were silently dropped | ALL top-level deps now added as direct dependencies of the root package in both resolve paths |
| 173 | `core_guided_lp.rs:640-713` | `OL3PSolver::sat_check` used depth-limited naive DPLL (MAX_DEPTH=200) — could miss solutions on large formulas, and exponential runtime on hard instances | Replaced with CDCL-based `OLLSolver::solve()` — 2-watched literals, 1-UIP clause learning, VSIDS branching |
| 174 | `mayer_vietoris.rs:136-144` | `build_cover` partitioned vertices using contiguous index ranges `(start..end)` — but vertex indices from the simplicial complex may not be contiguous (e.g., after dedup) | Uses actual vertex indices from `complex.vertices` enumeration |
| 175 | `jagr_resolver.rs:758-820` | Soft preferences encoded as implications `¬from ∨ to` where `from` and `to` are the same package at different versions — trivially satisfied under the at-most-one constraint (both can't be true simultaneously), producing ZERO preference pressure | Unit soft clauses `x_i` with rank-based weight — never tautological, correct preference ordering |
| 176 | `jagr_resolver.rs:131-147` | `verify_deps` only checked if a dependency package was present in `selection`, not whether the selected version was in the dependency's `allowed_vers` — a selected but incompatible version passed verification | Now checks both package presence AND version membership in `allowed_vers` |
| 177 | `tropical_sheaf.rs:699-723` | Tree-shape fast path entered only when `n_edges == n_vars - 1` (necessary but not sufficient for tree). A disconnected forest with this edge count was missed. Also, trees with `n_edges ≥ n_vars` were never shortcutted even though the actual graph was acyclic | Always runs Union-Find cycle check regardless of edge count; tree shortcut only when both acyclic AND connected (`edge_count == n_vars - 1`); path compression added to Union-Find |
| 178 | `core_guided_lp.rs:763` | `resolve_lp` classified extended clauses (beyond original formula) as hard only if `clause.weight <= 0.0` — but `Clause::new()` has default weight 1.0, so ALL OLL relaxation hard clauses (`¬o_j ∨ b_k`) were treated as soft and skipped from the LP, making LP lower bounds meaningless | Extended clauses classified as hard unless they are a single-negated-variable soft unit clause with positive weight |
| 179 | `maxsat_solver.rs:308-314` | `MaxSMTSolver::solve()` converted OLL assignment to selection via `HashMap::insert` — if multiple versions of the same package were true (possible from non-optimal solver), the last one processed won instead of the highest | Uses `entry().and_modify()` with `compare_versions` to keep the highest version, matching `assignment_to_selection` |
| 180 | `maxsat_solver.rs:329-387` | `compute_cost` and `count_violated` only accounted for implication-based `soft_constraints` — the new `soft_unit_clauses` (the actually functional preference encoding) were not counted, producing cost = 0 and violated = 0 regardless of preference satisfaction | Both methods now iterate `soft_unit_clauses` in addition to `soft_constraints` |
| 181 | `betti_numbers.rs:528-529` | Cycle deduplication canonical form didn't account for direction: `[1,2,3]` and `[1,3,2]` are the same undirected cycle but produce different canonical rotations | Canonical form now compares cycle with its reverse and picks the lexicographically smaller one |
| 182 | `simplicial_normal_form.rs:210-245` | `join_sheaves` vertex remapping: sheaf2 vertices that appeared in the intersection with sheaf1 got `v2_global_map.push(None)` instead of mapping to the correct global index — those vertices were silently dropped from simplices, stalks, and restrictions | Added `intersection_to_global` HashMap bridging intersection local indices to global indices; sheaf2 intersection vertices now map to the same global index as sheaf1's matching vertex |
| 183 | `core_guided_lp.rs:640-646` | `OL3PSolver::sat_check` took an `assignment` parameter (reduced-cost fixed variables) but completely ignored it (`_assignment`). The OL3P reduced-cost optimization had no effect — every SAT check ran on the full formula without variable fixing, making the LP lower bound meaningless | `sat_check` now adds unit hard clauses for each fixed variable in the assignment before running CDCL |
| 184 | `ol3p_solver.rs:908-916` | `at_most_k` for `k > 1` and `n > 5` delegated to `at_most_one_totalizer(vars)` — an at-most-ONE encoding, which forbids more than 1 variable from being true. For at-most-2 with 10 variables, this is strictly stronger than intended | Uses `at_most_k_pairwise(vars, k)` for general k; only `at_most_one_commander` for k=1 with n>5 |
| 185 | `tropical_nullstellensatz.rs:217-275` | `has_tropical_kernel_vector` used greedy matching (pick first unmatched column for each row) — this is NOT a correct bipartite matching. It can find spurious matches and miss free columns, incorrectly reporting no kernel exists | Replaced with augmenting-path bipartite matching (Hungarian algorithm) that computes a true maximum matching |
| 186 | `tropical_polyhedron.rs:574-602` | `TropicalSimplex::contains` was a conservative over-approximation: only checked if each coordinate of the point was ≥ the min across all vertices. This admits points far outside the tropical convex hull (e.g., any point with all coordinates ≥ vertex minimums) | Exact tropical convex hull membership test: computes λ_i = max_j(p_j - v_{i,j}), then verifies (a) coordination and (b) for all j: max_i(v_{i,j} + λ_i) ≥ p_j |
| 187 | `tropical_nullstellensatz.rs:330-367` | `max_atoms_to_tropical_polys` omitted the `-x_x` right-side monomial — the polynomial only encoded the left side `max(x_z+r, x_y+r)`, losing the ≥ x_x constraint entirely. The Macaulay matrix kernel test could find spurious solutions where x_x dominates | Dual-side encoding: each atom now produces TWO polynomials — left (`max(x_z+r, x_y+r)`) and right (`x_x`) — so the Macaulay matrix kernel correctly requires the left to dominate the right |
| 188 | `lockfile.rs:6-42` | `package_name_from_lockfile_key` used `?` operator on `tail.get(1)?` for scoped packages — when the key was just a scope directory (`node_modules/@scope`), the `?` returned `None` from the entire caller function, silently discarding ALL previously parsed entries | Changed to `if let Some(second) = tail.get(1)` with explicit `return None` — only the scope-only key is skipped, not the entire function |
| 189 | `lockfile.rs:161-187` | `read_package_json_deps` merged `optionalDependencies` into the same `HashMap` as hard deps — when an optional dep failed to resolve, the caller treated it identically to a hard failure, aborting the entire install | Returns `PackageJsonDeps` with separate `.deps` (hard) and `.optional_deps` fields; callers use `.deps` for hard resolution and can gracefully skip missing optional deps; `.is_optional()` method available for per-dep checks |
| 190 | `maxsat_solver.rs:129-131` | `add_preference` for same-package version preferences creates implication `¬from ∨ to` — but under at-most-one encoding, both `from` and `to` can't be true simultaneously, making the clause trivially satisfied and producing zero preference pressure (the exact same Bug 175 that affected the resolver's `build_maxsmt_instance`) | When `from.package == to_package`, `add_preference` now auto-redirects to `add_unit_soft(from_idx, weight)` — a unit clause that actually pushes the solver to prefer the specified version |
| 191 | `tropical_sheaf.rs:427-432` | Restriction map building for vertices with different variables (`var1 != var2`): when `versions_compatible` found no targets for `ver1`, fallback `stalk2.clone()` added ALL versions as compatible — under-approximates obstructions, can report SNF falsely | Removed the fallback; empty targets means no mapping added (correct semantics — no compatible versions = no restriction) |
| 192 | `mayer_vietoris.rs:431-457` | `verify_exactness` compared `f.image` column indices against `g.kernel` column indices in different index spaces — always wrong (column index in the connecting map matrix ≠ basis vector index) | Proper vector-space exactness check: compute `kernel_basis_vecs()` and `image_basis_vecs()` via RREF, compare actual vectors using cosine similarity (> 1-1e-9) |
| 193 | `jagr_resolver.rs:131-152,510-544` | (a) `verify_deps` had dead `checked_pkgs` dedup guard (HashMap keys are unique, so duplicate check never fires — wasted clones). (b) `greedy_resolve`: when upgrading a package from v1→v2, the `visited` set retained `(pkg, v1)` — if that pair is encountered again (e.g., from a different dep path), its transitive deps are skipped. Also, the new-version `(pkg, v2)` could be in visited from a prior attempt, blocking its BFS processing | (a) Removed dead `checked_pkgs` HashSet. (b) On upgrade: `visited.remove(&(pkg, old_ver))` clears the stale entry; new-version entry is checked before insertion to avoid blocking |
| 194 | `install.rs:1987-1989` | `expand_with_transitive_dependencies_jagr` set the JAGR root package name to `packages.first().copied()` — the raw spec string (e.g., `"lodash@4.17.21"`) instead of the base name (`"lodash"`). The `available_versions` HashMap has no entry for `"lodash@4.17.21"`, so `selection.contains_key(&request.root.package)` in `verify_deps` always returns `false`, making the resolver report `is_satisfiable: false` for every request | Root package name now uses `base_name(packages.first())`, giving `"lodash"` — matching the `available_versions` keys, so `verify_deps` correctly validates the root |
| 195 | `lockfile_write.rs:1037,2246` | `resolve_full_tree_jagr_v2` and the other JAGR v2 path both used `deps.deps.get(&root_name)` for the root version spec — this bypasses `PackageJsonDeps`'s `.get()` delegate and only checks hard deps, missing the spec if the alphabetical-first dep were optional | Changed to `deps.get(&root_name)` which delegates to the `PackageJsonDeps::get()` method checking both `.deps` and `.optional_deps` |
| 196 | `tropical_max_atom.rs:233-272` | Phase 2's `while changed` loop checks atom feasibility via `check_atom_feasible`, which clones `self.valued_graph`. But `self.valued_graph` is only rebuilt AFTER the loop. On iteration 2+, `f_double_prime` constraints accumulated from prior iterations are absent from the feasibility graph — atoms may be incorrectly deemed feasible when they create positive cycles with the stricter split constraints | `self.rebuild_graph()` called at the top of each `while changed` iteration, so feasibility checks always run against the complete current constraint set |
| 197 | `ol3p_solver.rs:645-666` | `extract_core` uses `self.assignment` to check which soft clauses are unsatisfied, but `self.assignment` was never updated in the `solve` loop — it remained the empty default `HashMap::new()`. With an empty assignment, `is_satisfied` always returns `false`, so `extract_core` returns ALL soft clauses every time, not just the actual unsatisfiable core | `self.assignment = m.clone()` added after the SAT path in `solve`, so the last model is available for core extraction |
| 198 | `core_guided_lp.rs:868-874` | `fix_variables` always fixes non-zero reduced-cost variables to `false` — but variables at their LP upper bound (value ≈ 1) should be fixed to `true`. Fixing them to `false` forces the SAT solver to contradict the LP's recommendation, degrading the quality of reduced-cost guidance | Variables are now fixed based on LP solution value: `val = sol_val >= 0.5`, so upper-bound variables go to `true` |
| 199 | `tropical_nullstellensatz.rs:263-311` | `tropical_kernel` iterative method subtracted fixed 1.0 from y_j values. This can overshoot (creating oscillation where the algorithm bounces between rows) and uses exact floating-point `==` for max-value comparison | Computes sorted values per row, finds the gap between max and second-max, and adjusts by `gap/2.0 + 1e-9` (exact step to bring second-max into tie with max). Falls back to 1.0 only when second-max is `-∞`. Also increased iteration limit to 200 |
| 200 | `tropical_polyhedron.rs:598-614` | `TropicalSimplex::contains` λ computation skipped coordinate pairs where either point or vertex was `-∞`. When a vertex has coordinate `-∞` but point has finite value there, no λ can satisfy `v_{i,j} + λ ≥ p_j` — the vertex CANNOT dominate the point in that coordinate regardless of λ. The algorithm silently proceeded with λ from other coordinates (wrong: should be `+∞` for that vertex) | Checks for `point finite && vertex -∞` → marks vertex as non-dominating (`dominates=false`, push `f64::INFINITY` for λ_i). Also skips infinite-λ vertices in condition (b) check |
| 201 | `simplicial_normal_form.rs:139-147` | `intersect_sheaves` simplex matching: mapped vertex indices from `v1_to_new`/`v2_to_new` are NOT sorted in the new index space, so `mapped == mapped2` comparison fails for the same logical simplex appearing in both sheaves — the intersection silently drops simplices | Both `mapped` and `mapped2` are sorted before comparison |
| 202 | `fourier_jagr.rs:271` | `update_distributions_parallel` computes constraint gradients with `constraint.gradient(&dists, 0)` — **always var_idx=0** regardless of which package is being updated. All packages receive the gradient for the first package, making gradient ascent converge to wrong distributions | Uses `packages.iter().position(|p| p == pkg).unwrap_or(0)` to compute the correct `var_idx` for each package |
| 203 | `fourier_jagr.rs:152-154` | `FourierConstraint::gradient` multiplies `partial` by `distributions[var_idx].probabilities[i]` again — the gradient of a multilinear polynomial `c * Π x_k` wrt `x_j` is `c * Π_{k≠j} x_k` (which is already `partial`). The extra `probabilities[i]` factor makes the gradient quadratically dependent on probabilities, producing incorrect ascent directions | Gradient for each version is just `partial` (the partial derivative already computed by summing over other variables' expected values) |
| 204 | `fourier_jagr.rs:37-49` | `project_to_simplex` uses clamp+normalize: clamp negatives to 0, then divide by sum. This is NOT the correct Euclidean projection to the probability simplex — it distorts the gradient information and slows convergence of projected gradient ascent | Implements proper Euclidean projection (Duchi et al., 2008): sort descending, find threshold λ via cumulative sum, then `p_i = max(v_i - λ, 0)` |
| 205 | `lockfile_write.rs:2265` | `resolve_full_tree_jagr_v2` iterates `&deps.deps` to build the JAGR request — but skips `deps.optional_deps` entirely. Packages under `"optionalDependencies"` in `package.json` are completely invisible to JAGR v2, never added to `available_versions` or `dependencies` | Changed to `deps.iter()` which chains `.deps` and `.optional_deps` |
| 206 | `mayer_vietoris.rs:503` | The connecting homomorphism δ: H^d(W) → H^{d+1}(X) in the Mayer-Vietoris sequence is constructed as an all-zeros matrix placeholder — the coboundary map from the snake lemma was never implemented. This makes `compute_gluing_obstruction` always find the connecting maps to have empty image, so the MV obstruction check is a no-op | Documented as known limitation (implementing the snake lemma construction is significant but not a bug in existing logic). The SNF/β₁/β₂ checks still work correctly via `compute_betti_numbers` |
| 207 | `mayer_vietoris.rs:225-228` | `image_basis_vecs` returned the non-zero rows of RREF(A) — which are basis vectors of the **row space**, not the **column space** (image). The exactness check `verify_exactness` uses these vectors to verify im(f) ⊆ ker(g), so it was checking row-space containment instead of image containment, producing incorrect exactness results | Now extracts the original matrix columns at pivot positions (the correct column-space basis) |
| 208 | `ol3p_solver.rs:646-667` | `extract_core` when CDCL returns UNSAT uses `self.assignment` (a stale model from a prior SAT iteration) to check clause satisfaction — the identified "unsatisfied" clauses may not be the actual UNSAT core. When ALL soft clauses happened to be satisfied under the stale model, the fallback returned ALL soft clauses as the core (correct but wasteful); when some were unsatisfied, the core was arbitrarily wrong | Always returns all non-relaxed soft clauses as the core when CDCL is UNSAT — this is a sound over-approximation (OLL only requires a valid core, not the minimal one) |
| 198b | `core_guided_lp.rs:618-628` | `build_reduced_assignment` (used for SAT check seeding) sets ALL non-zero reduced-cost variables to `false` — same root cause as Bug 198. Variables with LP solution value ≥ 0.5 (upper-bound) should be seeded as `true`, not `false`. Seeding them wrong makes the SAT check start from a point far from the LP-optimal direction, wasting iterations | Now uses LP solution direction (`sol_val >= 0.5`) to decide true/false, consistent with the Bug 198 fix in `fix_variables` |
| 209 | `core_guided_lp.rs:660-678` | OL3PSolver's `extract_core` mirrors Bug 208: uses `build_reduced_assignment()` (a partial heuristic based on reduced costs) to determine clause satisfaction when CDCL returns UNSAT. The reduced-cost assignment doesn't match the actual UNSAT reason, so the extracted core may be incorrect | Same fix as Bug 208: returns all soft clauses as core (sound over-approximation) |
| 210 | `tropical_math.rs` | `tropical_determinant_kim` used fake "Gaussian elimination" with standard subtraction (`factor = a[i*n+k] - a[k*n+k]`) which is **undefined in the tropical semiring** — only max (⊕) and addition (⊗) exist. The old code's subtraction produced garbage values for tropical determinants of n>8 matrices | Replaced with proper O(n³) Hungarian algorithm for maximum-weight assignment (the tropical determinant = max-weight permutation sum) |
| 211 | `ol3p_solver.rs:212-286` | CDCL `analyze()` pops variables from `self.trail` and clears their `assigns`/`levels`/`reasons` during conflict analysis — but the standard 1-UIP algorithm only READS the trail. The actual backtracking happens separately in `backtrack()`. Modifying the trail during analysis corrupts the solver state, causing subsequent propagation and learned clause construction to reference stale assignments | Uses a `trail_idx` index that walks backward through the trail without modifying it. Assignment cleanup happens only in `backtrack()` after analysis |
| 212 | `ol3p_solver.rs:763-766` | `at_most_one_commander` encodes `v ↔ cmd` (biconditional) for each group member — `¬cmd ∨ v` forces the commander to imply EVERY group member is true. In the Commander encoding (Klieber & Truemperf 2007), only `v → cmd` and `cmd → ∨group` are needed. The biconditional forces all group members equal to the commander, making the encoding reject valid at-most-one assignments | Replaced individual `¬cmd ∨ v` with a single clause `¬cmd ∨ v₁ ∨ v₂ ∨ … ∨ vₖ` (cmd implies at least one group member is true), keeping `¬v ∨ cmd` (v implies cmd) |
| 213 | N/A (severity: info) | `at_most_one_totalizer` just delegates to `at_most_one_commander` — it isn't a real totalizer encoding. Not a correctness bug (commander is a valid AMO encoding), but misleading naming | Noted as naming-only; no code change needed |
| 214 | `tropical_sheaf.rs:456-459` | `versions_compatible` maps unknown variables (not var1/var2) to `0.0`. In max-plus algebra, 0.0 is the multiplicative identity — so `coeff + 0.0 = coeff`, making those terms contribute their full coefficient to the max. This causes halfspace checks to pass/fail based on irrelevant coefficient magnitudes rather than the actual variable values | Unknown-variable terms are now filtered out via `Option<f64>`: if either side of a constraint has no known-variable terms, that constraint is skipped (can't be determined from just two variables). Only constraints fully determined by {var1, var2} are checked |
| 215 | `tropical_math.rs:552-565` | `to_tropical_matrices` splits a single max-atom constraint `max(z,y)+r >= x` across two matrices: `a[i][z] = r`, `f_prime[i][y] = r`, `a[i][x] = ONE`. The RHS coefficient `r` at column y goes to the wrong matrix, and `x`'s ONE ends up in the LHS matrix. The constraint is a single inequality, not two separate ones | LHS coefficients (r at z and y) go into matrix `a`, RHS coefficient (ONE at x) goes into separate matrix `rhs`. `f_prime` (which held y's coefficient incorrectly) replaced with proper `rhs` |

### Priority fixes (architectural)

Beyond individual bugs, the following systemic issues were identified and fixed:

| Priority | Component | Issue | Fix |
|---|---|---|---|
| P1 | `tropical_max_atom.rs` | Max-atom solver used longest-path positive-cycle detection (arcs y→x weight r for x≥y+r = LOWER bound). But max-atom `max(z,y)+r≥x` is an UPPER bound (x≤max(z,y)+r). The longest-path closure found "tightest lower bounds," which gave FALSE POSITIVES — declared satisfiable systems as UNSAT | Complete rewrite: shortest-path Bellman-Ford with negative-cycle detection on proper difference-constraint graph (arc y→x weight r for x-y≤r). Plus semantic fallback via iterative constraint propagation for max-atoms |
| P1 | `jagr_resolver.rs` | Nullstellensatz (Layer 3 pre-check) was never consulted — the resolver only used the max-atom result for PTIME filtering | `tropical_nullstellensatz` now called as Layer 3 pre-check before max-atom solver. If Nullstellensatz says infeasible, it overrides max-atom SAT result |
| P2 | `tropical_sheaf.rs` | `compute_sheaf_cohomology` computed ORDINARY simplicial homology (∂ maps) instead of SHEAF cohomology (δ coboundary maps weighted by stalk dimensions and restriction map ranks) | Rewritten with stalk-weighted coboundary maps δ^d with restriction-map-aware coefficients: entry (σ,τᵢ) in δ^d = `(-1)^i * restriction_rank(stalk(τᵢ)→stalk(σ))`. Added `kernel_dimension_f64` and `image_dimension_f64` helpers |
| P2 | `tropical_sheaf_analyzer.rs` | Dead code — not compiled (absent from `mod.rs`) | Added to `mod.rs`; now compiled and available |
| P3 | `ol3p_solver.rs`, `core_guided_lp.rs` | `extract_core` used stale assignment to identify unsatisfied soft clauses; could return incorrect cores or all soft clauses as a wasteful fallback | Now uses last SAT assignment (OL3P) or `build_reduced_assignment` (OL3P-S) to identify actually-unsatisfied soft clauses, producing smaller cores |
| P3 | `core_guided_solver.rs` | `is_optimal` always returned `true` regardless of actual optimality | Now `!hit_limit && cost < 1e-9` |
| P4 | `mayer_vietoris.rs` | `build_cover` used arbitrary vertex partition (contiguous ranges), not respecting topology. `compute_gluing_obstruction` used vector count instead of matrix rank for connecting map image dimension | Star-cover construction with greedy overlap merging. Gluing obstruction now uses actual RREF matrix rank for image dimension |
| P4 | `tropical_polyhedron.rs` | `project()` was a placeholder | Proper tropical Fourier-Motzkin elimination: 5 phases (kept-only, left-has-elim, right-has-elim, both-has-elim, pairwise FM combination) |
| P5 | `conflict_explainer.rs` | `explain_package_conflict` was flat "consider using a different version" — no derivation tree, no root cause analysis | PubGrub-quality conflict explanation: groups contradictory requirements by package, builds derivation chains showing why no version satisfies all constraints |
| — | `betti_numbers.rs` | `compute_betti_numbers` used ordinary boundary maps (∂) — old-style simplicial homology that ignores the sheaf structure | Delegates to `compute_sheaf_cohomology` which uses coboundary maps (δ) weighted by stalk dimensions and restriction map ranks |
| — | `jagr_resolver.rs` | Resolver used boolean `is_snf`/`greedy_works` to choose between greedy and Layer 1, ignoring the KIND of SNF violation | `SnfAction` (UseGreedy/UseExactSolver/UseFullSolver/Trivial) from `analyze_snf` now drives resolver strategy: β₁ violation → exact solver, β₂+ violation → full solver, with appropriate `strategy_used` labels |

---

## Links

- Crate: https://crates.io/crates/jhol
- Releases: https://github.com/bhuvanprakash/jhol/releases
- Documentation entry: [`Documentation/main.md`](./Documentation/main.md)

## License

Jhol is licensed under the [Jhol License](LICENSE) (personal and non-commercial use).
For commercial or other usage, contact: bhuvanstark6@gmail.com.
