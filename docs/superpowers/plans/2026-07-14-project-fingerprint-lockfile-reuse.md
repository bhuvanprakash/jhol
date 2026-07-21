# Project Fingerprint and Lockfile Reuse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make unchanged installs and unchanged lockfile-only commands return without redundant resolution while preserving exact manifest identity, integrity validation, and every existing resolver fallback.

**Architecture:** Add a versioned project-scoped fingerprint record keyed by canonical project path and containing both manifest and lockfile hashes. Add the same manifest hash to Jhol-generated lockfiles, then reuse a lockfile only when that full-manifest hash, root identity, direct dependency set, compatible resolved versions, and integrity data all validate.

**Tech Stack:** Rust 2021, serde/serde_json, xxhash-rust xxh3, tempfile-based unit tests, Cargo tests, release Jhol-versus-Bun harness.

---

### Task 1: Versioned project-scoped fingerprints

**Files:**
- Modify: `crates/jhol-core/src/install.rs`

- [ ] **Step 1: Write failing isolated filesystem tests**

Add tests for `lockfile_fingerprint_matches_in(project, fingerprint_root)` and `write_project_fingerprint_in(project, fingerprint_root)` using two `tempfile::TempDir` projects. Assert matching files hit, two projects do not collide, manifest/lockfile mutations miss, malformed records miss, and missing `node_modules` misses.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p jhol-core install::tests::project_fingerprint --lib -- --test-threads=1`

Expected: compilation fails because the scoped helpers do not exist.

- [ ] **Step 3: Implement the minimal fingerprint unit**

Add a serde record with `format_version`, `manifest_hash`, and `lockfile_hash`. Derive a filename from xxh3(canonical project path), hash file bytes directly, write `<fingerprints>/<project-id>.tmp`, and atomically rename it. The public no-argument wrapper resolves cwd and `~/.jhol/fingerprints` and never trusts the legacy global file.

- [ ] **Step 4: Write the record on every successful native/backend install return**

Replace the single `global_fingerprint.json` write and add `write_project_fingerprint()` after `sync_hidden_lockfile()` on the lockfile-layout, cache-only, native-success, and backend-success paths. Fingerprint write failure remains a cache miss on the next command and does not turn a successful install into failure.

- [ ] **Step 5: Verify GREEN**

Run: `cargo test -p jhol-core install::tests::project_fingerprint --lib -- --test-threads=1`

Expected: all fingerprint tests pass.

### Task 2: Exact manifest identity in generated lockfiles

**Files:**
- Modify: `crates/jhol-core/src/lockfile_write.rs`

- [ ] **Step 1: Write a failing writer test**

Generate a lockfile from a temporary manifest and minimal resolved tree. Assert top-level `jholManifestHash` equals `xxh3_64` of the exact manifest bytes, then mutate whitespace in the manifest and assert the expected hash changes.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p jhol-core lockfile_write::tests::package_lock_records_exact_manifest_hash --lib -- --exact`

Expected: assertion fails because `jholManifestHash` is absent.

- [ ] **Step 3: Add one shared manifest-hash helper and emit the field**

Implement `manifest_content_hash(path) -> Result<String, String>` using `std::fs::read` and xxh3. Add `"jholManifestHash": manifest_hash` to every Jhol package-lock construction path, including incremental, full, and async writers.

- [ ] **Step 4: Verify GREEN and existing writer parity**

Run: `cargo test -p jhol-core lockfile_write::tests --lib -- --test-threads=1`

Expected: all lockfile writer tests pass.

### Task 3: Validated lockfile-only reuse

**Files:**
- Modify: `crates/jhol-core/src/install.rs`
- Test: `crates/jhol-core/src/install.rs` inline test module

- [ ] **Step 1: Write failing validator tests**

Construct temporary manifests/lockfiles without network access. Assert reuse only for matching `jholManifestHash`, root name/version, identical root dependency names, compatible exact resolved versions, and complete integrity. Assert false for manifest mutation, missing hash, malformed JSON, missing integrity, missing/extra root dependencies, and incompatible versions.

- [ ] **Step 2: Verify RED**

Run: `cargo test -p jhol-core install::tests::lockfile_reuse --lib -- --test-threads=1`

Expected: compilation fails because `lockfile_is_reusable` does not exist.

- [ ] **Step 3: Implement the pure validator**

Parse both JSON documents, compare the exact manifest content hash, root name/version, and root dependency names. For every manifest dependency/optional/dev dependency, read the root locked version and call Jhol's existing `registry::version_satisfies`. Reuse the existing `lockfile_integrity_complete` rule. Any parse, protocol, range, or metadata uncertainty returns `false`.

- [ ] **Step 4: Place the fast path before network validation**

At the start of `install_lockfile_only`, after confirming `package.json` exists, return `Ok(())` when `lockfile_is_reusable(package.json, package-lock.json)` succeeds. Otherwise execute the original peer validation, full native resolution, and deterministic writer unchanged.

- [ ] **Step 5: Verify GREEN and fallback behavior**

Run: `cargo test -p jhol-core install::tests::lockfile_reuse --lib -- --test-threads=1`

Expected: all validator tests pass and mismatch cases remain misses.

### Task 4: Correctness and performance guardrails

**Files:**
- Verify: `crates/jhol-core/src/install.rs`
- Verify: `crates/jhol-core/src/lockfile_write.rs`

- [ ] **Step 1: Format and run focused tests**

Run: `rustfmt --edition 2021 --check crates/jhol-core/src/install.rs crates/jhol-core/src/lockfile_write.rs`

Run: `cargo test -p jhol-core install::tests --lib -- --test-threads=1`

Run: `cargo test -p jhol-core lockfile_write::tests --lib -- --test-threads=1`

- [ ] **Step 2: Run complete core and relevant CLI tests**

Run: `cargo test -p jhol-core --lib -- --test-threads=1`

Run: `cargo test --test cli -- --test-threads=1`

Expected: zero failures.

- [ ] **Step 3: Build the exact release binary**

Run: `cargo build --release -p jhol`

Expected: exit 0; existing unrelated warnings are recorded but not treated as new failures.

- [ ] **Step 4: Rerun Jhol-after versus Bun**

Recreate `/tmp/jhol-bench` from `tests/fixtures/jagr-instance-bench/package.json` and run `bash bench_vs_bun.sh`. For noisy cases, rerun at least seven samples using the same fixture, caches, binaries, and command flags.

- [ ] **Step 5: Apply the guardrail**

Compare against the recorded before medians: warm 251 ms, no-op 275 ms, offline 238 ms, lockfile-only 1489 ms, cached add 565 ms, repeat-warm 598 ms; Bun medians were 424, 176, N/A, 501, 315, and 525 ms. Revert the independently responsible optimization if any current Jhol-winning case regresses repeatedly by more than 3%, or if correctness differs.
