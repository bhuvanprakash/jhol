//! Integration tests: run the jhol binary and check exit codes and output.

use std::process::Command;

fn jhol() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jhol"))
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
    let out = jhol().arg("install").output().unwrap();
    assert!(!out.status.success(), "jhol install with no package.json should fail");
}

#[test]
fn test_cache_key_succeeds() {
    let out = jhol().args(["cache", "key"]).output().unwrap();
    assert!(out.status.success(), "jhol cache key should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.is_empty());
}
