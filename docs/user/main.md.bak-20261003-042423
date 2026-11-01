# Jhol Documentation

**Version:** 1.0.2  
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
- Optional polyglot planning across native JavaScript, Python/uv, and Rust/Cargo tools
- Project receipts, local snapshots, migration planning, and verified model-file pins

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

### Polyglot project preparation

Jhol keeps its native JavaScript install path and can coordinate Python and Rust
projects through uv and Cargo. `plan` only inspects files. `sync` records the
native lockfile digests; `sync --apply` runs detected adapters after checking
all requested capabilities.

```sh
jhol plan --path .
jhol sync --path .
jhol sync --path . --apply --offline --allow-build-scripts
```

`--offline` prevents adapter network fetches. Python build backends and Cargo
build scripts/procedural macros need the explicit `--allow-build-scripts` flag.
JavaScript lifecycle scripts stay disabled for this operation. The native
package managers remain responsible for their own lockfiles and resolution.

### Compare project receipts

`diff` compares two saved execution receipt JSON files. The current report
classifies input paths by whether their digests are unchanged, changed, or new;
it is not a replacement for Git diff or a complete graph-aware rebuild plan.

```sh
jhol diff before.receipt.json after.receipt.json
jhol diff before.receipt.json after.receipt.json --json
```

### Snapshot and restore project lock state

Snapshots live under `.jhol/snapshots`. Creating the first snapshot can also
write `jhol.project.lock` if the project does not already have one. Listing and
showing snapshots are read-only. Restore requires an explicit plan or apply;
apply writes only the project lock. Database restoration is unsupported and
fails closed.

```sh
jhol snapshot create --path .
jhol snapshot list --path .
jhol snapshot show SNAPSHOT_ID --path .
jhol restore SNAPSHOT_ID --plan --path .
jhol restore SNAPSHOT_ID --apply --path .
```

### Plan database changes and check readiness

`db plan` hashes `migrations/*.sql`; it does not connect to or change a database.
`db probe` reads `DATABASE_URL` and checks only whether its host and port accept
a TCP connection. It runs no SQL and never applies migrations.

```sh
jhol db plan --path .
jhol db probe --json
```

Set `DATABASE_URL` in the environment before the second command. Jhol reports
the host and port only; it does not print the credential fields.

### Pin and prepare model files

`ai pin` hashes local files. `ai pin-remote` records a public Hugging Face
repository, exact 40-character commit, and expected SHA-256 and size for each
file; it makes no network request. Supply one `--digest` and `--size-bytes` for
each `--file`, using values checked from that pinned revision. `ai prepare` verifies local files offline by
default. Add `--allow-network` to fetch missing files over HTTPS. Downloads
resume from partial files, obey `--max-bytes`, verify digest and size before
publishing, and return a JSON receipt with received-byte counts and removed
partial paths when using `--json`. `jhol ai infer` also runs the checked-in
Jhol linear-classifier fixture on a bounded CPU runtime. It validates pinned
config and weight bytes before inference and never executes downloaded model
code. This small numerical classifier does not claim transformer or LLM support.

```sh
jhol ai pin-remote --name demo --source hf://owner/repository \
  --revision 0123456789012345678901234567890123456789 \
  --file models/demo/config.json --digest DIGEST_FROM_PINNED_SOURCE \
  --size-bytes SIZE_FROM_PINNED_SOURCE --path .
jhol ai verify --path .
jhol ai prepare --path .
jhol ai prepare --allow-network --max-bytes 10737418240 --json --path .
jhol ai infer --path examples/blueprint \
  --manifest models/linear-demo.jhol-model.json \
  --config models/linear-demo/config.json --input=-2,1
```

### Inspect and start declared services

`dev` reads `[[service]]` entries from `jhol.toml`. By default it prints their
dependency order without starting processes. `--probe` checks declared TCP
readiness addresses. `--spawn` runs only service `run` argument arrays declared
in the project file; it does not invoke a shell or invent PostgreSQL/Uvicorn/Node
commands. Service working directories must exist inside the project root.

For example, a project can declare a local web server like this:

```toml
schema = 1

[[service]]
name = "web"
kind = "other"
run = ["python3", "-m", "http.server", "8000"]
cwd = "web"
ready_tcp = "127.0.0.1:8000"
```

`cwd` is relative to the project and must resolve to an existing directory
inside it. Add `needs = ["service-name"]` to start another service after that
service's readiness check passes. Keep credentials in environment variables,
not in command arguments or `jhol.toml`.

```sh
jhol dev --path examples/blueprint
jhol dev --path examples/blueprint --probe
jhol dev --path examples/blueprint --spawn --hold-secs 2
```

`--spawn` starts local processes and is an explicit side effect. Jhol waits for
declared readiness checks and cleans up the child processes when it exits; this
is a development helper, not a service supervisor or process-tree sandbox.

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
