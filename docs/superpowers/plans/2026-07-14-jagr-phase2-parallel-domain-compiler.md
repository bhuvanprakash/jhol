# JAGR Phase 2 Parallel Domain Compiler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove JAGR's sequential metadata waterfall and avoid exploring versions that cannot satisfy any reachable incoming range, without excluding any potentially valid resolution.

**Architecture:** Fetch each BFS wave's full packuments concurrently so every candidate retains authoritative dependencies. Track the union of incoming ranges per package; scan only versions satisfying at least one reachable range, and rescan cached metadata when a newly discovered range expands that union.

**Tech Stack:** Rust 2021, Rayon, existing registry cache and semver helpers.

---

### Task 1: Range-directed candidate selection

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`
- Test: `crates/jhol-core/src/lockfile_write.rs`

- [ ] Write a failing test proving the union of `^1` and `^3` keeps matching 1.x/3.x candidates and excludes 2.x.
- [ ] Run the exact test and confirm the expected failure.
- [ ] Implement `candidate_versions_for_incoming_specs` using existing npm-range semantics.
- [ ] Run the exact test and confirm it passes.

### Task 2: Parallel authoritative packuments

**Files:**
- Modify: `crates/jhol-core/src/registry.rs`
- Modify: `crates/jhol-core/src/lockfile_write.rs`

- [ ] Add `parallel_fetch_metadata_full`, reusing caller-local caching and Rayon while always returning authoritative full version metadata.
- [ ] Replace the per-result abbreviated/full sequential waterfall in JAGR BFS with one bounded parallel full-packument wave.
- [ ] Keep errors for required metadata and allow optional root metadata misses.

### Task 3: Incremental range frontier

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`
- Test: `crates/jhol-core/src/lockfile_write.rs`

- [ ] Track incoming ranges and processed ranges per package.
- [ ] Scan dependencies, optional dependencies, and non-optional peers only for range-reachable candidate versions.
- [ ] Requeue an already fetched package when a new incoming range is discovered; reuse its cached packument instead of refetching.
- [ ] Feed the same incoming-range union into request-domain construction.

### Task 4: Verification and benchmark

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`
- Modify: `crates/jhol-core/src/registry.rs`

- [ ] Format only touched files with direct `rustfmt`.
- [ ] Run `cargo test -p jhol-core jagr --lib`.
- [ ] Run `cargo test -p jhol-core --lib`.
- [ ] Build release and compare preserved before binary, Phase 1 binary, Phase 2 binary, and Bun on identical fixtures.
- [ ] Reject Phase 2 if the Phase 1 certificate finds missing or incompatible dependencies.
