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

#[test]
fn test_install_frozen_alias_is_accepted() {
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

    let out = jhol_in(td.path()).args(["install", "--frozen"]).output().unwrap();
    assert!(!out.status.success(), "expected failure without lockfile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unexpected argument '--frozen'"),
        "--frozen should be accepted as alias"
    );
}

#[test]
fn test_offline_frozen_reports_transitive_lockfile_missing_cache() {
    let td = tempdir().unwrap();
    let cache_dir = td.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": {
    "a": "1.0.0"
  }
}
"#,
    )
    .unwrap();

    std::fs::write(
        td.path().join("package-lock.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "fixture", "version": "1.0.0" },
    "node_modules/a": {
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/a/-/a-1.0.0.tgz",
      "integrity": "sha512-xxx"
    },
    "node_modules/b": {
      "version": "2.0.0",
      "resolved": "https://registry.npmjs.org/b/-/b-2.0.0.tgz",
      "integrity": "sha512-yyy"
    }
  }
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .env("JHOL_CACHE_DIR", &cache_dir)
        .args(["install", "--offline", "--frozen-lockfile"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "offline frozen install should fail on empty cache");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("a@1.0.0"),
        "expected top-level lockfile spec in error: {}",
        stderr
    );
    assert!(
        stderr.contains("b@2.0.0"),
        "expected transitive lockfile spec in error: {}",
        stderr
    );
}

#[test]
fn test_fixture_package_jsons_parse() {
    let fixtures = [
        "tests/fixtures/react-app/package.json",
        "tests/fixtures/express-app/package.json",
        "tests/fixtures/typescript-app/package.json",
        "tests/fixtures/peer-conflict/package.json",
        "tests/fixtures/optional-deps-app/package.json",
        "tests/fixtures/peer-deps-app/package.json",
        "tests/fixtures/overrides-app/package.json",
        "tests/fixtures/workspace-app/package.json",
        "tests/fixtures/next-app/package.json",
        "tests/fixtures/nuxt-app/package.json",
        "tests/fixtures/nest-app/package.json",
        "tests/fixtures/turbo-app/package.json",
        "tests/fixtures/expo-app/package.json",
    ];
    for fixture in fixtures {
        let data = std::fs::read_to_string(fixture).expect("fixture package.json missing");
        let v: serde_json::Value = serde_json::from_str(&data).expect("fixture json invalid");
        let has_deps = v.get("dependencies").is_some();
        let has_dev_deps = v.get("devDependencies").is_some();
        assert!(
            has_deps || has_dev_deps,
            "fixture missing dependencies/devDependencies: {}",
            fixture
        );
    }
}

#[test]
fn test_peer_dependency_conflict_detected() {
    let out = jhol_in(std::path::Path::new("tests/fixtures/peer-conflict"))
        .args(["install", "--lockfile-only"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "expected lockfile-only to fail on peer dependency conflict"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("peer") || stderr.contains("Dependency conflict"),
        "expected peer dependency conflict message, got: {}",
        stderr
    );
}

#[test]
fn test_cache_import_requires_signature_when_key_set() {
    let td = tempdir().unwrap();
    let export_dir = td.path().join("export");
    std::fs::create_dir_all(&export_dir).unwrap();
    let manifest_path = export_dir.join("manifest.json");
    std::fs::write(manifest_path, "[]").unwrap();

    let out = jhol_in(td.path())
        .env("JHOL_CACHE_SIGNING_KEY", "secret")
        .args(["cache", "import", export_dir.to_string_lossy().as_ref()])
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected import to fail without signature");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("signature"), "expected signature error, got: {}", stderr);
}

#[test]
fn test_uninstall_without_package_json_change() {
    let td = tempdir().unwrap();
    std::fs::create_dir_all(td.path().join("node_modules/foo")).unwrap();
    std::fs::write(td.path().join("node_modules/foo/package.json"), "{}")
        .unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": { "foo": "1.0.0" }
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .args(["uninstall", "foo"])
        .output()
        .unwrap();
    assert!(out.status.success(), "uninstall should succeed");
    assert!(!td.path().join("node_modules/foo").exists());
    let pj = std::fs::read_to_string(td.path().join("package.json")).unwrap();
    assert!(pj.contains("foo"), "package.json should remain unchanged without --save");
}

#[test]
fn test_update_requires_save_for_specific_packages() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": { "left-pad": "1.3.0" }
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .args(["update", "left-pad"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "expected update without --save to fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--save"), "expected hint about --save, got: {}", stderr);
}
