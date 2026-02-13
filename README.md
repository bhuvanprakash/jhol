# Jhol

A fast, offline-friendly package manager that plays nice with your existing `package.json`. It caches everything it can and runs **without Node, Bun, or npm** for install, doctor, and audit by default.

---

## Why use Jhol?

- **No Node/Bun/npm required** – Install, lockfile-only, doctor, and audit use the npm registry and OSV API directly. Use `--fallback-backend` to fall back to Bun/npm if needed.
- **Fast installs** – Once a package is in the cache, repeat installs skip the network. Same lockfile? You’re basically done.
- **Offline-friendly** – No internet? No problem. If it’s cached, you can install it.
- **Fallback optional** – Pass `--fallback-backend` to use Bun or npm when native install fails.
- **Doctor** – `jhol doctor` shows what’s outdated; `jhol doctor --fix` updates those packages.
- **Audit & SBOM** – `jhol audit` checks for known vulnerabilities; `jhol sbom` spits out a software bill of materials for your project.
- **Workspaces** – Use `--all-workspaces` and it’ll run install, doctor, or audit across all your workspace packages in one go.

---

## Installation

**From crates.io:**
```sh
cargo install jhol
```

**From source:**
```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

**Prebuilt binaries (Linux & Windows):**  
Grab `jhol-linux` or `jhol-windows.exe` from [Releases](https://github.com/bhuvanprakash/jhol/releases).  
On Linux: `chmod +x jhol-linux` and optionally move it into your PATH.  
On Windows: run it or add its folder to your PATH.

**Put `jhol` on your PATH (any install method):**
```sh
jhol global-install
```

---

## Quick start

```sh
jhol install lodash              # Install one package (cache when possible)
jhol install react react-dom     # Install several at once
jhol install                    # No args = install from package.json (and lockfile)
jhol doctor                     # See what’s outdated
jhol doctor --fix               # Update those packages
jhol audit                      # Check for vulnerabilities
jhol audit --fix                # Try to fix them
```

---

## Commands (the important ones)

| What you want | Command |
|---------------|--------|
| Install packages | `jhol install <pkg> [pkgs...]` or just `jhol install` (from package.json) |
| Ignore cache and fetch fresh | `jhol install --no-cache <pkg>` |
| Only update the lockfile | `jhol install --lockfile-only` |
| Offline only (fail if not cached) | `jhol install --offline` or set `JHOL_OFFLINE=1` |
| Strict lockfile (fail if out of sync) | `jhol install --frozen` |
| Use Bun/npm when native fails | `jhol install --fallback-backend` |
| Check outdated deps | `jhol doctor` |
| Update outdated deps | `jhol doctor --fix` |
| Run in all workspaces | `jhol install --all-workspaces`, `jhol doctor --all-workspaces`, `jhol audit --all-workspaces` |
| Security audit | `jhol audit` / `jhol audit --fix` |
| Generate SBOM | `jhol sbom` or `jhol sbom -o sbom.json` |
| List cache | `jhol cache list` |
| Cache size | `jhol cache size` |
| Prune old cache | `jhol cache prune` or `jhol cache prune --keep 50` |
| Export deps for offline | `jhol cache export ./my-bundle` |
| Import from bundle | `jhol cache import ./my-bundle` |
| Wipe cache | `jhol cache clean` |
| Lockfile hash (for CI cache key) | `jhol cache key` |
| Prefetch deps into cache (no node_modules) | `jhol prefetch` then `jhol install --offline` |
| Install the binary to PATH | `jhol global-install` |

Use `-q` or `--quiet` when you want less noise. Use `--json` on install, doctor, or audit if you need machine-readable output.

---

## Configuration

| Env / file | What it does |
|------------|--------------|
| `JHOL_CACHE_DIR` | Where to put the cache (default: `~/.jhol-cache` on Unix, `%USERPROFILE%\.jhol-cache` on Windows) |
| `JHOL_LOG=quiet` or `-q` | Less logging |
| `JHOL_OFFLINE=1` or `--offline` | Only use cache; fail if something isn’t there |
| `.jholrc` (JSON in project or home) | Optional: set `backend` (`"bun"` or `"npm"`), `cacheDir`, `offline`, `frozen` so you don’t have to pass flags every time |

**CI tip:** Run `jhol cache key` to get a hash of your lockfile (`bun.lock` or `package-lock.json`). Same lockfile → same key. Use that as your CI cache key so you can reuse the Jhol store between runs.

### Deterministic installs (CI)

With a lockfile and `jhol install --frozen`, Jhol does **no resolution** and **no packument** requests: it only downloads missing tarballs (from lockfile URLs) and links or extracts from the store. Recommended for CI. Use `jhol cache key` as your cache key so the same lockfile reuses the same store.

---

## How it fits together

The repo is a Cargo workspace: the **jhol** binary lives at the root and talks to **jhol-core** in `crates/jhol-core`. The core does the real work (cache, install, doctor, registry, lockfile, audit, workspaces); the CLI just parses args and calls in. You can depend on `jhol-core` from other tools (e.g. a script or a future LSP) without pulling in the CLI.

---

## Performance benchmarking

Jhol includes a simple benchmark harness at `scripts/benchmark.py` to measure install performance.

### What it measures
- `jhol_cold_install`: empty cache + install
- `jhol_warm_install`: cached install
- `jhol_offline_install`: cached install in `--offline` mode
- Optional: `npm_cold_install` / `npm_warm_install` with `--compare-npm`

### Run it
```sh
cargo build --release
python3 scripts/benchmark.py --repeats 3 --json-out benchmark-results.json
```

Optional npm comparison:
```sh
python3 scripts/benchmark.py --repeats 3 --compare-npm --json-out benchmark-results.json
```

Tip: use exact versions in `--packages` for stable and repeatable results.

### Regression check against baseline
Use the baseline in `benchmarks/baseline.json` and fail if a metric regresses beyond threshold:

```sh
python3 scripts/check_benchmark_regression.py \
  --baseline benchmarks/baseline.json \
  --results benchmark-results.json \
  --threshold 0.25
```

`--threshold 0.25` means up to 25% slowdown is allowed before failing.

---

## Compatibility & current limitations

### What is stable today
- Native install flow with npm registry metadata + tarball extraction
- Cache-first and offline workflows (`prefetch`, `install --offline`)
- Lockfile-aware deterministic installs (`--frozen`)
- Workspace traversal for install/doctor/audit/run

### Known limitations (to improve next)
- Dependency resolution is currently a greedy strategy (single version preference) and may differ from npm behavior on complex trees.
- Some advanced npm ecosystem edge cases (complex peer dependency graphs, rare postinstall assumptions) are not fully parity-tested yet.
- Benchmarking is available and automated in CI with a threshold gate, but baseline tuning per environment/project profile is still evolving.

If you hit an issue, please open one with the failing package graph and lockfile for fastest debugging.

---

## Links

- **Crate:** [crates.io/crates/jhol](https://crates.io/crates/jhol)
- **Releases:** [GitHub Releases](https://github.com/bhuvanprakash/jhol/releases)

## License

Jhol is licensed under the [Jhol License](LICENSE) (personal and non-commercial use). For other use, contact bhuvanstark6@gmail.com.
