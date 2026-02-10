# Jhol Roadmap: Better, Faster, More Reliable — and Beyond

This document adds **out-of-the-box** ideas so Jhol can eventually **replace npm, yarn, bun, and pnpm** or stand as a first-class alternative.

---

## What’s Already in Good Shape

- **Real cache**: Tarballs stored via `npm pack`, install from cache with `npm install <path>.tgz`
- **Windows cache dir**: `USERPROFILE` on Windows, `JHOL_CACHE_DIR` override
- **Global install**: Only on `jhol global-install` (no auto-install on every run)
- **Doctor**: Uses `npm outdated --json` for real outdated detection
- **Timeouts**: `npm show` and `npm install` run with timeouts
- **Subcommands**: `cache list`, `cache clean`, `--no-cache`, `-q` / `--quiet`
- **Config**: `JHOL_CACHE_DIR`, `JHOL_LOG=quiet`
- **SLSA workflow**: Builds `target/release/jhol` and generates provenance

---

## Quick Wins (Reliability & Polish)

| Item | What | Why |
|------|------|-----|
| **Quiet flag in logs** | When user passes `-q` / `--quiet`, pass it into `utils::log()` (or a global/thread-local “quiet” flag) so log lines don’t still print to stdout | Right now only subcommand printlns are suppressed; logs still appear |
| **Exit codes** | Make `install_package()` return `Result<(), String>` and in `main` exit with 1 when install fails (or when any package fails) | Scripts and CI need reliable success/failure |
| **`jhol install` with no args** | In a directory with `package.json`, run the equivalent of `npm install` (install deps from package.json + lockfile) | Matches user expectation and makes “replace npm” more realistic |
| **Doc typo** | Fix “nstallation” → “Installation” in Documentation/main.md if still present | Small trust/polish |
| **Tokio** | Either use it (e.g. parallel installs, async HTTP later) or remove from `Cargo.toml` to keep deps honest | Clean dependency story |

---

## Speed

1. **Parallel validation**  
   For `jhol install pkg1 pkg2 ... pkgN`, run `npm show` for each package in parallel (e.g. thread pool or tokio, cap concurrency 4–8). Same for “is it cached?” checks.

2. **Parallel fetch + cache**  
   When multiple packages need to be fetched, run multiple `npm install <pkg>` (or later, native registry fetches) in parallel with a concurrency limit.

3. **Use Tokio for I/O**  
   If you keep Tokio: use `tokio::process::Command` and async file/network so that one slow npm or network call doesn’t block everything.

4. **Native registry client (big lever)**  
   Replace `npm show` / `npm install` / `npm pack` with direct HTTP to the registry:
   - `GET https://registry.npmjs.org/<pkg>` for metadata
   - Download tarball from `dist.tarball`
   - Extract to `node_modules/<pkg>` (and handle dependency tree)
   That removes subprocess overhead and allows parallel downloads in one process, making Jhol **much** faster and **independent of npm** for installs.

---

## Reliability

1. **Retries with backoff**  
   You already retry install 3 times; add exponential backoff (e.g. 2s, 4s, 8s) and optional `--retries N`.

2. **Stricter timeouts**  
   Make timeout configurable (e.g. `JHOL_INSTALL_TIMEOUT`, `JHOL_SHOW_TIMEOUT`) for slow networks or big installs.

3. **Integrity**  
   - Verify tarball integrity (e.g. compare with `dist.shasum` or `dist.integrity` from registry).
   - Optionally verify lockfile (e.g. `package-lock.json`) so installs are deterministic and tamper-evident.

4. **Clear errors**  
   Return `Result` from core functions; in `main`, print one clear message and set exit code so users and scripts know what failed.

---

## To Replace npm / Yarn / Bun / pnpm Entirely

To be a **standalone** package manager that doesn’t depend on npm at all:

### 1. Native registry client (core)

