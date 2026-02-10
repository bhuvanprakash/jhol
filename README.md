# Jhol

A fast, offline-friendly package manager that plays nice with your existing `package.json`. It caches everything it can and uses **Bun** (or npm) under the hood so you get quick installs and the option to work offline.

---

## Why use Jhol?

- **Fast installs** – Once a package is in the cache, repeat installs skip the network. Same lockfile? You’re basically done.
- **Offline-friendly** – No internet? No problem. If it’s cached, you can install it.
- **Bun first, npm fallback** – Prefers Bun when it’s installed (it’s quick and well maintained). No Bun? It uses npm. You can force either with `--backend bun` or `--backend npm`.
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

---

## How it fits together

The repo is a Cargo workspace: the **jhol** binary lives at the root and talks to **jhol-core** in `crates/jhol-core`. The core does the real work (cache, install, doctor, registry, lockfile, audit, workspaces); the CLI just parses args and calls in. You can depend on `jhol-core` from other tools (e.g. a script or a future LSP) without pulling in the CLI.

---

## Links

- **Crate:** [crates.io/crates/jhol](https://crates.io/crates/jhol)
- **Releases:** [GitHub Releases](https://github.com/bhuvanprakash/jhol/releases)

## License

Jhol is licensed under the [Jhol License](LICENSE) (personal and non-commercial use). For other use, contact bhuvanstark6@gmail.com.
