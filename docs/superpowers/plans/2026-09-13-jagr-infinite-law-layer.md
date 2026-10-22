# JAGR Infinite Law Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep JAGR-infinite as main custom resolver while enforcing npm-compatible legality for exact pins and direct ranges.

**Architecture:** JAGR-infinite remains candidate engine. A shared finalization path reconciles versions, prunes unreachable packages, validates published versions, and persists only legal results. Homotopy and RVM both flow through this same path.

**Tech Stack:** Rust 2021, serde_json, semver, cargo test.

---

### Task 1: Exact direct pin test

**Files:**
- Modify: `crates/jhol-core/src/jagr_infinite.rs`

- [ ] Add unit test showing direct exact pin is enforced by reconciliation/finalization.
- [ ] Run focused test and verify it fails before code change.
- [ ] Implement smallest reconciliation/finalization fix.
- [ ] Run focused test and smoke install.

### Task 2: Homotopy safety path

**Files:**
- Modify: `crates/jhol-core/src/jagr_infinite.rs`

- [ ] Route homotopy candidate through same finalize function as RVM.
- [ ] Fix lockfile version ordinal lookup using chart version list, not hash.
- [ ] Run focused tests.

### Task 3: Verification

**Files:**
- Modify: `crates/jhol-core/src/jagr_infinite.rs`

- [ ] Run `cargo test -p jhol-core jagr_infinite`.
- [ ] Run isolated `jhol install` exact-pin smoke for `lodash@4.17.21`.