- **HTTP client**: Use `reqwest` (or `ureq`) to talk to `registry.npmjs.org` (or `npm config get registry` later).
- **Metadata**: `GET /<package>` (and `/@scope%2Fpkg` for scoped). Parse JSON for `versions`, `dist-tags`, `version['dist'].tarball`.
- **Download**: Stream tarball into a file or memory; verify `integrity`/`shasum` if present.
- **No Node required for install**: Only Node is needed if you want to run `npm run` / lifecycle scripts; install itself can be 100% Rust.

### 2. Native node_modules layout

- **Resolution**: Flatten dependency tree (or use a hoisting algorithm like npm’s). Resolve versions from package.json + lockfile (semver).
- **Extract**: Unpack tarballs into `node_modules/<name>` (and handle scoped `node_modules/@scope/pkg`). Use `tar` crate (e.g. `tar::Archive`) for .tgz.
- **Symlinks (optional)**: Like pnpm, use a content-addressable store and symlink into `node_modules` for speed and disk savings.

### 3. Lockfile

- **Read/write package-lock.json** (v2/v3) so installs are deterministic and you don’t need npm to generate it.
- Or define a **Jhol lockfile** (e.g. `jhol.lock`) and document migration from lockfile ↔ npm.

### 4. Full CLI parity (over time)

- `jhol install` (with no args) → install from package.json (and lockfile).
- `jhol uninstall <pkg>` → remove from package.json and node_modules.
- `jhol run <script>` → run scripts from package.json (spawn `npm run` or, later, a small script runner).
- `jhol init`, `jhol publish` (optional), workspace/monorepo support.

### 5. Content-addressable cache (like bun/pnpm)

- Cache key = hash of tarball (or integrity string). Same tarball = one copy on disk; symlink or copy into projects. Saves space and speeds installs.

### 6. Optional: Plug’n’Play (PnP) or virtual store

- Like Yarn PnP or pnpm’s store: one global store, `node_modules` as symlinks or a loader. Bigger change but differentiator for speed and disk usage.

---

## Out-of-the-Box Ideas

| Idea | What | Benefit |
|------|------|--------|
| **Zero Node for install** | Native registry + extract; no `npm` subprocess for install | Works on minimal envs; no Node needed to *install* deps |
| **Offline-first** | Prefer cache always; only hit network when cache miss | Fast and works in air-gapped / flaky networks |
| **Integrity by default** | Verify every tarball with integrity/shasum from registry | Security and supply-chain trust |
| **Monorepo / workspaces** | Support multiple packages in one repo (e.g. npm workspaces) | One tool for small and large projects |
| **Decentralized / P2P (later)** | Optional backend: IPFS or other DHT for package fetch | Censorship resistance; mirrors; research appeal |
| **Binary artifacts** | Download platform-specific binaries (e.g. optionalDependencies, node-pre-gyp) from registry or CDN | Full compatibility with native addons |
| **Structured output** | `jhol install --json` for machine-readable progress and result | CI, IDEs, and UIs can integrate easily |
| **Config file** | `~/.jholrc` or `.jholrc` for registry, timeout, concurrency, cache dir | Power users and enterprises |
| **Audit / SBOM** | `jhol audit` (or use existing tools on lockfile) and export SBOM | Security and compliance |

---

## Suggested Order of Work

1. **Polish**: Quiet → log suppression, exit codes, `jhol install` (no args) from package.json.
2. **Speed**: Parallel `npm show` and parallel fetch (still using npm); then introduce native registry client.
3. **Independence**: Native registry client + tarball download + extract to node_modules (no npm for install).
4. **Correctness**: Lockfile read/write, integrity verification, deterministic installs.
5. **Parity**: Uninstall, run scripts, init; then workspaces and content-addressable store.

This order gets you to “better and faster” quickly, then “replaces npm” step by step without a single big rewrite.

---

## One-Line Vision

**Short term:** Fast, reliable, npm-compatible manager with real cache and good DX.  
**Long term:** A standalone, Node-optional package manager with native registry client, lockfile, integrity, and optional PnP or content-addressable store — so you can replace npm/yarn/bun/pnpm where it matters.
