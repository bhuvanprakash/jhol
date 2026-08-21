# Atomic Wavefront Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install the complete executable dependency closure with no binary-package runtime path, atomically complete stores, and bounded parallel cold/warm materialization.

**Architecture:** A fixed-point wavefront resolver fetches missing packuments concurrently, resolves every accumulated constraint deterministically, and repeats until no dependency edge changes. Registry tarballs are verified and extracted into temporary store directories that become visible only through atomic rename. Warm installs replay the complete exact graph from lockfile/capsule and link complete store directories.

**Tech Stack:** Rust 2021, serde_json, semver, tar, libdeflater, SHA/SRI verification, existing worker pool and blocking HTTP client, Cargo integration/unit tests, Bash brutal benchmark.

---

## File Map

- Modify `crates/jhol-core/src/install.rs`: replace incomplete transitive expansion with testable fixed-point wavefront closure; remove bundled install hooks.
- Modify `crates/jhol-core/src/registry.rs`: delete runtime binary-package reads; atomically extract and validate complete stores.
- Modify `crates/jhol-core/src/selective_extract.rs`: preserve complete runtime contents by default; minimal extraction remains explicit opt-in.
- Modify `tests/cli.rs`: add network-backed full-closure cold/warm regression test guarded by the existing test environment.
- Modify `benchmarks/brutal/brutal_v2.sh`: make benchmark failures visible and validate full executable installs.
- Modify `AGENTS.md`: record final architecture, quality evidence, and measured numbers after verification.

### Task 1: Add Fixed-Point Closure Regression Tests

**Files:**
- Modify: `crates/jhol-core/src/install.rs:570-955`

- [ ] **Step 1: Add a metadata-injected closure helper test module**

Add tests that construct synthetic packuments and exercise a helper named
`resolve_dependency_closure_with`:

```rust
#[cfg(test)]
mod wavefront_tests {
    use super::*;

    fn packument(versions: &[(&str, &[(&str, &str)])]) -> serde_json::Value {
        let versions = versions.iter().map(|(version, deps)| {
            let dependencies = deps.iter()
                .map(|(name, range)| (name.to_string(), serde_json::Value::String(range.to_string())))
                .collect::<serde_json::Map<_, _>>();
            (version.to_string(), serde_json::json!({
                "name": "fixture",
                "version": version,
                "dependencies": dependencies,
                "dist": { "tarball": format!("https://example.invalid/{version}.tgz") }
            }))
        }).collect::<serde_json::Map<_, _>>();
        serde_json::json!({ "versions": versions })
    }

    #[test]
    fn wavefront_reaches_transitive_fixed_point() {
        let metadata = HashMap::from([
            ("root".to_string(), packument(&[("1.0.0", &[("a", "^1.0.0")])])),
            ("a".to_string(), packument(&[("1.1.0", &[("b", "^2.0.0")])])),
            ("b".to_string(), packument(&[("2.0.0", &[])])),
        ]);
        let result = resolve_dependency_closure_with(&["root@1.0.0"], 32, |names| {
            names.iter().map(|name| {
                (name.clone(), metadata.get(name).cloned().ok_or_else(|| "missing fixture".to_string()))
            }).collect()
        }).unwrap();
        assert_eq!(result, vec!["a@1.1.0", "b@2.0.0", "root@1.0.0"]);
    }

    #[test]
    fn wavefront_reselects_after_late_constraint() {
        let metadata = HashMap::from([
            ("root".to_string(), packument(&[("1.0.0", &[("a", "^1.0.0"), ("bridge", "1.0.0")])])),
            ("bridge".to_string(), packument(&[("1.0.0", &[("a", ">=1.2.0")])])),
            ("a".to_string(), packument(&[("1.0.0", &[]), ("1.2.0", &[])])),
        ]);
        let result = resolve_dependency_closure_with(&["root@1.0.0"], 32, |names| {
            names.iter().map(|name| {
                (name.clone(), metadata.get(name).cloned().ok_or_else(|| "missing fixture".to_string()))
            }).collect()
        }).unwrap();
        assert!(result.contains(&"a@1.2.0".to_string()));
        assert!(!result.contains(&"a@1.0.0".to_string()));
    }
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test -p jhol-core wavefront_ -- --nocapture`

Expected: compile failure because `resolve_dependency_closure_with` does not exist.

- [ ] **Step 3: Keep this checkpoint uncommitted**

Do not commit unless the user explicitly requests a commit.

### Task 2: Implement the Atomic Wavefront Resolver

**Files:**
- Modify: `crates/jhol-core/src/install.rs:570-955`

- [ ] **Step 1: Implement the injectable resolver**

Replace the current three-pass queue logic with:

