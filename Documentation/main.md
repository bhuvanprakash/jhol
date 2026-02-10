# Jhol – A faster, offline-friendly package manager

**Version:** 1.0.0  
**Author:** Bhuvan Prakash  
**License:** Jhol License (see repo)

---

## Table of contents

1. [Introduction](#introduction)
2. [Installation and setup](#installation-and-setup)
3. [Basic usage](#basic-usage)
4. [Advanced features](#advanced-features)
5. [Configuration](#configuration)
6. [Package and cache management](#package-and-cache-management)
7. [Troubleshooting](#troubleshooting)
8. [Contributing](#contributing)
9. [Security](#security)
10. [FAQs](#faqs)

---

## Introduction

Jhol is a package manager that sits on top of your existing Node/JS workflow. It’s built to be **fast** (cache-first) and **offline-friendly**, and it uses **Bun** when you have it, otherwise **npm**. You keep using the same `package.json` and lockfiles; Jhol just tries to make installs quicker and to let you work without a network when possible.

### What you get

- **Local caching** – Tarballs are stored so the next install of the same thing can skip the registry.
- **Bun or npm** – Prefers Bun if it’s on your PATH; otherwise npm. You can force one with `--backend bun` or `--backend npm`.
- **Doctor** – Figures out what’s outdated and can update it with `jhol doctor --fix`.
- **Audit** – Runs a security check (`jhol audit`) and can try to fix issues (`jhol audit --fix`).
- **SBOM** – Generates a software bill of materials for your project.
- **Workspaces** – `--all-workspaces` runs install, doctor, or audit across all workspace packages.
- **Offline and strict lockfile** – `--offline` and `--frozen` for controlled, reproducible installs.

---

## Installation and setup

### What you need

- **Rust and Cargo** – Jhol is written in Rust. Install from [rustup.rs](https://rustup.rs).
- **Node and a backend** – You need either **Bun** or **Node/npm** so Jhol can install packages. Bun is preferred if it’s installed.
- **Git** – Only if you’re building from source.

### Build and run

1. Clone the repo:
   ```sh
   git clone https://github.com/bhuvanprakash/jhol.git
   cd jhol
   ```
2. Build:
   ```sh
   cargo build --release
   ```
3. Run:
   ```sh
   ./target/release/jhol --help
   ```

To have `jhol` available everywhere:

```sh
./target/release/jhol global-install
```

Or install via Cargo from the repo:

```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

---

## Basic usage

### Installing packages

```sh
jhol install lodash
jhol install axios express react
```

With no arguments, Jhol installs from your `package.json` and lockfile:

```sh
jhol install
```

It checks the cache first. If the package is there, it installs from there. If not, it fetches via Bun or npm and then caches it.

### Checking and fixing dependencies

```sh
jhol doctor
```

Lists outdated dependencies.

```sh
jhol doctor --fix
```

Updates them (e.g. to latest within your ranges or as per the backend).

### Viewing the cache

```sh
jhol cache list
jhol cache size
```

Logs live in `~/.jhol-cache/logs.txt` (or your `JHOL_CACHE_DIR`).

### Clearing the cache

```sh
jhol cache clean
```

Or remove the directory yourself: `rm -rf ~/.jhol-cache` (Unix) or the equivalent on Windows.

---

## Advanced features

### Specific versions

```sh
jhol install react@18.0.0
jhol install lodash@4.17.21 react@17.0.0
```

Jhol caches each version separately.

### Offline mode

Use `--offline` (or `JHOL_OFFLINE=1`). Jhol will *only* use the cache. If something isn’t cached, it fails and tells you what’s missing. Handy when you’re offline or in a locked-down environment.

```sh
jhol install --offline
```

### Strict lockfile (frozen)

Use `--frozen` when you want the lockfile to be the source of truth. If there’s no lockfile or if `package.json` and the lockfile don’t match, Jhol fails instead of updating the lockfile.

```sh
jhol install --frozen
```

### Lockfile only

To only update the lockfile (no `node_modules`):

```sh
jhol install --lockfile-only
```

### Security audit

```sh
jhol audit
```

Shows known vulnerabilities. To try to fix them:

```sh
jhol audit --fix
```

Use `--json` if you want raw JSON output.

### SBOM (software bill of materials)

```sh
jhol sbom
jhol sbom --format simple -o sbom.json
```

Generates a bill of materials (CycloneDX by default) for your dependencies.

### Workspaces

If your root `package.json` has a `workspaces` field, you can run commands across all workspace packages:

```sh
jhol install --all-workspaces
jhol doctor --fix --all-workspaces
jhol audit --all-workspaces
```

### Cache export and import

Export everything your project needs into a folder (e.g. for another machine or offline):

```sh
jhol cache export ./jhol-bundle
```

On the other machine (or later):

```sh
jhol cache import ./jhol-bundle
```

Then `jhol install --offline` can use that cache.

### Prune and CI cache key

- **Prune** – Remove tarballs that aren’t in the index, or keep only the N most recent:
  ```sh
  jhol cache prune
  jhol cache prune --keep 50
  ```
- **CI cache key** – `jhol cache key` prints a hash of your lockfile. Use it as the cache key in CI so the same lockfile reuses the same cache.

---

## Configuration

Jhol uses a cache directory (default: `~/.jhol-cache` on Unix, `%USERPROFILE%\.jhol-cache` on Windows).

| Env / file | Effect |
|------------|--------|
| `JHOL_CACHE_DIR` | Override the cache directory |
| `JHOL_LOG=quiet` | Less log output |
| `JHOL_OFFLINE=1` | Behave like `--offline` |
| `.jholrc` (JSON) | Optional: `backend`, `cacheDir`, `offline`, `frozen` |

Example `.jholrc` in project root or home:

```json
{
  "backend": "bun",
  "cacheDir": "/tmp/my-jhol-cache",
  "offline": false,
  "frozen": false
}
```

---

## Package and cache management

- **List what’s cached:** `jhol cache list`
- **See cache size:** `jhol cache size`
- **Remove everything:** `jhol cache clean`
- **Uninstalling a package:** Jhol doesn’t have an `uninstall` command. Remove it from `package.json` and run `jhol install` again, or edit the lockfile and reinstall. The cache can stay; it’s keyed by name and version.

---

## Troubleshooting

| Problem | What to try |
|--------|-------------|
| `jhol: command not found` | Run `cargo build --release` in the repo, or run `jhol global-install` so the binary is on your PATH. |
| Permission denied on cache dir | Use `sudo` only if you really need to (e.g. `sudo rm -rf ~/.jhol-cache`). Prefer keeping the cache in your home dir. |
| Failed to install package | Make sure Bun or npm is installed (`bun --version` or `npm --version`). Check the network. Try `--no-cache` once to rule out a bad cache. |
| Jhol seems to hang | It might be waiting on the registry or a slow network. Check `~/.jhol-cache/logs.txt`. You can set timeouts via the backend (Bun/npm) if needed. |
| Offline install fails | Run `jhol cache export ./bundle` where you have network, then `jhol cache import ./bundle` and `jhol install --offline` where you don’t. |

---

## Contributing

1. Fork the repo on GitHub.
2. Clone it, make your changes, and test (e.g. `cargo test`, run a few commands by hand).
3. Open a pull request.

A few guidelines: follow normal Rust style, handle errors clearly, and keep the code readable. If you’re adding a feature, a quick note in the PR (or in docs) helps.

---

## Security

- Jhol does not verify package signatures itself; it relies on the registry and the backend (Bun/npm).
- The cache is just files on disk; if someone can write to it, they could tamper with it. Use `--offline` only in environments where the cache is trusted (e.g. you built it yourself with `cache export`).
- Use `jhol audit` and `jhol audit --fix` to stay on top of known vulnerabilities. For stricter supply-chain needs, use the SBOM and your own tooling.

---

## FAQs

**How is Jhol different from npm or Yarn?**  
Jhol is cache-first and can work offline. It also prefers Bun when available and gives you doctor, audit, SBOM, and workspace-wide commands in one place.

**Do I need Bun?**  
No. If Bun isn’t installed, Jhol uses npm. You can force npm with `--backend npm` or in `.jholrc`.

**What if a package isn’t in the cache?**  
Jhol fetches it from the registry (via Bun or npm) and then caches it. Next time it’s cached.

**How do I update dependencies?**  
Run `jhol doctor --fix`. That updates packages that are outdated according to the backend.

**Can I use Jhol in CI?**  
Yes. Use `jhol cache key` as your cache key (same lockfile ⇒ same key), restore the Jhol cache, then run `jhol install --frozen` (or without `--frozen` if you want the lockfile updated). Use `--json` if you need machine-readable output.

**What about global install?**  
`jhol global-install` copies the `jhol` binary to a standard location (e.g. `/usr/local/bin` or your user dir on Windows) so you can run `jhol` from any directory.

---

## Summary

| Feature | Status |
|---------|--------|
| Local caching | Yes |
| Bun / npm backend | Yes |
| Offline mode | Yes |
| Doctor (outdated + fix) | Yes |
| Audit (+ fix) | Yes |
| SBOM | Yes |
| Workspaces (--all-workspaces) | Yes |
| Cache export/import | Yes |
| Global install (binary to PATH) | Yes |
| Config file (.jholrc) | Yes |

Jhol is a single CLI that tries to make installs fast, support offline workflows, and give you doctor, audit, and SBOM without switching tools. If you hit something that doesn’t match this doc, open an issue on GitHub and we can fix the docs or the behavior.
