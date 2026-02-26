//! Compatibility matrix smoke tests for npm-switch paths.

use std::collections::HashMap;

fn npm_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

#[test]
fn lockfile_nested_scoped_entries_are_normalized_and_stable() {
    let td = tempfile::tempdir().expect("tmp");
    let lockfile = td.path().join("package-lock.json");
    std::fs::write(
        &lockfile,
        r#"{
  "name": "fixture",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "fixture", "version": "1.0.0" },
    "node_modules/b": {
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/b/-/b-1.0.0.tgz"
    },
    "//node_modules/a//node_modules//@scope/pkg//": {
      "version": "2.0.0",
      "resolved": "https://registry.npmjs.org/@scope/pkg/-/pkg-2.0.0.tgz"
    },
    "node_modules/a": {
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/a/-/a-1.0.0.tgz"
    }
  }
}
"#,
    )
    .expect("write lockfile");

    let entries = jhol_core::lockfile::read_npm_lock_install_entries(&lockfile).expect("entries");

    let install_paths: Vec<String> = entries.iter().map(|e| e.install_path.clone()).collect();
    assert_eq!(
        install_paths,
        vec![
            "node_modules/a",
            "node_modules/a/node_modules/@scope/pkg",
            "node_modules/b",
        ]
    );

    let scoped = entries
        .iter()
        .find(|e| e.package == "@scope/pkg")
        .expect("scoped entry");
    assert!(!scoped.top_level);
}


#[test]
fn lockfile_plain_keys_are_supported_for_resolve_and_layout() {
    let td = tempfile::tempdir().expect("tmp");
    let lockfile = td.path().join("package-lock.json");
    std::fs::write(
        &lockfile,
        r#"{
  "name": "fixture",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "fixture", "version": "1.0.0" },
    "lodash": { "version": "4.17.21", "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz" },
    "@scope/pkg": { "version": "2.0.0", "resolved": "https://registry.npmjs.org/@scope/pkg/-/pkg-2.0.0.tgz" }
  }
}
"#,
    )
    .expect("write lockfile");

    let resolved = jhol_core::lockfile::read_lockfile_resolved(&lockfile).expect("resolved");
    assert_eq!(resolved.get("lodash").map(String::as_str), Some("4.17.21"));
    assert_eq!(resolved.get("@scope/pkg").map(String::as_str), Some("2.0.0"));

    let entries = jhol_core::lockfile::read_npm_lock_install_entries(&lockfile).expect("entries");
    assert!(entries.iter().any(|e| e.install_path == "lodash" && e.top_level));
    assert!(entries.iter().any(|e| e.install_path == "@scope/pkg" && e.top_level));
}

#[test]
fn lockfile_optional_platform_dependency_is_filtered() {
    let td = tempfile::tempdir().expect("tmp");
    let lockfile = td.path().join("package-lock.json");
    let current = npm_os();
    let incompatible = format!("!{}", current);

    std::fs::write(
        &lockfile,
        format!(
            r#"{{
  "name": "fixture",
  "lockfileVersion": 3,
  "packages": {{
    "": {{ "name": "fixture", "version": "1.0.0" }},
    "node_modules/required-native": {{
      "version": "1.0.0",
      "resolved": "https://registry.npmjs.org/required-native/-/required-native-1.0.0.tgz",
      "os": ["{current}"]
    }},
    "node_modules/optional-native": {{
      "version": "1.0.0",
      "optional": true,
      "resolved": "https://registry.npmjs.org/optional-native/-/optional-native-1.0.0.tgz",
      "os": ["{incompatible}"]
    }}
  }}
}}
"#
        ),
    )
    .expect("write lockfile");

    let entries = jhol_core::lockfile::read_npm_lock_install_entries(&lockfile).expect("entries");
    let names: Vec<String> = entries.into_iter().map(|e| e.package).collect();

    assert!(names.iter().any(|n| n == "required-native"));
    assert!(!names.iter().any(|n| n == "optional-native"));
}

#[test]
fn workspace_bin_resolution_walks_up_to_root() {
    let td = tempfile::tempdir().expect("tmp");
    let root = td.path();
    let leaf = root.join("packages").join("app").join("src");
    std::fs::create_dir_all(&leaf).expect("mkdir leaf");

    let bin_dir = root.join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
    let bin_name = if cfg!(windows) { "mytool.cmd" } else { "mytool" };
    std::fs::write(bin_dir.join(bin_name), "echo ok").expect("write bin");

    let found = jhol_core::find_binary_in_node_modules("mytool", &leaf).expect("find");
    assert!(found.ends_with(bin_name));
}

#[test]
fn resolve_deps_for_install_is_deterministic() {
    let mut deps = HashMap::new();
    deps.insert("zlib".to_string(), "^1.0.0".to_string());
    deps.insert("axios".to_string(), "^1.0.0".to_string());
    deps.insert("chalk".to_string(), "^5.0.0".to_string());

    let out = jhol_core::lockfile::resolve_deps_for_install(&deps, None);
    assert_eq!(
        out,
        vec![
            "axios@^1.0.0".to_string(),
            "chalk@^5.0.0".to_string(),
            "zlib@^1.0.0".to_string()
        ]
    );
}


#[test]
fn resolve_install_prefers_shrinkwrap_over_package_lock() {
    let td = tempfile::tempdir().expect("tmp");
    std::fs::write(
        td.path().join("package.json"),
        r#"{
  "name": "fixture",
  "version": "1.0.0",
  "dependencies": { "left-pad": "^1.3.0" }
}
"#,
    )
    .expect("package json");

    std::fs::write(
        td.path().join("package-lock.json"),
        r#"{
  "name": "fixture",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "fixture", "version": "1.0.0" },
    "node_modules/left-pad": { "version": "1.3.0" }
  }
}
"#,
    )
    .expect("package-lock");

    std::fs::write(
        td.path().join("npm-shrinkwrap.json"),
        r#"{
  "name": "fixture",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "fixture", "version": "1.0.0" },
    "node_modules/left-pad": { "version": "1.1.0" }
  }
}
"#,
    )
    .expect("shrinkwrap");

    let prev = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(td.path()).expect("chdir fixture");
    let specs = jhol_core::resolve_install_from_package_json(false).expect("resolve from lockfile");
    std::env::set_current_dir(prev).expect("restore cwd");

    assert!(specs.iter().any(|s| s == "left-pad@1.1.0"));
    assert!(!specs.iter().any(|s| s == "left-pad@1.3.0"));
}

#[test]
fn read_all_resolved_specs_includes_scoped_transitive() {
    let td = tempfile::tempdir().expect("tmp");
    std::fs::write(
        td.path().join("package-lock.json"),
        r#"{
  "name": "fixture",
  "lockfileVersion": 3,
  "packages": {
    "": { "name": "fixture", "version": "1.0.0" },
    "node_modules/axios": { "version": "1.6.0" },
    "node_modules/axios/node_modules/follow-redirects": { "version": "1.15.6" },
    "//node_modules/axios//node_modules//@scope/pkg//": { "version": "2.0.0" }
  }
}
"#,
    )
    .expect("package-lock");

    let specs = jhol_core::lockfile::read_all_resolved_specs_from_dir(td.path()).expect("specs");
    assert!(specs.iter().any(|s| s == "axios@1.6.0"));
    assert!(specs.iter().any(|s| s == "follow-redirects@1.15.6"));
    assert!(specs.iter().any(|s| s == "@scope/pkg@2.0.0"));
}
