//! Integration tests: run the jhol binary and check exit codes and output.

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

#[test]
fn test_help() {
    let out = jhol().arg("--help").output().unwrap();
    assert!(out.status.success(), "jhol --help should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("install"));
    assert!(stdout.contains("doctor"));
    assert!(stdout.contains("cache"));
}

#[test]
fn test_version() {
    let out = jhol().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1.0.0") || stdout.contains("jhol"));
}

#[test]
fn test_cache_list_empty_or_success() {
    let out = jhol().args(["cache", "list"]).output().unwrap();
    assert!(out.status.success(), "jhol cache list should succeed");
}

#[test]
fn test_install_no_package_json_fails() {
    let td = tempdir().unwrap();
    let out = jhol_in(td.path()).arg("install").output().unwrap();
    assert!(!out.status.success(), "jhol install with no package.json should fail");
}

#[test]
fn test_cache_key_succeeds() {
    let td = tempdir().unwrap();
    let out = jhol_in(td.path()).args(["cache", "key"]).output().unwrap();
    assert!(out.status.success(), "jhol cache key should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.is_empty());
}

#[test]
fn test_install_frozen_requires_lockfile() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": {
    "left-pad": "1.3.0"
  }
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .args(["install", "--frozen-lockfile"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "jhol install --frozen should fail when lockfile is missing"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Strict lockfile required"),
        "expected strict lockfile error, got: {}",
        stderr
    );
}

#[test]
fn test_install_offline_without_cache_fails() {
    let td = tempdir().unwrap();
    let cache_dir = td.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": {
    "left-pad": "1.3.0"
  }
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .env("JHOL_CACHE_DIR", &cache_dir)
        .args(["install", "--offline"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "jhol install --offline should fail on cache miss"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Offline mode: package(s) not in cache"),
        "expected offline cache-miss error, got: {}",
        stderr
    );
}

#[test]
fn test_cache_export_without_package_json_fails() {
    let td = tempdir().unwrap();
    let out_dir = td.path().join("out");
    let out = jhol_in(td.path())
        .args(["cache", "export", out_dir.to_string_lossy().as_ref()])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "cache export without package.json should fail"
    );
}