```rust
fn resolve_dependency_closure_with<F>(
    packages: &[&str],
    max_depth: usize,
    mut fetch_wave: F,
) -> Result<Vec<String>, String>
where
    F: FnMut(&[String]) -> Vec<(String, Result<serde_json::Value, String>)>,
{
    let mut constraints: HashMap<String, Vec<String>> = HashMap::new();
    let mut required: HashMap<String, bool> = HashMap::new();
    let mut depth_by_name: HashMap<String, usize> = HashMap::new();
    let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
    let mut selected: HashMap<String, String> = HashMap::new();
    let mut expanded_version: HashMap<String, String> = HashMap::new();

    for package in packages {
        let name = base_name(package).to_string();
        constraints.entry(name.clone()).or_default().push(requested_version_spec(package));
        required.insert(name.clone(), true);
        depth_by_name.insert(name, 0);
    }

    loop {
        let mut missing = constraints.keys()
            .filter(|name| !metadata.contains_key(*name))
            .cloned().collect::<Vec<_>>();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            for (name, result) in fetch_wave(&missing) {
                metadata.insert(name.clone(), result.map_err(|e| format!("Failed to fetch metadata for {name}: {e}"))?);
            }
        }

        let mut changed = false;
        let mut names = constraints.keys().cloned().collect::<Vec<_>>();
        names.sort();
        for name in names {
            let ranges = constraints.get(&name).cloned().unwrap_or_default();
            let packument = metadata.get(&name).ok_or_else(|| format!("Missing metadata for {name}"))?;
            let seeded = crate::lattice_cache::lookup_lattice_for_all(&name, &ranges);
            let version = seeded.or_else(|| resolve_version_for_constraints(packument, &ranges))
                .ok_or_else(|| format!("Dependency conflict for {name} (constraints: {})", ranges.join(", ")))?;
            if !version_supported_on_current_platform(packument, &version) {
                if *required.get(&name).unwrap_or(&true) {
                    return Err(format!("Package {name}@{version} is incompatible with current platform (os/cpu)"));
                }
                continue;
            }
            if selected.get(&name) != Some(&version) {
                selected.insert(name.clone(), version);
                changed = true;
            }
        }

        let mut discovered = false;
        let selections = selected.iter().map(|(n, v)| (n.clone(), v.clone())).collect::<Vec<_>>();
        for (name, version) in selections {
            if expanded_version.get(&name) == Some(&version) {
                continue;
            }
            let depth = *depth_by_name.get(&name).unwrap_or(&0);
            expanded_version.insert(name.clone(), version.clone());
            if depth >= max_depth { continue; }
            let packument = metadata.get(&name).ok_or_else(|| format!("Missing metadata for {name}"))?;
            for (dep, range, is_required) in dependency_edges(packument, &version) {
                let ranges = constraints.entry(dep.clone()).or_default();
                if !ranges.contains(&range) {
                    ranges.push(range);
                    discovered = true;
                }
                required.entry(dep.clone()).and_modify(|v| *v |= is_required).or_insert(is_required);
                depth_by_name.entry(dep).and_modify(|d| *d = (*d).min(depth + 1)).or_insert(depth + 1);
            }
        }
        if !changed && !discovered { break; }
    }

    let mut specs = selected.into_iter().map(|(n, v)| format!("{n}@{v}")).collect::<Vec<_>>();
    specs.sort();
    Ok(specs)
}
```

Add `dependency_edges` as a small helper combining existing required and optional dependency readers. Add `lookup_lattice_for_all` in `lattice_cache.rs`; it returns a seeded version only when it satisfies every range.

- [ ] **Step 2: Wire production metadata waves**

Make `expand_with_transitive_dependencies` preserve its persistent memo cache and call the helper with `registry::parallel_fetch_metadata`. Remove both obsolete queue passes and all binary-index branches.

- [ ] **Step 3: Run tests and verify GREEN**

Run: `cargo test -p jhol-core wavefront_ lattice_cache::tests -- --nocapture`

Expected: wavefront and lattice tests pass.

- [ ] **Step 4: Run existing core tests**

Run: `cargo test -p jhol-core -- --skip pubgrub --skip jagrs`

Expected: new tests pass; only the known macOS Windows-path assertion may remain.

### Task 3: Remove All Runtime Binary-Package Paths

**Files:**
- Modify: `crates/jhol-core/src/install.rs:139-184, 1097-1110, 1200-1215, 1958, 2260-2325`
- Modify: `crates/jhol-core/src/registry.rs:1062-1249, 1368-1408, 1518-1539, 1680-1744`

- [ ] **Step 1: Add a source-level regression test**

Add a test in `tests/cli.rs` that reads `registry.rs` and asserts production download functions do not contain `load_binary_tarball(`. This fails before deletion and guards the explicit no-binary requirement.

- [ ] **Step 2: Run test and verify RED**

Run: `cargo test --test cli runtime_download_path_does_not_use_binary_packages -- --nocapture`

Expected: FAIL because both download functions still prefer binary tarballs.

- [ ] **Step 3: Delete binary runtime code**

Remove binary directory/index discovery, `.jhol` decoding, bundled cache accessors, store preseed, binary materialization in `get_cached_tarball_fast`, and `try_install_bundled_from_cache`. Remove the two `load_binary_tarball` branches from `download_tarball_to_store_hash_only` and `download_unpack_to_store_unpacked`.

