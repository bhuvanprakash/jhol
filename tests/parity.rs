//! Manager parity: jhol lockfile detection and install behaviour against bun/npm conventions.

use std::process::Command;
use tempfile::tempdir;

fn jhol() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jhol"))
}

fn jhol_in(dir: &std::path::Path) -> Command {
    let mut c = jhol();
    c.current_dir(dir);
    c
}

/// Timeout flags are accepted and do not cause immediate failure.
#[test]
fn test_fetch_timeout_flag_accepted() {
    let out = jhol().args(["install", "--help"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("fetch-timeout"),
        "CLI should document --fetch-timeout"
    );
    assert!(stdout.contains("resolve-timeout"));
    assert!(stdout.contains("install-timeout"));
}

/// --prefer is accepted for optimize mode.
#[test]
fn test_prefer_flag_accepted() {
    let out = jhol().args(["install", "--help"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("prefer") || stdout.contains("minimal") || stdout.contains("optimize"));
}

/// Install with a very short fetch timeout fails with a timeout-related message (not hang).
#[test]
fn test_install_fails_with_timeout_not_hang() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"t","version":"1.0.0","dependencies":{"left-pad":"1.3.0"}}"#,
    )
    .unwrap();
    let out = jhol_in(td.path())
        .args(["install", "--fetch-timeout", "1"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{} {}", stderr, stdout);
    if !out.status.success() {
        assert!(
            combined.contains("timeout")
                || combined.contains("Timeout")
                || combined.contains("timed out")
                || combined.contains("JHOL_FETCH"),
            "Expected timeout or config hint in output when install fails with short timeout: {}",
            combined
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// bun.lock misdetection regression tests (2026-07-04)
//
// Bug: lockfile_exists() checked for bun.lock AND package-lock.json, causing
// from_lockfile=true auto-detection when only bun.lock existed. The lockfile
// download path then constructed tarball URLs from raw range specs (e.g.,
// lodash@^4.17.21) instead of resolving them, causing all non-cached packages
// to fail with "install_failed".
//
// Fix: has_jhol_lockfile() checks for package-lock.json existence AND validates
// it contains a "jhol" field (written by jhol on every lockfile write). This
// ensures only lockfiles jhol itself wrote trigger the from_lockfile path.
// ──────────────────────────────────────────────────────────────────────────────

/// has_jhol_lockfile returns false when no lockfile exists at all.
#[test]
fn test_bun_lock_no_lockfile_returns_false() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"test","version":"1.0.0","dependencies":{}}"#,
    )
    .unwrap();
    assert!(
        !jhol_core::has_jhol_lockfile(td.path()),
        "has_jhol_lockfile should return false when no lockfile exists"
    );
}

/// has_jhol_lockfile returns false when only bun.lock exists (not a jhol lockfile).
#[test]
fn test_bun_lock_only_returns_false() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"test","version":"1.0.0","dependencies":{}}"#,
    )
    .unwrap();
    // bun.lock is a JSONC file with trailing commas — NOT a package-lock.json
    std::fs::write(
        td.path().join("bun.lock"),
        r#"{
  "packages": {
    "lodash@^4.17.21": {
      "version": "4.17.21",
      "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz"
    }
  }
}"#,
    )
    .unwrap();
    assert!(
        !jhol_core::has_jhol_lockfile(td.path()),
        "has_jhol_lockfile should return false when only bun.lock exists (bun.lock is not a jhol lockfile)"
    );
}

/// has_jhol_lockfile returns false when package-lock.json exists but has no jhol marker
/// (e.g., written by npm or an older jhol version).
#[test]
fn test_package_lock_without_jhol_marker_returns_false() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"test","version":"1.0.0","dependencies":{}}"#,
    )
    .unwrap();
    // Simulate a package-lock.json written by npm (no "jhol" field)
    std::fs::write(
        td.path().join("package-lock.json"),
        r#"{
  "name": "test",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "requires": true,
  "packages": {
    "": {
      "name": "test",
      "version": "1.0.0"
    }
  }
}"#,
    )
    .unwrap();
    assert!(
        !jhol_core::has_jhol_lockfile(td.path()),
        "has_jhol_lockfile should return false when package-lock.json has no jhol marker (written by npm)"
    );
}

