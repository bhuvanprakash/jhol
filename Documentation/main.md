# Jhol Documentation

**Version:** 1.0.1  
**Author:** Bhuvan Prakash  
**License:** Jhol License

---

## Table of contents

1. [Introduction](#introduction)
2. [Installation and setup](#installation-and-setup)
3. [Basic usage](#basic-usage)
4. [Advanced usage](#advanced-usage)
5. [Configuration](#configuration)
6. [Cache and package management](#cache-and-package-management)
7. [Troubleshooting](#troubleshooting)
8. [Contributing](#contributing)
9. [Security](#security)
10. [FAQ](#faq)

---

## Introduction

Jhol is a cache-first, offline-friendly package manager for JavaScript projects. It works with your existing `package.json` and lockfiles while keeping installs predictable and fast.

By default, Jhol supports native install, doctor, and audit behavior. For compatibility fallback, use `--fallback-backend` to delegate to Bun or npm.

### Highlights

- Native-first install, lockfile, doctor, and audit workflows
- Cache-first design for faster repeat installs
- Offline mode (`--offline`) and deterministic mode (`--frozen` / `ci`)
- Workspace-aware execution (`--all-workspaces`)
- Built-in security audit and SBOM generation

---

## Installation and setup

### Prerequisites

- Rust/Cargo from [rustup.rs](https://rustup.rs)
- Git (if building from source)

### Build from source

```sh
git clone https://github.com/bhuvanprakash/jhol.git
cd jhol
cargo build --release
./target/release/jhol --help
```

### Install globally

```sh
./target/release/jhol global-install
```

Or install directly via Cargo:

```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

---

## Basic usage

### Install dependencies

```sh
jhol install
jhol install lodash
jhol install axios express react
```

### Check and update outdated packages

```sh
jhol doctor
jhol doctor --fix
```

### Audit vulnerabilities

```sh
jhol audit
jhol audit --fix
jhol audit --gate
```

### Generate SBOM

```sh
jhol sbom
jhol sbom -o sbom.json
```

---

## Advanced usage

### Offline installs

```sh
jhol install --offline
```

### Strict lockfile / CI mode

```sh
jhol install --frozen
jhol ci
```

### Lockfile-only updates

```sh
jhol install --lockfile-only
```

### Workspaces

```sh
jhol install --all-workspaces
jhol doctor --fix --all-workspaces
jhol audit --all-workspaces
```

### Prefetch and air-gapped flow

```sh
jhol prefetch
jhol cache export ./bundle
jhol cache import ./bundle
jhol install --offline
```

---

## Configuration

| Env / file | Effect |
|---|---|
| `JHOL_CACHE_DIR` | Override cache directory |
| `JHOL_LOG=quiet` | Reduce log output |
| `JHOL_OFFLINE=1` | Enable offline mode |
| `JHOL_NETWORK_CONCURRENCY` | Max concurrent HTTP requests |
| `JHOL_LINK=0` | Copy from store instead of linking |
| `.jholrc` (JSON) | Default values for backend/cache/offline/frozen |

Example `.jholrc`:

```json
{
  "backend": "bun",
  "cacheDir": "/tmp/jhol-cache",
  "offline": false,
  "frozen": false
}
```

---

## Cache and package management

```sh
jhol cache list
jhol cache size
jhol cache prune
jhol cache prune --keep 50
jhol cache export ./bundle
jhol cache import ./bundle
jhol cache clean
jhol cache key
```

---

## Troubleshooting

| Problem | Suggested fix |
|---|---|
| `jhol: command not found` | Build release binary and run `jhol global-install` |
| Offline install fails | Import/export cache bundle, then retry with `--offline` |
| Install failures | Retry with `--no-cache`, verify network, verify lockfile |
| Slow/hanging behavior | Check cache logs and network conditions |

---

## Contributing

1. Fork the repository
2. Create branch
3. Implement and test changes
4. Open a pull request with a clear description

Run tests before opening PRs:

```sh
cargo test
```

---

## Security

- Use `jhol audit` regularly
- Use `jhol audit --gate` in CI
- For supply-chain reporting, generate and track SBOM outputs

---

## FAQ

**How is Jhol different from npm/yarn?**  
Jhol is cache-first and optimized for offline/deterministic workflows.

**Do I need Bun or npm installed?**  
Not for core native flows. Use fallback backend only when needed.

**Can I use Jhol in CI?**  
Yes. Prefer `jhol ci` or `jhol install --frozen` and use `jhol cache key` for cache reuse.
