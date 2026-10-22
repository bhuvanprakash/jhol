# Scope and Stability

Jhol is a **focused** package manager. This document states what is in scope, what is out of scope, and our stability priorities.

## In scope

- **Install**: From `package.json` and from lockfiles (`.jhol.lock`, `package-lock.json`, `bun.lock`). Native resolution and store; optional fallback to Bun/npm.
- **Resolve**: Dependency resolution through `jagr3` with `pubgrub-v2` fallback, plus lock generation.
- **Lockfile**: Read and write `.jhol.lock` and `package-lock.json`; read `bun.lock`; import/convert compatibility lockfile formats.
- **Run**: Execute scripts from `package.json` (`jhol run <script>`).
- **Exec**: Run binaries from `node_modules/.bin` (`jhol exec` / `jhol x`).
- **Prefetch**: Populate the cache from a lockfile without installing to `node_modules`.
- **Doctor**: Check and fix outdated dependencies.
- **Audit / SBOM**: Security audit and software bill of materials.
- **Cache**: List, prune, export, import, and manage the content-addressable store.
- **Configuration**: Timeouts, proxy, registry, resolver strategy via env and `.npmrc`/`.jholrc`.

## Out of scope (current)

- **Bundler / runtime**: No built-in Markdown runtime, no general-purpose JS/TS bundler or VM.
- **Language features**: Only what is needed to run scripts and binaries (e.g. `PATH`, `node_modules/.bin`).
- **Monorepo task orchestration**: No built-in equivalent of Turborepo/Nx; workspaces are supported for install/doctor.

## Stability priorities

- **No silent hangs**: All network and resolution paths use configurable timeouts; failures produce clear errors.
- **Clear errors**: Resolution failures surface derivation trees or concise conflict messages; timeout and proxy issues are explicit.
- **Determinism**: Same inputs and registry state produce the same resolution and lockfile.
- **No regressions**: New features and optimizations must not break existing workflows; tests and regression suites guard Bun/npm pain points (timeout, proxy, peer loop, lockfile).

New features should not regress reliability. When in doubt, we prefer stability and clarity over extra functionality.