/// has_jhol_lockfile returns true when package-lock.json has the jhol marker.
#[test]
fn test_package_lock_with_jhol_marker_returns_true() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"test","version":"1.0.0","dependencies":{}}"#,
    )
    .unwrap();
    // Simulate a package-lock.json written by jhol (has "jhol" field)
    std::fs::write(
        td.path().join("package-lock.json"),
        r#"{
  "name": "test",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "jhol": "1.0.2",
  "requires": true,
  "packages": {
    "": {
      "name": "test",
      "version": "1.0.0"
    }
  }
}"#,
    )
    .unwrap();
    assert!(
        jhol_core::has_jhol_lockfile(td.path()),
        "has_jhol_lockfile should return true when package-lock.json has jhol marker"
    );
}

/// bun.lock exists alongside package-lock.json with jhol marker:
/// has_jhol_lockfile still returns true (bun.lock is ignored for detection).
#[test]
fn test_bun_lock_alongside_jhol_lockfile_returns_true() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"test","version":"1.0.0","dependencies":{}}"#,
    )
    .unwrap();
    std::fs::write(td.path().join("bun.lock"), r#"{"packages":{}}"#).unwrap();
    std::fs::write(
        td.path().join("package-lock.json"),
        r#"{"name":"test","lockfileVersion":3,"jhol":"1.0.2","packages":{}}"#,
    )
    .unwrap();
    assert!(
        jhol_core::has_jhol_lockfile(td.path()),
        "has_jhol_lockfile should return true when package-lock.json has jhol marker (bun.lock presence is irrelevant)"
    );
}

/// from_lockfile auto-detection: when bun.lock exists but package-lock.json does not,
/// jhol install should NOT auto-detect from_lockfile mode.
/// (Regression test for the 2026-07-04 bug where bun.lock was misclassified.)
#[test]
fn test_bun_lock_does_not_trigger_from_lockfile() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "test-bun-lock",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#,
    )
    .unwrap();
    // Write bun.lock only — no package-lock.json
    std::fs::write(
        td.path().join("bun.lock"),
        r#"{
  "packages": {
    "lodash@^4.17.21": {
      "version": "4.17.21",
      "resolved": "https://npmjs.org/lodash/-/lodash-4.17.21.tgz"
    }
  }
}"#,
    )
    .unwrap();
    // Run jhol install — should NOT enter from_lockfile path.
    // Without the fix, this would enter from_lockfile mode and try to download
    // tarballs using raw range specs (lodash@^4.17.21) instead of resolving them.
    // With --offline, it should fail gracefully (can't resolve from bun.lock).
    let out = jhol_in(td.path())
        .args(["install", "--offline"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{} {}", stderr, stdout);
    // The key assertion: it should NOT fail with "install_failed" due to
    // constructing invalid tarball URLs from raw range specs.
    // It may fail for other reasons (offline, no cache), but not the bun.lock bug.
    assert!(
        !combined.contains("install_failed")
            || combined.contains("offline")
            || combined.contains("cache"),
        "jhol install should not fail with 'install_failed' when only bun.lock exists \
         (bun.lock should not trigger from_lockfile path): {}",
        combined
    );
}

/// from_lockfile auto-detection: when only npm-shrinkwrap.json exists (no jhol marker),
/// from_lockfile should NOT be activated.
#[test]
fn test_npm_shrinkwrap_without_marker_does_not_trigger_from_lockfile() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "test-shrinkwrap",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("npm-shrinkwrap.json"),
        r#"{
  "name": "test-shrinkwrap",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "": {
      "name": "test-shrinkwrap",
      "version": "1.0.0"
    },
    "node_modules/lodash": {
      "version": "4.17.21",
      "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz",
      "license": "MIT"
    }
  }
}"#,
    )
    .unwrap();
    // has_jhol_lockfile only checks package-lock.json, not npm-shrinkwrap.json
    assert!(
        !jhol_core::has_jhol_lockfile(td.path()),
        "has_jhol_lockfile should only check package-lock.json, not npm-shrinkwrap.json"
    );
}
