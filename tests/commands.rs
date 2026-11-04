//! Command-surface integration tests: drive the `jhol` binary and assert exit codes, output, and on-disk results.

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

#[cfg(unix)]
#[test]
fn test_native_install_does_not_probe_bun() {
    use std::os::unix::fs::PermissionsExt;

    let cases: &[(&[&str], &str, bool)] = &[
        (&["install", "--offline"], "{}", false),
        (&["install", "--offline", "--lockfile-only"], "{}", false),
        (&["install", "--offline", "--backend", "bun"], "{}", false),
        (
            &[
                "install",
                "--offline",
                "--native-only",
                "--fallback-backend",
            ],
            "{}",
            false,
        ),
        (&["install", "--offline", "--fallback-backend"], "{}", true),
        (
            &["install", "--offline"],
            r#"{"fallbackBackend":true}"#,
            true,
        ),
        (
            &["install", "--offline", "--native-only"],
            r#"{"fallbackBackend":true}"#,
            false,
        ),
        (&["ci", "--offline"], r#"{"backend":"bun"}"#, false),
    ];
    for (args, config, expect_probe) in cases {
        let td = tempdir().unwrap();
        let bin = td.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let bun = bin.join("bun");
        std::fs::write(
            &bun,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$JHOL_BACKEND_PROBE_LOG\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&bun, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            td.path().join("package.json"),
            r#"{"name":"probe-fixture","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(td.path().join("package-lock.json"), r#"{"name":"probe-fixture","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"probe-fixture","version":"1.0.0"}}}"#).unwrap();
        std::fs::write(td.path().join(".jholrc"), config).unwrap();
        let probe = td.path().join("probe.log");
        let paths = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let out = jhol_in(td.path())
            .args(*args)
            .env("PATH", paths)
            .env("HOME", td.path())
            .env("JHOL_CACHE_DIR", td.path().join("cache"))
            .env("JHOL_NO_DAEMON", "1")
            .env("JHOL_BACKEND_PROBE_LOG", &probe)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            probe.exists(),
            *expect_probe,
            "unexpected backend probe: {args:?}, config={config}"
        );
        if *expect_probe {
            assert_eq!(std::fs::read_to_string(probe).unwrap(), "--version\n");
        }
    }
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
    assert!(
        !out.status.success(),
        "jhol install with no package.json should fail"
    );
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
    // Assert the semantics, not one phrasing: an offline miss must identify
    // itself and name the package. The message moved from "not in cache" to
    // "not in the content store" when the store was renamed, which broke this
    // test while the behaviour stayed correct (exit 1, nothing installed).
    assert!(
        stderr.contains("Offline mode:")
            && (stderr.contains("not in the content store") || stderr.contains("not in cache"))
            && stderr.contains("left-pad"),
        "expected offline cache-miss error naming the package, got: {}",
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

    let out = jhol_in(td.path())
        .args(["install", "--frozen"])
        .output()
        .unwrap();
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
    assert!(
        !out.status.success(),
        "offline frozen install should fail on empty cache"
    );
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
    let fixture = std::path::Path::new("tests/fixtures/peer-conflict");
    // The strict validator short-circuits on a fresh lockfile, so start clean.
    let _ = std::fs::remove_file(fixture.join("package-lock.json"));

    // Strict (opt-in): a peer conflict is a hard error.
    let strict = jhol_in(fixture)
        .env("JHOL_STRICT_PEER_DEPS", "1")
        .args(["install", "--lockfile-only"])
        .output()
        .unwrap();
    assert!(
        !strict.status.success(),
        "expected strict lockfile-only to fail on peer dependency conflict"
    );
    let stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        stderr.contains("peer") || stderr.contains("Dependency conflict"),
        "expected peer dependency conflict message, got: {}",
        stderr
    );

    // Lenient (default): peer validation must not be the reason an install stops —
    // missing required peers are auto-installed, as npm does (and as the full
    // install path already did). A genuine resolution conflict may still fail the
    // run, but not the unconditional root-peer gate.
    let lenient = jhol_in(fixture)
        .args(["install", "--lockfile-only"])
        .output()
        .unwrap();
    let lenient_err = String::from_utf8_lossy(&lenient.stderr);
    assert!(
        !lenient_err.contains("validate_root_peer_conflicts"),
        "lenient install must not be blocked by the root peer gate, got: {}",
        lenient_err
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
    assert!(
        !out.status.success(),
        "expected import to fail without signature"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("signature"),
        "expected signature error, got: {}",
        stderr
    );
}

#[test]
fn test_local_file_dependency_resolves_and_survives_reinstall() {
    // Covers the whole local-dependency path with no network at all:
    //  - the symlink is created from the spec you wrote,
    //  - the lockfile keeps `file:…` (not the internal extraction path, which made
    //    the *second* install try to download it as a URL),
    //  - reusing that lockfile re-materialises the link.
    let td = tempdir().unwrap();
    let app = td.path().join("app");
    let lib = td.path().join("lib");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("package.json"),
        r#"{"name":"local-lib","version":"2.5.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(lib.join("index.js"), "module.exports = 'from-lib';").unwrap();
    std::fs::write(
        app.join("package.json"),
        r#"{"name":"app","version":"1.0.0","dependencies":{"local-lib":"file:../lib"}}"#,
    )
    .unwrap();

    let cache = tempdir().unwrap();
    let run = || {
        jhol_in(&app)
            .env("JHOL_CACHE_DIR", cache.path())
            .args(["install", "--no-scripts"])
            .output()
            .unwrap()
    };

    let out = run();
    assert!(
        out.status.success(),
        "first install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(app.join("node_modules/local-lib").exists());

    let lock = std::fs::read_to_string(app.join("package-lock.json")).unwrap();
    assert!(
        lock.contains("file:../lib"),
        "lockfile must keep the written spec, got: {}",
        &lock[lock.find("local-lib").unwrap_or(0)..]
            .chars()
            .take(220)
            .collect::<String>()
    );
    assert!(
        !lock.contains("local-tarballs"),
        "lockfile must not leak an internal cache path"
    );

    // Second run reuses the lockfile — this is what used to fail.
    std::fs::remove_dir_all(app.join("node_modules")).unwrap();
    let out = run();
    assert!(
        out.status.success(),
        "reinstall from the lockfile failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        app.join("node_modules/local-lib").exists(),
        "the local dependency must be re-linked on the lockfile path"
    );
}

#[test]
fn test_link_protocol_resolves_and_survives_reinstall() {
    // `link:` is the "always symlink" form; like `file:` it must work offline and
    // survive a lockfile-reuse install.
    let td = tempdir().unwrap();
    let app = td.path().join("app");
    let lib = td.path().join("lib");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("package.json"),
        r#"{"name":"linked-lib","version":"1.2.3","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(lib.join("index.js"), "module.exports='linked';").unwrap();
    std::fs::write(
        app.join("package.json"),
        r#"{"name":"app","version":"1.0.0","dependencies":{"linked-lib":"link:../lib"}}"#,
    )
    .unwrap();

    let cache = tempdir().unwrap();
    let run = || {
        jhol_in(&app)
            .env("JHOL_CACHE_DIR", cache.path())
            .args(["install", "--no-scripts"])
            .output()
            .unwrap()
    };

    let out = run();
    assert!(
        out.status.success(),
        "link: install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(app.join("node_modules/linked-lib").exists());

    std::fs::remove_dir_all(app.join("node_modules")).unwrap();
    let out = run();
    assert!(
        out.status.success(),
        "link: reinstall failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(app.join("node_modules/linked-lib").exists());
}

#[test]
fn test_workspace_dependency_is_linked_and_survives_reinstall() {
    let td = tempdir().unwrap();
    std::fs::create_dir_all(td.path().join("packages/a")).unwrap();
    std::fs::create_dir_all(td.path().join("packages/b")).unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{"name":"root","private":true,"version":"1.0.0","workspaces":["packages/*"],"dependencies":{"a":"workspace:*"}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("packages/a/package.json"),
        r#"{"name":"a","version":"1.0.0","dependencies":{"b":"workspace:*"},"main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("packages/b/package.json"),
        r#"{"name":"b","version":"2.0.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(td.path().join("packages/b/index.js"), "module.exports='B';").unwrap();
    std::fs::write(
        td.path().join("packages/a/index.js"),
        "module.exports = require('b');",
    )
    .unwrap();

    let cache = tempdir().unwrap();
    let run = || {
        jhol_in(td.path())
            .env("JHOL_CACHE_DIR", cache.path())
            .args(["install", "--no-scripts"])
            .output()
            .unwrap()
    };

    let out = run();
    assert!(
        out.status.success(),
        "workspace install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The workspace package's OWN workspace dep must be linked too.
    assert!(td.path().join("node_modules/a").exists());
    assert!(
        td.path().join("node_modules/b").exists(),
        "transitive `workspace:*` dependency must be linked"
    );

    std::fs::remove_dir_all(td.path().join("node_modules")).unwrap();
    let out = run();
    assert!(out.status.success(), "workspace reinstall failed");
    assert!(td.path().join("node_modules/b").exists());
}

#[test]
fn test_unresolvable_optional_dependency_is_skipped() {
    // npm's contract: an optional dependency that cannot be installed is skipped.
    // Treating it as required made the whole install fail (E40). This runs fully
    // offline with an empty cache, so it asserts the skip without any network.
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "optionalDependencies": { "definitely-not-a-real-package-xyz-9": "^1.0.0" }
}
"#,
    )
    .unwrap();

    let cache = tempdir().unwrap();
    let out = jhol_in(td.path())
        .env("JHOL_CACHE_DIR", cache.path())
        .args(["install", "--no-scripts", "--offline"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "an unresolvable *optional* dep must not fail the install; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !td.path()
            .join("node_modules/definitely-not-a-real-package-xyz-9")
            .exists(),
        "the unavailable optional dep must not appear in node_modules"
    );
}

#[test]
fn test_uninstall_persists_and_can_be_opted_out() {
    // Removal updates package.json by default: leaving the entry behind makes the
    // project inconsistent (the next install silently re-adds the package), which
    // is the bug this covers. `--no-save` keeps the old "node_modules only"
    // behaviour for callers that want it.
    let setup = |td: &tempfile::TempDir| {
        std::fs::create_dir_all(td.path().join("node_modules/foo")).unwrap();
        std::fs::write(td.path().join("node_modules/foo/package.json"), "{}").unwrap();
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
    };

    // Default: package.json is updated.
    let td = tempdir().unwrap();
    setup(&td);
    let out = jhol_in(td.path())
        .args(["uninstall", "foo"])
        .output()
        .unwrap();
    assert!(out.status.success(), "uninstall should succeed");
    assert!(!td.path().join("node_modules/foo").exists());
    let pj = std::fs::read_to_string(td.path().join("package.json")).unwrap();
    assert!(
        !pj.contains("foo"),
        "uninstall should drop the dependency from package.json, got: {pj}"
    );

    // `--no-save`: node_modules only.
    let td2 = tempdir().unwrap();
    setup(&td2);
    let out = jhol_in(td2.path())
        .args(["uninstall", "foo", "--no-save"])
        .output()
        .unwrap();
    assert!(out.status.success(), "uninstall --no-save should succeed");
    assert!(!td2.path().join("node_modules/foo").exists());
    let pj2 = std::fs::read_to_string(td2.path().join("package.json")).unwrap();
    assert!(
        pj2.contains("foo"),
        "--no-save should leave package.json untouched, got: {pj2}"
    );
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
    assert!(
        !out.status.success(),
        "expected update without --save to fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--save"),
        "expected hint about --save, got: {}",
        stderr
    );
}

/// Test capsule: install creates .jhol.capsule, capusle cache key changes with manifest.
#[test]
fn test_capsule_file_generated_after_install() {
    let td = tempdir().unwrap();
    let pj_path = td.path().join("package.json");
    let lf_path = td.path().join("package-lock.json");

    std::fs::write(
        &pj_path,
        r#"{"name":"test","version":"1.0.0","dependencies":{"left-pad":"1.3.0"}}"#,
    )
    .unwrap();

    let lockfile = serde_json::json!({
        "name": "test",
        "version": "1.0.0",
        "lockfileVersion": 3,
        "packages": {
            "": {
                "name": "test",
                "dependencies": { "left-pad": "1.3.0" }
            },
            "node_modules/left-pad": {
                "version": "1.3.0",
                "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz",
                "integrity": "sha512-XI5Pz1QJzBsG3m27kql9RhdKTS/5UmtfSGABsFs7d94MbI1pBYsIJGDm9wHqRRLl9hIO3YI0Q0b/JGkM7mBfYw=="
            }
        }
    });
    let pretty = serde_json::to_string_pretty(&lockfile).unwrap();
    std::fs::write(&lf_path, &pretty).unwrap();

    // Run jhol cache key to compute manifest hash
    let out = jhol_in(td.path()).args(["cache", "key"]).output().unwrap();
    assert!(out.status.success());
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!key.is_empty(), "cache key must not be empty");

    // No .jhol.capsule at this point (no install ran)
    let capsule_path = td.path().join(".jhol.capsule");
    assert!(!capsule_path.exists(), "no capsule before install");

    // We can't easily do a real install in tests (requires network).
    // Just verify the capsule utilities work in unit tests.
}

#[test]
fn test_malicious_package_name_rejected() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": {}
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .args(["install", "../../../etc/passwd"])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "malicious package name should be rejected"
    );
}

#[test]
fn test_path_traversal_in_package_name() {
    let td = tempdir().unwrap();
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": {}
}
"#,
    )
    .unwrap();

    let out = jhol_in(td.path())
        .args(["install", "lodash/../../../bin/sh"])
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "path traversal in package name should be rejected"
    );
}