- [ ] **Step 4: Run test and verify GREEN**

Run: `cargo test --test cli runtime_download_path_does_not_use_binary_packages -- --nocapture`

Expected: PASS.

### Task 4: Make Store Extraction Atomic and Complete

**Files:**
- Modify: `crates/jhol-core/src/registry.rs:1449-1604`

- [ ] **Step 1: Add failing atomic-store unit tests**

Create an in-memory fixture tarball with `package/package.json` and `package/index.js`, then test that a pre-existing directory without `.jhol-complete` is rebuilt and that successful extraction creates the marker.

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test -p jhol-core atomic_unpack_ -- --nocapture`

Expected: FAIL because current code accepts any existing directory.

- [ ] **Step 3: Implement atomic extraction**

Add `ensure_complete_unpacked` that validates a marker, removes incomplete destinations, extracts to `<hash>.tmp-<pid>-<counter>`, verifies package identity, writes `.jhol-complete`, then renames into place. Use it from both download paths and `ensure_unpacked_in_store`.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `cargo test -p jhol-core atomic_unpack_ -- --nocapture`

Expected: PASS; incomplete directory is replaced and both package files exist.

### Task 5: Preserve Complete Runtime Package Contents

**Files:**
- Modify: `crates/jhol-core/src/selective_extract.rs:16-38, 86-105`

- [ ] **Step 1: Add a failing default-mode test**

Assert that `FeatureSheaf::from_env()` defaults to `Full`, and that an arbitrary runtime asset such as `vendor/runtime.dat` is preserved in default mode.

- [ ] **Step 2: Run test and verify RED**

Run: `cargo test -p jhol-core selective_extract::tests::default_mode_is_full -- --nocapture`

Expected: FAIL because default currently maps to `Default` selective mode.

- [ ] **Step 3: Make complete extraction the default**

Map an unset `JHOL_EXTRACT_MODE` to `Full`; retain `minimal` as explicit opt-in. Keep legal/type filtering code for explicit minimal mode only.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `cargo test -p jhol-core selective_extract::tests -- --nocapture`

Expected: PASS.

### Task 6: Add Full Cold/Warm Executability Regression

**Files:**
- Modify: `tests/cli.rs`

- [ ] **Step 1: Add ignored network integration test**

Create a temp project with express, axios, react, and react-dom; install with an isolated cache; run Node imports; delete node_modules; reinstall from the same cache with `--offline`; run imports again; assert lockfile package count exceeds direct dependency count.

- [ ] **Step 2: Run cold test**

Run: `cargo test --test cli complete_dependency_closure_cold_and_warm -- --ignored --nocapture`

Expected after Tasks 2-5: PASS with required imports and a transitive lockfile.

### Task 7: Repair Brutal Benchmark Truthfulness

**Files:**
- Modify: `benchmarks/brutal/brutal_v2.sh:74-86, 127-183, 203-235`

- [ ] **Step 1: Fix shell and exit handling**

Change standalone `binary` to `local binary`; remove `|| true` from measured installs; record failures and exclude failed runs from medians.

- [ ] **Step 2: Add closure validation**

For Jhol simple, run dynamic imports for all direct packages and representative transitives (`body-parser`, `follow-redirects`, `scheduler`). Count files with `find -L` and record lockfile package count. A failed import sets `install_ok=0`.

- [ ] **Step 3: Smoke-run benchmark**

Run: `TOOLS="jhol" PROJECTS="simple" REPS=1 WARMUP=0 OUT_DIR="results/awc-smoke" bash brutal_v2.sh`

Expected: script completes, exit code is recorded, and install_ok is 1 only for a complete executable graph.

### Task 8: Verify and Measure

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: Run formatting and correctness suite**

Run: `cargo fmt --check`

Run: `cargo test --test cli`

Run: `cargo test -p jhol-core -- --skip pubgrub --skip jagrs`

Run: `cargo test --test cli complete_dependency_closure_cold_and_warm -- --ignored --nocapture`

- [ ] **Step 2: Build release binary**

Run: `cargo build --release`

Expected: successful build, binary remains near 6 MB and contains no embedded `.jhol` data.

- [ ] **Step 3: Run trustworthy brutal benchmark**

Run: `TOOLS="jhol bun" PROJECTS="simple heavy" REPS=3 WARMUP=0 OUT_DIR="results/awc-final" bash brutal_v2.sh`

Report median successful cold/warm timings, package/lockfile/file counts, import checks, binary size, and variance. Do not claim world-fastest status unless measured against the same successful workload.

- [ ] **Step 4: Update work log**

Document architecture, fixed correctness defects, exact commands, passing counts, benchmark numbers, and remaining risks in `AGENTS.md`.

- [ ] **Step 5: Leave changes uncommitted**

Do not commit unless the user explicitly asks for a local commit.
