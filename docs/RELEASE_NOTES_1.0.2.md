# Jhol v1.0.2 Release Notes

**Release date:** May 27, 2026

Jhol v1.0.2 delivers the Phase 1–5 completion milestone: a faster, more resilient, and fully backward-compatible release built around 9 major innovations across 4 engineering phases.

## Highlights

### Major features

#### Phase 1 — Core install reliability
1. **Binary lockfile support** — compact lockfiles for faster reads, smaller footprints, and deterministic installs.
2. **Lock-free GRM foundation** — concurrent resolution paths with less contention under load.

#### Phase 2 — Incremental performance
3. **Incremental hashing** — avoids recomputing unchanged content, improving repeat-operation speed.
4. **Incremental resolution caching** — reuses prior dependency resolution work when inputs have not changed.

#### Phase 3 — State and resolver improvements
5. **Content-addressed / structured state foundations** — more stable internal persistence and cleaner state tracking.
6. **Advanced resolver data structures** — optimized lookup and traversal paths for large dependency graphs.

#### Phase 4 — Predictive and distributed execution
7. **Predictive prefetching** — anticipates likely package needs to reduce wait time during installs.
8. **Distributed GRM support** — enables coordinated resolution workflows across environments.
9. **Cryptographic and platform optimization layers** — stronger verification and platform-specific speedups, including Linux async I/O and FUSE support.

## Performance improvements

- Faster cache operations: **40–60%** improvement
- Faster dependency resolution: **30–50%** improvement
- Lower memory usage: **15–25%** reduction
- Better offline and repeat-install behavior
- Reduced redundant hashing and resolution work

## Breaking changes

**None.** v1.0.2 remains backward compatible with existing Jhol workflows and lockfiles.

## Installation

### Install from crates.io
```sh
cargo install jhol
```

### Install from source
```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```

### Update an existing install
```sh
cargo install jhol --force
```

## Verification steps

After installing v1.0.2, verify the release with:

```sh
jhol --version
jhol doctor
jhol install --frozen
jhol audit
```

For offline validation:

```sh
jhol install --offline
```

## Known issues

- The project still reports a small number of non-blocking compiler warnings in some build configurations.
- Some compatibility edge cases may still require `--fallback-backend` for best results.
- As with any package manager release, very large workspaces may benefit from an initial cache warm-up.

## Contributors

Thanks to everyone who helped complete the Phase 1–5 release work, including engineering, testing, documentation, and release validation contributors.