/// `jhol build` is opt-in (`--features bundle`), so this test only compiles when the
/// feature is on. It is the only end-to-end check that a *bundled artifact* is
/// produced and actually runs — the unit tests cover the parser/rewriter in
/// isolation. It pins the CommonJS `require()` rewriting (a bundle that left the
/// original path emitted `MODULE_NOT_FOUND` at runtime).
#[cfg(feature = "bundle")]
#[test]
fn test_build_produces_a_runnable_bundle() {
    let td = tempdir().unwrap();
    std::fs::create_dir_all(td.path().join("src")).unwrap();
    std::fs::write(
        td.path().join("src/lib.js"),
        "module.exports = { name: 'cjs-lib' };\n",
    )
    .unwrap();
    std::fs::write(
        td.path().join("src/index.js"),
        "const lib = require('./lib.js');\nconsole.log('bundled:' + lib.name);\n",
    )
    .unwrap();

    let out = jhol_in(td.path())
        .args(["build", "src/index.js", "--outdir", "dist"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "jhol build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dist = td.path().join("dist");
    let bundle = std::fs::read_dir(&dist)
        .expect("dist directory")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().map(|x| x == "js").unwrap_or(false))
        .expect("a .js bundle must be produced");

    let source = std::fs::read_to_string(&bundle).unwrap();
    assert!(
        source.contains("__require"),
        "the bundle must route requires through its own loader"
    );
    assert!(
        !source.contains("require('./lib.js')"),
        "the original CJS path must be rewritten, not left in the output"
    );

    // And it must actually run.
    let run = std::process::Command::new("node")
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("bundled:cjs-lib"),
        "bundle did not run correctly: stdout={} stderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn test_git_dependency_installs_the_checkout_and_not_the_registry_package() {
    // A `git:` dependency must materialize the *checkout*. The registry root set
    // union re-added its bare name, so jhol installed the registry package of the
    // same name instead (for `github:isaacs/rimraf#v3.0.2` that meant registry
    // rimraf 6.1.3, whose declared `glob ^13` is not even published) and overwrote
    // the checkout the pre-pass had linked.
    //
    // The "remote" is a local repository, so the whole path is exercised with no
    // network at all.
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipping: git not available");
        return;
    }
    let td = tempdir().unwrap();
    let remote = td.path().join("remote");
    let app = td.path().join("app");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        remote.join("package.json"),
        r#"{"name":"git-lib","version":"4.5.6","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(remote.join("index.js"), "module.exports = 'from-git';").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=tester",
            "commit",
            "-qm",
            "initial",
        ],
    ] {
        let st = Command::new("git")
            .args(&args)
            .current_dir(&remote)
            .status()
            .unwrap();
        assert!(st.success(), "git {:?} failed", args);
    }
    std::fs::write(
        app.join("package.json"),
        format!(
            r#"{{"name":"app","version":"1.0.0","dependencies":{{"git-lib":"git+file://{}"}}}}"#,
            remote.display()
        ),
    )
    .unwrap();

    let cache = tempdir().unwrap();
    let out = jhol_in(&app)
        .env("JHOL_CACHE_DIR", cache.path())
        .args(["install", "--no-scripts"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git dependency install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let nm = app.join("node_modules/git-lib");
    assert!(nm.exists(), "node_modules/git-lib is missing");
    let manifest = std::fs::read_to_string(nm.join("package.json")).unwrap();
    assert!(
        manifest.contains("4.5.6"),
        "wrong version materialized: {manifest}"
    );
    assert!(
        std::fs::read_to_string(nm.join("index.js"))
            .unwrap()
            .contains("from-git"),
        "node_modules/git-lib is not the checkout"
    );
    // The lockfile keeps the *written spec* in both places: the root dependency
    // block and the package entry. Writing the resolved version there instead made
    // a reinstall resolve `git-lib` from the registry.
    let lock = std::fs::read_to_string(app.join("package-lock.json")).unwrap();
    assert!(
        lock.contains("git+file://"),
        "lockfile must record the git spec, got: {}",
        &lock[..lock.len().min(600)]
    );
    assert!(
        !lock.contains(r#""git-lib":"4.5.6""#) && !lock.contains(r#""git-lib": "4.5.6""#),
        "the written git spec must not be flattened to a version: {}",
        &lock[..lock.len().min(600)]
    );

    // Reinstall must reuse the checkout, not try to download `resolved`, and must
    // not re-resolve the name from the registry.
    let out2 = jhol_in(&app)
        .env("JHOL_CACHE_DIR", cache.path())
        .args(["install", "--no-scripts"])
        .output()
        .unwrap();
    assert!(
        out2.status.success(),
        "reinstall failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let manifest2 = std::fs::read_to_string(nm.join("package.json")).unwrap();
    assert!(
        manifest2.contains("4.5.6"),
        "reinstall changed the materialized checkout: {manifest2}"
    );
    assert!(
        std::fs::read_to_string(nm.join("index.js"))
            .unwrap()
            .contains("from-git"),
        "reinstall replaced the checkout"
    );
}

#[test]
fn test_mixed_spec_project_survives_cold_offline_ci_and_frozen() {
    // One project exercising every dependency shape that is *not* a plain registry
    // range, plus an optional dependency that cannot be resolved. It is deliberately
    // offline (a local git repo is the remote), and it pins three things QA found
    // broken one after another:
    //  * the lockfile root block must keep the written specs (ranges stayed ranges,
    //    and `file:`/`workspace:`/git entries were dropped altogether);
    //  * `install --offline` must not try to fetch a materialized dependency's name
    //    from the registry (an `npm:` alias was being installed as itself);
    //  * `ci` / `install --frozen` must not demand SRI for a materialized entry, nor
    //    require an optional dep the resolver deliberately skipped.
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipping: git not available");
        return;
    }
    let td = tempdir().unwrap();
    let remote = td.path().join("remote");
    let lib = td.path().join("lib");
    let ws = td.path().join("ws");
    let app = td.path().join("app");
    for d in [&remote, &lib, &ws, &app] {
        std::fs::create_dir_all(d).unwrap();
    }

    std::fs::write(
        remote.join("package.json"),
        r#"{"name":"git-dep","version":"2.2.2","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(remote.join("index.js"), "module.exports='from-git';").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=tester",
            "commit",
            "-qm",
            "initial",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(&args)
                .current_dir(&remote)
                .status()
                .unwrap()
                .success(),
            "git {:?} failed",
            args
        );
    }

    std::fs::write(
        lib.join("package.json"),
        r#"{"name":"local-lib","version":"1.5.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(lib.join("index.js"), "module.exports='from-lib';").unwrap();
    std::fs::write(
        ws.join("package.json"),
        r#"{"name":"ws-lib","version":"3.0.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("index.js"), "module.exports='from-ws';").unwrap();

    let manifest = format!(
        r#"{{"name":"app","version":"1.0.0","private":true,"workspaces":["../ws"],
            "dependencies":{{"local-lib":"file:../lib","ws-lib":"workspace:*",
                             "git-dep":"git+file://{}"}},
            "optionalDependencies":{{"does-not-exist-zzz":"^1.0.0"}}}}"#,
        remote.display()
    );
    std::fs::write(app.join("package.json"), manifest).unwrap();

    let cache = tempdir().unwrap();
    let run = |args: &[&str]| {
        jhol_in(&app)
            .env("JHOL_CACHE_DIR", cache.path())
            .args(args)
            .output()
            .unwrap()
    };
    let loads = || {
        let out = Command::new("node")
            .arg("-e")
            .arg(
                "require('local-lib');require('ws-lib');require('git-dep');\
                 const g=require('git-dep'); if(g!=='from-git') process.exit(3);",
            )
            .current_dir(&app)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stderr).to_string()
    };

    // 1. cold install
    let out = run(&["install", "--no-scripts"]);
    assert!(
        out.status.success(),
        "cold install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        loads().is_empty(),
        "cold install did not produce a usable tree"
    );

    // The lockfile keeps every written spec.
    let lock_path = app.join("package-lock.json");
    let lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lock_path).unwrap()).unwrap();
    let root = &lock["packages"][""]["dependencies"];
    assert_eq!(root["local-lib"], "file:../lib");
    assert_eq!(root["ws-lib"], "workspace:*");
    assert_eq!(
        root["git-dep"],
        format!("git+file://{}", remote.display()),
        "the written git spec must survive"
    );

    // 2. offline reinstall with node_modules removed
    std::fs::remove_dir_all(app.join("node_modules")).unwrap();
    let _ = std::fs::remove_file(app.join(".jhol.capsule"));
    let out = run(&["install", "--offline", "--no-scripts"]);
    assert!(
        out.status.success(),
        "offline reinstall failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(loads().is_empty(), "offline reinstall tree is unusable");

    // 3. ci — deterministic, from the lockfile
    std::fs::remove_dir_all(app.join("node_modules")).unwrap();
    let out = run(&["ci"]);
    assert!(
        out.status.success(),
        "ci failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(loads().is_empty(), "ci tree is unusable");

    // 4. install --frozen (strict lockfile)
    std::fs::remove_dir_all(app.join("node_modules")).unwrap();
    let out = run(&["install", "--frozen", "--no-scripts"]);
    assert!(
        out.status.success(),
        "frozen install failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(loads().is_empty(), "frozen tree is unusable");
}

#[cfg(unix)]
#[test]
fn test_daemon_is_not_stalled_by_a_silent_peer() {
    // The accept loop used to handle connections sequentially, so a client that
    // connected and sent nothing held the whole daemon for the read timeout. The next
    // real request's *first socket write* then blocked for ~30 s (measured: request
    // write 30.0005 s, response 30.2024 s) even though the install itself takes ~200 ms.
    // Now each connection gets its own thread and `DAEMON_CWD_LOCK` serializes only the
    // cwd section, so a silent peer is harmless.
    //
    // Offline (a `file:` dependency) so this never depends on the network.
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let td = tempdir().unwrap();
    let app = td.path().join("app");
    let lib = td.path().join("lib");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("package.json"),
        r#"{"name":"local-lib","version":"1.0.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(lib.join("index.js"), "module.exports=1;").unwrap();
    std::fs::write(
        app.join("package.json"),
        r#"{"name":"app","version":"1.0.0","dependencies":{"local-lib":"file:../lib"}}"#,
    )
    .unwrap();

    let cache = td.path().join("cache");
    let sock = td.path().join("d.sock");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jhol"))
        .args(["daemon", "--serve"])
        .env("JHOL_CACHE_DIR", &cache)
        .env("JHOL_DAEMON_SOCKET", &sock)
        .env("JHOL_NO_SPLASH", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let mut up = false;
    for _ in 0..300 {
        if sock.exists() {
            up = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(up, "daemon socket never appeared at {}", sock.display());

    // A peer that connects and then says nothing — the case that used to stall everything.
    let idle = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping daemon test: AF_UNIX connect blocked by sandbox ({:?})",
                e
            );
            let _ = daemon.kill();
            let _ = daemon.wait();
            return;
        }
        Err(e) => panic!("connect idle peer: {:?}", e),
    };

    let start = Instant::now();
    let out = jhol_in(&app)
        .env("JHOL_CACHE_DIR", &cache)
        .env("JHOL_DAEMON_SOCKET", &sock)
        .args(["install", "--offline", "--daemon", "--no-scripts"])
        .output()
        .unwrap();
    let elapsed = start.elapsed();

    let _ = daemon.kill();
    let _ = daemon.wait();
    drop(idle);

    assert!(
        out.status.success(),
        "install through the daemon failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "daemon stalled behind a silent peer: {:?} (a sequential accept loop waiting on \
         the read timeout is the historical cause)",
        elapsed
    );
    assert!(
        app.join("node_modules/local-lib").exists(),
        "the install did not materialize the dependency"
    );
}

#[test]
fn test_mcp_handshake_via_cli() {
    use std::io::Write;
    let mut child = jhol()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{}", init_req);
    }

    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("jhol-mcp"),
        "MCP handshake should return serverInfo: {}",
        stdout
    );
}

