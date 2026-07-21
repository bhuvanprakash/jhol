# JAGR Phase 1 Certified Constraints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make JAGR preserve every original dependency range, model the project as a synthetic root, reject incomplete required metadata, and certify the final assignment before materialization.

**Architecture:** Extract request construction and assignment verification into small pure helpers in `lockfile_write.rs`, so correctness can be tested without registry I/O. The synthetic root is a resolver-only node, while the final certificate validates the selected real packages against the original semver edges before any lockfile tree is written.

**Tech Stack:** Rust 2021, existing `semver`/registry range helpers, Cargo unit tests.

---

### Task 1: Preserve transitive ranges

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`
- Test: `crates/jhol-core/src/lockfile_write.rs`

- [ ] **Step 1: Write the failing test**

Add a unit test constructing metadata where `parent@1.0.0` requires `child@^1.0.0`, while the child domain contains `1.5.0` and `2.0.0`. Assert the generated dependency edge contains only `1.5.0`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p jhol-core lockfile_write::tests::jagr_request_filters_transitive_ranges --lib -- --exact`

Expected: FAIL because the current request builder retains `2.0.0`.

- [ ] **Step 3: Write minimal implementation**

Add `versions_satisfying_spec(spec, versions)` and use it for every dependency edge. Return an error when a required dependency has no metadata or no satisfying version; never create a `"*"` candidate.

- [ ] **Step 4: Run test to verify it passes**

Run the exact test from Step 2 and expect PASS.

### Task 2: Add a synthetic root

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`
- Test: `crates/jhol-core/src/lockfile_write.rs`

- [ ] **Step 1: Write the failing test**

Add a test asserting the request root is `__jhol_project_root__@0.0.0`, has a single version, and has range-filtered hard edges to every required direct dependency.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p jhol-core lockfile_write::tests::jagr_request_uses_synthetic_project_root --lib -- --exact`

Expected: FAIL because the alphabetically first dependency is currently used as root.

- [ ] **Step 3: Write minimal implementation**

Create constants for the synthetic package/version and insert the synthetic domain and root dependency edges after all real package domains have been built.

- [ ] **Step 4: Run test to verify it passes**

Run the exact test from Step 2 and expect PASS.

### Task 3: Certify assignments

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`
- Test: `crates/jhol-core/src/lockfile_write.rs`

- [ ] **Step 1: Write failing tests**

Add tests proving the certificate rejects a missing required root dependency and a selected transitive version outside its original range, while accepting a complete valid selection.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p jhol-core lockfile_write::tests::jagr_certificate --lib`

Expected: FAIL because no certificate function exists.

- [ ] **Step 3: Write minimal implementation**

Add a pure `certify_jagr_assignment` helper. Validate root requirements, every selected package's dependency edges, selected-version membership, and exclude the synthetic root from materialization.

- [ ] **Step 4: Run tests to verify they pass**

Run the command from Step 2 and expect all certificate tests to PASS.

### Task 4: Integrate and verify

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`

- [ ] **Step 1: Integrate helpers**

Use the pure request builder from `resolve_full_tree_jagr_v3`, run the certificate immediately after resolution, and remove the synthetic root before calling `build_tree_from_assignment`.

- [ ] **Step 2: Format**

Run: `cargo fmt --all`

- [ ] **Step 3: Run resolver tests**

Run: `cargo test -p jhol-core jagr --lib`

Expected: 124 existing tests plus new Phase 1 tests pass.

- [ ] **Step 4: Run broader core tests**

Run: `cargo test -p jhol-core --lib`

Expected: all core library tests pass.

- [ ] **Step 5: Benchmark guardrail**

Build release and run the existing JAGR/Bun benchmark wrapper on the same fixtures and repeat count used for the baseline. Record median and p95 separately for cold, warm, and offline paths. Do not proceed to Phase 2 if correctness differs or the warm median regresses beyond normal run variance.