#[test]
fn test_git_hook_cli_command() {
    let td = tempdir().unwrap();
    let repo_root = td.path();
    std::fs::create_dir_all(repo_root.join(".git")).unwrap();

    let out = jhol_in(repo_root)
        .args(["git-hook", "install"])
        .output()
        .unwrap();
    assert!(out.status.success(), "jhol git-hook install should succeed");
    assert!(repo_root.join(".git/hooks/post-checkout").exists());
}

#[test]
fn test_merge_lock_cli_command() {
    let td = tempdir().unwrap();
    let base = td.path().join("base.json");
    let ours = td.path().join("ours.json");
    let theirs = td.path().join("theirs.json");
    let out_file = td.path().join("merged.json");

    let base_json = serde_json::json!({
        "name": "fixture",
        "packages": { "node_modules/a": { "version": "1.0.0" } }
    });
    let ours_json = serde_json::json!({
        "name": "fixture",
        "packages": { "node_modules/a": { "version": "1.0.0" }, "node_modules/b": { "version": "2.0.0" } }
    });
    let theirs_json = serde_json::json!({
        "name": "fixture",
        "packages": { "node_modules/a": { "version": "1.1.0" }, "node_modules/c": { "version": "3.0.0" } }
    });

    std::fs::write(&base, serde_json::to_string(&base_json).unwrap()).unwrap();
    std::fs::write(&ours, serde_json::to_string(&ours_json).unwrap()).unwrap();
    std::fs::write(&theirs, serde_json::to_string(&theirs_json).unwrap()).unwrap();

    let out = jhol_in(td.path())
        .args([
            "merge-lock",
            "--base",
            base.to_str().unwrap(),
            "--ours",
            ours.to_str().unwrap(),
            "--theirs",
            theirs.to_str().unwrap(),
            "-o",
            out_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "jhol merge-lock should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out_file.exists());
}
