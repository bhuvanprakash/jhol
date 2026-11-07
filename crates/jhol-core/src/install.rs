use crate::backend::{self, Backend};
use crate::bin_links;

mod lockfile_patch;
use crate::error_handling::JholError;
use crate::lockfile;
use crate::materialize;
use crate::registry;
use crate::selective_extract; // JHOL Selective Extraction
use crate::utils::{self, NPM_SHOW_TIMEOUT_SECS};
use lockfile_patch::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

// NOTE: these were lowered (`cores*2` clamped `4..=12`, and `2..=6` for cache
// installs) in an attempt to bound memory on small machines; together with the
// HTTP/pool reductions that deadlocked plain installs, so the original sizing is
// restored. Any reduction must be validated with a real `jhol install` first.
fn download_concurrency() -> usize {
    crate::utils::cached_env_var("JHOL_DOWNLOAD_CONCURRENCY")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 128))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| (n.get() * 4).clamp(32, 128))
                .unwrap_or(64)
        })
}

fn cache_install_concurrency() -> usize {
    crate::utils::cached_env_var("JHOL_CACHE_INSTALL_CONCURRENCY")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 32))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().clamp(4, 32))
                .unwrap_or(8)
        })
}

fn worker_pool_sequential_threshold() -> usize {
    crate::utils::cached_env_var("JHOL_WORKER_POOL_SEQUENTIAL_THRESHOLD")
        .and_then(|v| v.parse::<usize>().ok())
        .map(|n| n.clamp(1, 64))
        .unwrap_or(4)
}

fn use_legacy_chunk_scheduler() -> bool {
    crate::utils::cached_env_var("JHOL_LEGACY_CHUNK_SCHEDULER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn run_worker_pool<I, O, F>(
    items: Vec<I>,
    concurrency: usize,
    sequential_threshold: usize,
    job: F,
) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    // Delegate to shared global pool (zero spawn overhead, persistent 64-thread pool)
    crate::pool::parallel_map(items, concurrency, sequential_threshold, job)
}

/// The install's shared metadata plane: packuments the flat expansion fetched are
/// kept here (as `Arc`s, so they are *shared*, never copied) and reused by every
/// later reader in the same run — the node-keyed layout builder and any second
/// resolution both read it.
///
/// This used to be enabled only under the daemon, which left a plain
/// `jhol install` with an empty snapshot. That is why the conflict path had to
/// re-resolve the manifest from scratch (a second full metadata pass) instead of
/// re-nesting what it had already fetched. Enabling it costs one map entry plus
/// an `Arc` clone per package — the packuments themselves already exist — so the
/// default is on; `JHOL_NO_TRANSITIVE_CACHE=1` restores the old behaviour.
fn transitive_cache_enabled() -> bool {
    !std::env::var("JHOL_NO_TRANSITIVE_CACHE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TransitiveExpansionCacheEntry {
    packages: Vec<String>,
    optional_variants: Vec<OptionalVariant>,
}

fn transitive_memo_cache() -> &'static Mutex<HashMap<String, TransitiveExpansionCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, TransitiveExpansionCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn trans_expansion_cache_dir() -> Result<std::path::PathBuf, String> {
    let cache_dir = crate::utils::get_cache_dir();
    let dir = std::path::PathBuf::from(cache_dir).join("expansion_cache");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create expansion cache dir: {}", e))?;
    Ok(dir)
}

fn trans_expansion_cache_key_hash(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

fn trans_expansion_cache_key(
    registry_identity: &str,
    max_depth: usize,
    packages: &[&str],
    offline: bool,
    registry_offline: bool,
    overrides: &HashMap<String, String>,
    optional_dependencies: &HashSet<String>,
) -> String {
    let mut specs: Vec<String> = packages.iter().map(|p| (*p).to_string()).collect();
    specs.sort();
    let mut overrides: Vec<(String, String)> = overrides
        .iter()
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect();
    overrides.sort();
    let mut optional_dependencies: Vec<String> = optional_dependencies.iter().cloned().collect();
    optional_dependencies.sort();

    // Version the serialized identity; older cache entries omitted inputs that
    // affect package selection, scoped registry/auth identity, and optional deps.
    serde_json::to_string(&(
        4u8,
        registry_identity,
        std::env::var("JHOL_STRICT_PEER_DEPS")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
        max_depth,
        offline,
        registry_offline,
        specs,
        overrides,
        optional_dependencies,
        current_npm_os(),
        current_npm_cpu(),
        crate::utils::target_libc(),
    ))
    .expect("expansion cache key tuple is serializable")
}

fn save_trans_expansion_cache(cache_key: &str, result: &TransitiveExpansionCacheEntry) {
    if let Ok(dir) = trans_expansion_cache_dir() {
        let hash = trans_expansion_cache_key_hash(cache_key);
        let bin_path = dir.join(format!("{}.bin", hash));
        if let Ok(bytes) = bincode::serialize(result) {
            let _ = std::fs::write(&bin_path, bytes);
        }
    }
}

/// A flat tree keeps exactly one version per name, so it cannot represent two
/// dependents wanting incompatible ranges. When the expansion selected a version
/// that some *transitive* dependent's declared range rejects, the flat tree is
/// silently wrong: `cliui@8.0.1` requires `string-width ^4.2.0`, but if the root
/// asks for `^5` the flat tree hands `cliui` 5.1.2 — and `cliui`'s own
/// `strip-ansi ^6.0.1` is likewise replaced by a newer major. The caller must
/// switch to the node-keyed layout rather than install versions nobody asked for.
///
/// Only `dependencies` participate: optional deps can be legitimately absent
/// (platform filtering) and peers have their own lenient resolution, so treating
/// either as a conflict would nest needlessly.
fn flat_constraint_conflict(
    selected: &HashMap<String, String>,
    metadata: &HashMap<String, std::sync::Arc<serde_json::Value>>,
) -> Option<(String, String, String, String)> {
    for (name, version) in selected {
        let Some(meta) = metadata.get(name) else {
            continue;
        };
        for (dep, range) in crate::registry::get_version_required_dependencies(meta, version) {
            let resolved = selected.get(&dep);
            if !resolved.is_some_and(|v| crate::registry::version_satisfies(&range, v)) {
                return Some((
                    name.clone(),
                    dep,
                    range,
                    resolved.cloned().unwrap_or_else(|| "<missing>".to_string()),
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn transitive_expansion_cache_key_tracks_resolution_inputs() {
        let registry_identity = "registry-identity-a";
        let packages = ["left@^1.0.0", "right@~2.0.0"];
        let overrides = HashMap::from([("shared".to_string(), "3.4.0".to_string())]);
        let optional = HashSet::from(["native-addon".to_string()]);
        let key = super::trans_expansion_cache_key(
            registry_identity,
            32,
            &packages,
            false,
            false,
            &overrides,
            &optional,
        );

        assert_eq!(
            key,
            super::trans_expansion_cache_key(
                registry_identity,
                32,
                &["right@~2.0.0", "left@^1.0.0"],
                false,
                false,
                &overrides,
                &optional,
            )
        );
        assert_ne!(
            key,
            super::trans_expansion_cache_key(
                registry_identity,
                32,
                &packages,
                false,
                false,
                &HashMap::from([("shared".to_string(), "4.0.0".to_string())]),
                &optional,
            )
        );
        assert_ne!(
            key,
            super::trans_expansion_cache_key(
                registry_identity,
                32,
                &packages,
                false,
                false,
                &overrides,
                &HashSet::new(),
            )
        );
        assert_ne!(
            key,
            super::trans_expansion_cache_key(
                registry_identity,
                32,
                &packages,
                true,
                false,
                &overrides,
                &optional,
            )
        );
        assert_ne!(
            key,
            super::trans_expansion_cache_key(
                registry_identity,
                32,
                &packages,
                false,
                true,
                &overrides,
                &optional,
            )
        );
        assert_ne!(
            key,
            super::trans_expansion_cache_key(
                registry_identity,
                64,
                &packages,
                false,
                false,
                &overrides,
                &optional,
            )
        );
        assert_ne!(
            key,
            super::trans_expansion_cache_key(
                "registry-identity-b",
                32,
                &packages,
                false,
                false,
                &overrides,
                &optional,
            )
        );
    }

    #[test]
    fn transitive_expansion_identity_tracks_scoped_origin_and_auth() {
        fn write_npmrc(path: &std::path::Path, scoped_registry: &str, scoped_token: &str) {
            std::fs::write(
                path.join(".npmrc"),
                format!(
                    "registry=https://default.example/\n//default.example/:_authToken=default-token\n@private:registry={scoped_registry}/\n//{}/:_authToken={scoped_token}\n",
                    scoped_registry.trim_start_matches("https://")
                ),
            )
            .unwrap();
        }

        let first = tempfile::tempdir().unwrap();
        let other_token = tempfile::tempdir().unwrap();
        let other_origin = tempfile::tempdir().unwrap();
        write_npmrc(first.path(), "https://private-a.example", "token-a");
        write_npmrc(other_token.path(), "https://private-a.example", "token-b");
        write_npmrc(other_origin.path(), "https://private-b.example", "token-a");

        let baseline = crate::registry::registry_configuration_fingerprint(first.path());
        assert!(!baseline.contains("token-a"));
        assert_ne!(
            baseline,
            crate::registry::registry_configuration_fingerprint(other_token.path()),
            "scoped credentials must change the expansion cache identity"
        );
        assert_ne!(
            baseline,
            crate::registry::registry_configuration_fingerprint(other_origin.path()),
            "scoped registry origin must change the expansion cache identity"
        );
    }

    #[test]
    fn transitive_expansion_cache_entry_preserves_optional_variants() {
        let metadata = serde_json::json!({
            "versions": {
                "1.2.3": {
                    "os": ["darwin"],
                    "cpu": ["arm64"],
                    "dist": { "tarball": "https://registry.example/native-addon.tgz" }
                }
            }
        });
        let variant = super::optional_variant_from_meta("native-addon", "1.2.3", &metadata);
        let entry = super::TransitiveExpansionCacheEntry {
            packages: vec!["native-addon@1.2.3".to_string()],
            optional_variants: vec![variant],
        };
        let bytes = serde_json::to_vec(&entry).expect("cache entry serializes");
        let decoded: super::TransitiveExpansionCacheEntry =
            serde_json::from_slice(&bytes).expect("cache entry deserializes");

        assert_eq!(decoded.packages, entry.packages);
        assert_eq!(decoded.optional_variants, entry.optional_variants);
    }

    /// The roots union must never re-add a materialized special by bare name: doing
    /// so sent `github:isaacs/rimraf#v3.0.2` to the registry as `rimraf` (`*`) and
    /// installed the registry 6.1.3 instead of the checkout.
    #[test]
    fn union_registry_roots_excludes_specials_and_dedupes() {
        let specials: HashSet<String> = ["rimraf".to_string(), "local-lib".to_string()]
            .into_iter()
            .collect();
        let registry = vec!["glob@^7.1.3".to_string()];
        let manifest = [
            "rimraf".to_string(),    // git special -> must be skipped
            "local-lib".to_string(), // file: special -> must be skipped
            "glob".to_string(),      // already requested -> dedupe
            "ms".to_string(),        // plain dep -> must be added
        ];
        let out = union_registry_roots(&registry, manifest.into_iter(), &specials);
        assert_eq!(out, vec!["glob@^7.1.3".to_string(), "ms".to_string()]);
    }

    /// Scoped names keep working: `@scope/pkg` must not be mistaken for `@scope`
    /// plus a version when deduping.
    #[test]
    fn union_registry_roots_handles_scoped_names() {
        let specials: HashSet<String> = HashSet::new();
        let registry = vec!["@scope/pkg@^1.0.0".to_string()];
        let manifest = ["@scope/pkg".to_string(), "@scope/other".to_string()];
        let out = union_registry_roots(&registry, manifest.into_iter(), &specials);
        assert_eq!(
            out,
            vec!["@scope/pkg@^1.0.0".to_string(), "@scope/other".to_string()]
        );
    }

    /// `base_name` must split at the *first* separator after a scope, not the
    /// last: an `npm:` alias's spec contains its own `@` (`npm:real@range`).
    #[test]
    fn base_name_handles_npm_alias_specs() {
        assert_eq!(base_name("lodash"), "lodash");
        assert_eq!(base_name("lodash@4.17.21"), "lodash");
        assert_eq!(base_name("@scope/pkg"), "@scope/pkg");
        assert_eq!(base_name("@scope/pkg@1.0.0"), "@scope/pkg");
        assert_eq!(base_name("my-lodash@npm:lodash@4.17.21"), "my-lodash");
        assert_eq!(base_name("@scope/sem@npm:semver@7.6.0"), "@scope/sem");
    }

    /// Regression: `<alias>@npm:<real>@<range>` was compared against
    /// `special_names` *as the raw spec*, never matched, and leaked into
    /// resolution as a package to fetch (`Failed to fetch metadata for
    /// my-lodash@npm:lodash: HTTP 404`).
    #[test]
    fn union_registry_roots_skips_npm_aliases_by_name() {
        let specials: HashSet<String> = ["my-lodash".to_string()].into_iter().collect();
        let registry = vec!["lodash@4.17.21".to_string()];
        let manifest = [
            "my-lodash@npm:lodash@4.17.21".to_string(),
            "ms@^2.1.3".to_string(),
        ];
        let out = union_registry_roots(&registry, manifest.into_iter(), &specials);
        assert_eq!(
            out,
            vec!["lodash@4.17.21".to_string(), "ms@^2.1.3".to_string()],
            "the alias is skipped; the plain dep is added"
        );
    }

    fn meta_with(deps: &[(&str, &str)]) -> std::sync::Arc<serde_json::Value> {
        let deps: serde_json::Map<String, serde_json::Value> = deps
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_string(),
                    serde_json::Value::String((*v).to_string()),
                )
            })
            .collect();
        std::sync::Arc::new(serde_json::json!({
            "versions": { "8.0.1": { "dependencies": deps } }
        }))
    }

    /// The exact E98 case: `cliui` wants `string-width ^4.2.0` but the flat tree
    /// selected 5.1.2. This must be reported so the caller nests instead.
    #[test]
    fn flat_conflict_detected_when_transitive_range_unsatisfied() {
        let mut selected = HashMap::from([
            ("cliui".to_string(), "8.0.1".to_string()),
            ("string-width".to_string(), "5.1.2".to_string()),
        ]);
        let metadata = HashMap::from([(
            "cliui".to_string(),
            meta_with(&[("string-width", "^4.2.0")]),
        )]);
        let (parent, dep, range, version) =
            super::flat_constraint_conflict(&selected, &metadata).expect("conflict expected");
        assert_eq!((parent.as_str(), dep.as_str()), ("cliui", "string-width"));
        assert_eq!((range.as_str(), version.as_str()), ("^4.2.0", "5.1.2"));
    }

    /// No needless nesting: satisfied ranges pass, a dependency absent from the
    /// flat tree (platform-filtered optional) is skipped, and unknown metadata
    /// must not produce a false trigger.
    #[test]
    fn flat_conflict_absent_when_satisfied_or_unknown() {
        let selected = HashMap::from([
            ("cliui".to_string(), "8.0.1".to_string()),
            ("string-width".to_string(), "5.1.2".to_string()),
        ]);
        let mut cliui =
            (*meta_with(&[("string-width", "^5.0.0"), ("strip-ansi", "^6.0.1")])).clone();
        cliui["versions"]["8.0.1"]["optionalDependencies"] =
            serde_json::json!({"strip-ansi":"^6.0.1"});
        let metadata = HashMap::from([("cliui".to_string(), std::sync::Arc::new(cliui))]);
        assert!(super::flat_constraint_conflict(&selected, &metadata).is_none());
    }

    #[test]
    fn flat_conflict_detected_when_required_child_missing() {
        let selected = HashMap::from([("cliui".into(), "8.0.1".into())]);
        let metadata = HashMap::from([("cliui".into(), meta_with(&[("string-width", "^5.0.0")]))]);
        let conflict = super::flat_constraint_conflict(&selected, &metadata).unwrap();
        assert_eq!(conflict.1, "string-width");
        assert_eq!(conflict.3, "<missing>");
    }

    /// The lockfile patch records local deps with the *written* spec and aliases
    /// with the real package's identity — this is the code that used to leak an

    /// A workspace package's own dependencies must be discovered: its `workspace:`
    /// dep gets linked and its registry deps join the resolution. Only the root
    /// manifest used to be read, so both were silently dropped.
    #[test]
    fn materialize_special_deps_walks_workspace_packages() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("packages/a")).unwrap();
        std::fs::create_dir_all(root.join("packages/b")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","private":true,"version":"1.0.0","workspaces":["packages/*"],"dependencies":{"a":"workspace:*"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("packages/a/package.json"),
            r#"{"name":"a","version":"1.0.0","dependencies":{"b":"workspace:*","ms":"^2.1.3"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("packages/b/package.json"),
            r#"{"name":"b","version":"2.0.0"}"#,
        )
        .unwrap();

        let nm = root.join("node_modules");
        let result = crate::spec_grammar::materialize_special_deps(root, &nm);

        // The transitive `workspace:*` package b is discovered and linked, not just a.
        let names: Vec<&str> = result.specials.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"a"), "specials: {names:?}");
        assert!(names.contains(&"b"), "specials: {names:?}");
        assert!(nm.join("a").is_symlink() || nm.join("a").exists());
        assert!(nm.join("b").exists(), "b must be linked at the root");

        // The registry dep declared *by* the workspace package is surfaced.
        assert!(
            result.local_registry_deps.iter().any(|s| s == "ms@^2.1.3"),
            "local_registry_deps: {:?}",
            result.local_registry_deps
        );
    }

    use super::*;

    #[test]
    fn lockfile_driven_install_prefers_store_filling_materializer() {
        let options = InstallOptions {
            from_lockfile: true,
            ..InstallOptions::default()
        };

        assert!(prefer_lockfile_materialization(&options));
    }

    #[test]
    fn non_lockfile_install_does_not_force_lockfile_materializer() {
        assert!(!prefer_lockfile_materialization(&InstallOptions::default()));
    }

    #[test]
    fn constraints_satisfied_treats_bare_version_as_exact_pin() {
        let requested = vec!["4.17.21".to_string()];
        let higher = semver::Version::parse("4.18.1").unwrap();
        let exact = semver::Version::parse("4.17.21").unwrap();

        assert!(!constraints_satisfied(&higher, &requested));
        assert!(constraints_satisfied(&exact, &requested));
    }

    #[test]
    fn parse_overrides_reads_npm_and_yarn_forms() {
        let pj = serde_json::json!({
            "overrides": {
                "ms": "2.0.0",
                "strip-ansi": { ".": "7.0.0", "ansi-regex": "5.0.1" }
            },
            "resolutions": { "is-number": "7.0.0" }
        });
        let ov = crate::lockfile::parse_overrides(&pj);
        assert_eq!(ov.get("ms").map(String::as_str), Some("2.0.0"));
        // Nested form pins the package itself via `"."` …
        assert_eq!(ov.get("strip-ansi").map(String::as_str), Some("7.0.0"));
        // … and its named children (flattened; see `parse_overrides` docs).
        assert_eq!(ov.get("ansi-regex").map(String::as_str), Some("5.0.1"));
        // yarn `resolutions` are honoured too.
        assert_eq!(ov.get("is-number").map(String::as_str), Some("7.0.0"));
        assert_eq!(ov.len(), 4);
    }

    #[test]
    fn parse_overrides_ignores_non_string_values() {
        let pj = serde_json::json!({ "overrides": { "a": 5, "b": ["x"], "c": null } });
        assert!(crate::lockfile::parse_overrides(&pj).is_empty());
    }

    #[test]
    fn resolve_preferring_peers_prefers_peer_satisfying() {
        let meta = serde_json::json!({"versions": {"17.0.2": {}, "18.2.5": {}, "18.3.1": {}}});
        let all = vec![">=17.0.0".to_string(), "~18.2.0".to_string()];
        let peers = vec!["~18.2.0".to_string()];
        assert_eq!(
            resolve_preferring_peers(&meta, &all, &peers),
            Some("18.2.5".to_string())
        );
    }

    #[test]
    fn resolve_preferring_peers_falls_back_when_peer_conflicts() {
        let meta = serde_json::json!({"versions": {"17.0.2": {}, "18.3.1": {}}});
        // An exact dependency pin plus an incompatible peer range has no common
        // solution; the dependency-valid version must still be returned.
        let all = vec!["17.0.2".to_string(), "^18.0.0".to_string()];
        let peers = vec!["^18.0.0".to_string()];
        assert_eq!(
            resolve_preferring_peers(&meta, &all, &peers),
            Some("17.0.2".to_string())
        );
    }

    #[test]
    fn resolve_preferring_peers_returns_none_when_hard_unsatisfiable() {
        let meta = serde_json::json!({"versions": {"1.0.0": {}}});
        assert_eq!(
            resolve_preferring_peers(&meta, &["^2.0.0".to_string()], &[]),
            None
        );
    }

    #[test]
    fn load_installed_package_carries_peer_metadata() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("package.json");
        std::fs::write(
            &p,
            r#"{
                "name": "react-dom",
                "version": "18.3.1",
                "peerDependencies": { "react": "^18.2.0" },
                "peerDependenciesMeta": { "react": { "optional": false } }
            }"#,
        )
        .unwrap();
        // Use the live loader (the thin `insert_installed_package` wrapper was
        // dead: production goes through `read_all_installed_packages`, which calls
        // this directly).
        let (name, pkg) = load_installed_package(&p).expect("loaded");
        assert_eq!(name, "react-dom");
        assert_eq!(pkg.peer_dependencies["react"], "^18.2.0");
        assert_eq!(
            pkg.peer_dependencies_meta["react"]["optional"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn existing_lockfile_entries_reads_registry_integrity() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join("package-lock.json"),
            r#"{
              "lockfileVersion": 3,
              "packages": {
                "": { "name": "x", "version": "1.0.0" },
                "node_modules/lodash": {
                  "version": "4.17.21",
                  "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz",
                  "integrity": "sha512-abc"
                },
                "node_modules/@scope/pkg": { "version": "2.0.0", "integrity": "sha512-xyz" },
                "node_modules/a/node_modules/b": { "version": "3.0.0", "integrity": "sha512-nested" }
              }
            }"#,
        )
        .unwrap();

        let map = existing_lockfile_entries(&td.path().join("node_modules"));
        assert_eq!(map["lodash"].version, "4.17.21");
        assert_eq!(map["lodash"].integrity.as_deref(), Some("sha512-abc"));
        assert_eq!(
            map["lodash"].resolved.as_deref(),
            Some("https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz")
        );
        assert!(!map.contains_key("a/node_modules/b"));
    }

    #[test]
    fn store_hash_for_package_uses_store_index_when_not_a_symlink() {
        let td = tempfile::tempdir().unwrap();
        let install = td.path().join("node_modules").join("ms");
        std::fs::create_dir_all(&install).unwrap();
        // Real directory (clonefile/copy mode) — no symlink to read.
        std::fs::write(
            install.join("package.json"),
            r#"{"name":"ms","version":"2.1.3"}"#,
        )
        .unwrap();
        let mut idx = std::collections::HashMap::new();
        idx.insert("ms@2.1.3".to_string(), "deadbeef".to_string());
        assert_eq!(
            store_hash_for_package(&install, "ms", "2.1.3", &idx).as_deref(),
            Some("deadbeef")
        );
        assert_eq!(store_hash_for_package(&install, "ms", "2.0.0", &idx), None);
    }

    #[test]
    fn test_exact_version_from_spec_cases() {
        assert_eq!(exact_version_from_spec("lodash@4.17.21"), Some("4.17.21"));
        assert_eq!(
            exact_version_from_spec("@swc/helpers@0.5.23"),
            Some("0.5.23")
        );
        assert_eq!(
            exact_version_from_spec("@scope/pkg@1.2.3-alpha.1"),
            Some("1.2.3-alpha.1")
        );
        assert_eq!(exact_version_from_spec("lodash@^4.17.21"), None);
        assert_eq!(exact_version_from_spec("lodash@~4.17.21"), None);
        assert_eq!(exact_version_from_spec("lodash@latest"), None);
        assert_eq!(exact_version_from_spec("lodash@*"), None);
        assert_eq!(exact_version_from_spec("lodash"), None);
        assert_eq!(exact_version_from_spec("@scope/pkg"), None);
    }

    #[test]
    fn test_can_reuse_lockfile_for_requested_version_mismatch() {
        let td = tempfile::tempdir().unwrap();
        let pj_path = td.path().join("package.json");
        let lf_path = td.path().join("package-lock.json");

        std::fs::write(
            &pj_path,
            r#"{"name":"test-app","version":"1.0.0","dependencies":{"@swc/helpers":"0.5.15"}}"#,
        )
        .unwrap();

        std::fs::write(
            &lf_path,
            r#"{
              "name": "test-app",
              "version": "1.0.0",
              "lockfileVersion": 3,
              "packages": {
                "": {
                  "name": "test-app",
                  "version": "1.0.0",
                  "dependencies": {
                    "@swc/helpers": "0.5.15"
                  }
                },
                "node_modules/@swc/helpers": {
                  "version": "0.5.15",
                  "resolved": "https://registry.npmjs.org/@swc/helpers/-/helpers-0.5.15.tgz"
                }
              }
            }"#,
        )
        .unwrap();

        // Exact mismatch: requesting @swc/helpers@0.5.23 must NOT reuse the 0.5.15 lockfile
        let reuse_mismatch =
            can_reuse_lockfile_for_requested_in(td.path(), &["@swc/helpers@0.5.23"]);
        assert!(
            reuse_mismatch.is_none(),
            "lockfile reuse must be rejected when exact requested version differs from locked"
        );

        // Exact match: requesting @swc/helpers@0.5.15 can be reused
        let reuse_match = can_reuse_lockfile_for_requested_in(td.path(), &["@swc/helpers@0.5.15"]);
        assert!(
            reuse_match.is_some(),
            "lockfile reuse should succeed when exact requested version matches locked"
        );
        let specs = reuse_match.unwrap();
        assert!(specs.iter().any(|s| s.starts_with("@swc/helpers@0.5.15")));
    }
}

fn load_trans_expansion_cache(cache_key: &str) -> Option<TransitiveExpansionCacheEntry> {
    if let Ok(dir) = trans_expansion_cache_dir() {
        let hash = trans_expansion_cache_key_hash(cache_key);
        let bin_path = dir.join(format!("{}.bin", hash));
        if let Ok(bytes) = std::fs::read(&bin_path) {
            if let Ok(entry) = bincode::deserialize::<TransitiveExpansionCacheEntry>(&bytes) {
                return Some(entry);
            }
        }
        let json_path = dir.join(format!("{}.json", hash));
        if let Ok(content) = std::fs::read_to_string(json_path) {
            return serde_json::from_str(&content).ok();
        }
    }
    None
}

// ── Single-pass decode cache: extract → node_modules from cached bytes ────
// get_bundled_tarball caches the decoded gzip tarball bytes per-package in a
// process-global Mutex<HashMap>.  When the install phase needs to materialise
// a bundled package, it checks this cache: on hit, it extracts directly from
// the in-memory bytes to node_modules (zero decompression, zero store I/O).
// On miss (e.g. cross-project), it falls through to the normal store path.

/// Install a bundled package's tarball directly from the decode cache,
/// bypassing the store entirely.  Returns true on success (cache hit + extract).
/// Also writes to the store as a side-effect so future runs hit the index.
fn try_install_bundled_from_cache(pkg: &str, base: &str, node_modules: &Path) -> bool {
    let cache = crate::registry::get_bundled_cache();
    let tgz = match cache.lock().unwrap().get(pkg) {
        Some(b) => b.clone(),
        None => return false,
    };
    let install_path = node_modules.join(base);
    if install_path.exists() {
        let _ = std::fs::remove_dir_all(&install_path);
    }
    if let Some(parent) = install_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    use flate2::read::GzDecoder;
    use tar::Archive;
    let decoder = GzDecoder::new(std::io::Cursor::new(&tgz));
    let mut archive = Archive::new(decoder);
    if archive.unpack(&install_path).is_err() {
        return false;
    }
    // Also write to store for future cross-project runs
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(&tgz));
    let store_dir = crate::content::store_dir(std::path::Path::new(&crate::utils::get_cache_dir()));
    let store_file =
        crate::content::tarball_path(std::path::Path::new(&crate::utils::get_cache_dir()), &hash);
    if !store_file.exists() {
        let _ = std::fs::create_dir_all(&store_dir);
        let _ = std::fs::write(&store_file, &tgz);
        let mut index = crate::utils::read_store_index();
        index.insert(pkg.to_string(), hash);
        let _ = crate::utils::write_store_index(&index);
    }
    true
}

fn transitive_metadata_snapshot(
) -> &'static Mutex<HashMap<String, std::sync::Arc<serde_json::Value>>> {
    // `Arc` values so a packument is allocated once and shared by the snapshot, the
    // per-run metadata cache and every read. Holding plain `Value`s made each
    // packument exist 2-3x (snapshot + cache + a deep clone per read).
    static CACHE: OnceLock<Mutex<HashMap<String, std::sync::Arc<serde_json::Value>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn install_profile_enabled() -> bool {
    std::env::var("JHOL_PROFILE_INSTALL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

struct InstallProfiler {
    enabled: bool,
    start: Instant,
    last: Instant,
}

impl InstallProfiler {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            enabled: install_profile_enabled(),
            start: now,
            last: now,
        }
    }

    /// Total elapsed time since this install started.
    fn total_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn mark(&mut self, stage: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let delta = now.duration_since(self.last).as_millis();
        let total = now.duration_since(self.start).as_millis();
        // Always write to stderr directly, bypassing the quiet/log-file check.
        eprintln!(
            "[jhol-profile] stage={} delta_ms={} total_ms={}",
            stage, delta, total
        );
        self.last = now;
    }
}

#[derive(Clone)]
struct ResolvedFetch {
    pkg: String,
    url: String,
    integrity: Option<String>,
    version: String,
}

// Exact selections keep the registry's transfer descriptor across discovery,
// prefetch, foreground download and lock writing. A version hint alone is not
// evidence of an archive URL or integrity.
fn cached_resolved_fetch(pkg: &str) -> Option<ResolvedFetch> {
    let version = exact_version_from_spec(pkg)?;
    let base = base_name(pkg);
    let meta = transitive_cache_enabled()
        .then(|| {
            transitive_metadata_snapshot()
                .lock()
                .ok()?
                .get(base)
                .cloned()
        })
        .flatten()
        .or_else(|| registry::get_cached_packument(base).map(Arc::new))?;
    Some(ResolvedFetch {
        pkg: pkg.to_string(),
        url: registry::get_tarball_url(&meta, version)?,
        integrity: registry::get_integrity_for_version(&meta, version),
        version: version.to_string(),
    })
}

trait ColdResolveStrategy {
    fn resolve(&self, to_fetch: &[String], npm_fallback: &mut Vec<String>) -> Vec<ResolvedFetch>;
}

struct ManifestThenPackumentStrategy;

impl ColdResolveStrategy for ManifestThenPackumentStrategy {
    fn resolve(&self, to_fetch: &[String], npm_fallback: &mut Vec<String>) -> Vec<ResolvedFetch> {
        let mut resolved_work = Vec::new();
        let mut manifest_requests: Vec<(String, String, String)> =
            Vec::with_capacity(to_fetch.len());
        let mut request_meta: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::with_capacity(to_fetch.len());

        for (idx, pkg) in to_fetch.iter().enumerate() {
            let base = base_name(pkg).to_string();
            let version_req = if pkg.contains('@') && !pkg.starts_with('@') {
                pkg.split_once('@')
                    .map(|x| x.1)
                    .unwrap_or("latest")
                    .trim()
                    .to_string()
            } else if pkg.starts_with('@') {
                let idx = pkg.rfind('@').unwrap_or(0);
                if idx > 0 {
                    pkg[idx + 1..].trim().to_string()
                } else {
                    "latest".to_string()
                }
            } else {
                "latest".to_string()
            };

            if let Some(item) = cached_resolved_fetch(pkg) {
                resolved_work.push(item);
                continue;
            }

            let req_id = idx.to_string();
            manifest_requests.push((req_id.clone(), base.clone(), version_req.clone()));
            request_meta.insert(req_id, (pkg.clone(), base, version_req));
        }

        let mut pending_packument: Vec<(String, String, String)> = Vec::new();
        let mut manifest_results =
            registry::parallel_resolve_tarballs_via_manifest(&manifest_requests);
        manifest_results.sort_by(|a, b| a.0.cmp(&b.0));

        for (req_id, result) in manifest_results {
            let Some((pkg, base, version_req)) = request_meta.remove(&req_id) else {
                continue;
            };

            match result {
                Ok(Some((version, url, integrity))) => {
                    registry::remember_lattice_for_current_identity(&base, &version_req, &version);
                    resolved_work.push(ResolvedFetch {
                        pkg,
                        url,
                        integrity,
                        version,
                    });
                }
                Ok(None) | Err(_) => {
                    pending_packument.push((pkg, base, version_req));
                }
            }
        }

        if !pending_packument.is_empty() {
            // Soul Path: check zero-copy metadata store first (avoids network when
            // another project already fetched this packument).
            let mut zero_copy_hits: Vec<(String, ResolvedFetch)> = Vec::new();
            let mut still_needed: Vec<(String, String, String)> = Vec::new();
            for (pkg, base, version_req) in &pending_packument {
                if let Some(meta) = registry::lookup_zero_copy_packument(base) {
                    if let Some(version) =
                        crate::soul_path::resolve_version_from_meta(version_req, &meta)
                    {
                        let version_str = version.to_string();
                        let url = if meta.dist.tarball_url.is_empty() {
                            lockfile::tarball_url_from_registry(base, &version_str)
                        } else {
                            meta.dist.tarball_url.clone()
                        };
                        registry::remember_lattice_for_current_identity(
                            base,
                            version_req,
                            &version_str,
                        );
                        zero_copy_hits.push((
                            base.clone(),
                            ResolvedFetch {
                                pkg: pkg.clone(),
                                url,
                                integrity: meta.dist.integrity.clone(),
                                version: version_str,
                            },
                        ));
                        continue;
                    }
                }
                still_needed.push((pkg.clone(), base.clone(), version_req.clone()));
            }

            if !zero_copy_hits.is_empty() && still_needed.is_empty() {
                // All resolved from zero-copy — skip network entirely
                resolved_work.extend(zero_copy_hits.into_iter().map(|(_, rf)| rf));
                return resolved_work;
            }

            let cache_arc = Arc::new(Mutex::new(std::collections::HashMap::<
                String,
                serde_json::Value,
            >::new()));

            let mut unique_names: Vec<String> = still_needed
                .iter()
                .map(|(_, base, _)| base.clone())
                .collect();
            unique_names.sort();
            unique_names.dedup();

            let mut meta_results = registry::parallel_fetch_metadata(&unique_names, &cache_arc);
            meta_results.sort_by(|a, b| a.0.cmp(&b.0));

            let mut meta_map: std::collections::HashMap<String, serde_json::Value> =
                std::collections::HashMap::new();
            for (name, res) in meta_results {
                if let Ok(meta) = res {
                    meta_map.insert(name, meta);
                }
            }

            for (pkg, base, version_req) in still_needed {
                let Some(meta) = meta_map.get(&base) else {
                    // Try direct version manifest fetch before giving up
                    if let Ok(manifest) = registry::fetch_manifest(&base, &version_req) {
                        if let Some(ver) = manifest.get("version").and_then(|v| v.as_str()) {
                            let url = manifest
                                .get("dist")
                                .and_then(|d| d.get("tarball"))
                                .and_then(|t| t.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| lockfile::tarball_url_from_registry(&base, ver));
                            let integrity = manifest
                                .get("dist")
                                .and_then(|d| d.get("integrity"))
                                .and_then(|i| i.as_str())
                                .map(|s| s.to_string());
                            registry::remember_lattice_for_current_identity(
                                &base,
                                &version_req,
                                ver,
                            );
                            resolved_work.push(ResolvedFetch {
                                pkg,
                                url,
                                integrity,
                                version: ver.to_string(),
                            });
                            continue;
                        }
                    }
                    crate::utils::log(&format!("Metadata not found for {}", base));
                    crate::utils::record_fallback_reason(
                        "metadata_not_found",
                        std::slice::from_ref(&pkg),
                    );
                    npm_fallback.push(pkg);
                    continue;
                };

                let resolved = if let Some(version) = registry::resolve_version(meta, &version_req)
                {
                    if let Some(url) = registry::get_tarball_url(meta, &version) {
                        let integrity = registry::get_integrity_for_version(meta, &version);
                        Some((version, url, integrity))
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some((version, url, integrity)) = resolved {
                    registry::remember_lattice_for_current_identity(&base, &version_req, &version);
                    resolved_work.push(ResolvedFetch {
                        pkg,
                        url,
                        integrity,
                        version,
                    });
                    continue;
                }

                // If resolve against (potentially stale) cached packument failed, try direct version manifest fetch!
                let mut recovered = false;
                if let Ok(manifest) = registry::fetch_manifest(&base, &version_req) {
                    if let Some(ver) = manifest.get("version").and_then(|v| v.as_str()) {
                        let url = manifest
                            .get("dist")
                            .and_then(|d| d.get("tarball"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| lockfile::tarball_url_from_registry(&base, ver));
                        let integrity = manifest
                            .get("dist")
                            .and_then(|d| d.get("integrity"))
                            .and_then(|i| i.as_str())
                            .map(|s| s.to_string());
                        registry::remember_lattice_for_current_identity(&base, &version_req, ver);
                        resolved_work.push(ResolvedFetch {
                            pkg: pkg.clone(),
                            url,
                            integrity,
                            version: ver.to_string(),
                        });
                        recovered = true;
                    }
                }
                if !recovered {
                    if let Ok(fresh_meta) = registry::fetch_metadata_fresh(&base) {
                        if let Some(version) = registry::resolve_version(&fresh_meta, &version_req)
                        {
                            if let Some(url) = registry::get_tarball_url(&fresh_meta, &version) {
                                let integrity =
                                    registry::get_integrity_for_version(&fresh_meta, &version);
                                registry::remember_lattice_for_current_identity(
                                    &base,
                                    &version_req,
                                    &version,
                                );
                                resolved_work.push(ResolvedFetch {
                                    pkg: pkg.clone(),
                                    url,
                                    integrity,
                                    version,
                                });
                                recovered = true;
                            }
                        }
                    }
                }

                if !recovered {
                    crate::utils::log(&format!(
                        "Could not resolve version {} for {}",
                        version_req, base
                    ));
                    crate::utils::record_fallback_reason(
                        "version_resolve_failed",
                        std::slice::from_ref(&pkg),
                    );
                    npm_fallback.push(pkg);
                }
            }
            // Add zero-copy hits
            resolved_work.extend(zero_copy_hits.into_iter().map(|(_, rf)| rf));
        }

        resolved_work
    }
}

/// Package name without its spec: `lodash@4` -> `lodash`,
/// `@scope/pkg@1.0` -> `@scope/pkg`, and **`my-lodash@npm:lodash@4.17.21` ->
/// `my-lodash`**.
///
/// The split is at the *first* `@` after a leading scope, not the last. Splitting
/// at the last one is only correct when the spec has no `@` of its own; an
/// `npm:` alias does (`npm:real@range`), and the naive `rfind('@')` produced the
/// bogus name `my-lodash@npm:lodash`, which then leaked into resolution as a
/// package to fetch (HTTP 404) and broke the alias install.
/// The CLI-requested package list for the install currently running.
///
/// A JGrid bundle answers *one request*. The manifest-only replay key
/// (`BundleCache::deps_manifest_key`) hashes just `package.json` + platform,
/// so two projects with a byte-identical manifest but different
/// `jhol install <pkg>` arguments collided: `install zod` in a fresh
/// directory whose manifest read `{"name":"t","version":"1.0.0"}` replayed
/// the bundle published by an earlier `install express` in another directory
/// and produced express's tree. Publishing a manifest-keyed alias is
/// therefore only legal when the install had no CLI packages at all — a
/// plain `jhol install`, where the manifest *is* the request.
fn request_slot() -> &'static std::sync::Mutex<Vec<String>> {
    static REQ: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    REQ.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn set_request_key(packages: &[&str]) {
    if let Ok(mut slot) = request_slot().lock() {
        *slot = packages.iter().map(|s| s.to_string()).collect();
        slot.sort();
    }
}

/// True when this install was asked for by name (not by manifest alone).
pub fn request_has_packages() -> bool {
    request_slot().lock().map(|s| !s.is_empty()).unwrap_or(false)
}

/// Transitive `npm:` aliases discovered while expanding the graph:
/// `alias_name -> real_name`. Filled by the expansion, consumed after the
/// install to materialise the alias path.
///
/// `pretty-format` (and therefore jest, expect, vite, next, prisma …)
/// declares `"@jest/react-is-19": "npm:react-is@^19.2.5"`. Those names do not
/// exist on the registry — fetching the alias 404s — so the expansion must
/// resolve the *target* and remember the alias to link afterwards.
fn alias_registry() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static ALIASES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    ALIASES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Public hook for the nested resolver in `jagr_infinite` to record an alias
/// discovered while building the layout.
pub fn record_alias_public(alias: &str, real: &str) {
    record_alias(alias, real)
}

fn record_alias(alias: &str, real: &str) {
    if alias == real {
        return;
    }
    if let Ok(mut m) = alias_registry().lock() {
        m.insert(alias.to_string(), real.to_string());
    }
}

/// The aliases recorded by the most recent expansion.
pub fn recorded_aliases() -> std::collections::HashMap<String, String> {
    alias_registry().lock().map(|m| m.clone()).unwrap_or_default()
}

/// Create `node_modules/<alias>` as a link to `node_modules/<real>` for every
/// recorded alias. Idempotent; missing targets are skipped.
pub fn materialize_aliases(project: &Path) -> usize {
    let aliases = recorded_aliases();
    let nm = project.join("node_modules");
    let mut made = 0usize;
    for (alias, real) in aliases {
        let target = nm.join(&real);
        let link = nm.join(&alias);
        if !target.exists() || link.exists() {
            continue;
        }
        if let Some(parent) = link.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(&target, &link).is_ok() {
                made += 1;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (&target, &link);
        }
    }
    made
}

fn base_name(package: &str) -> &str {
    let start = if package.starts_with('@') {
        // `@scope/...` — the scope's own `@` is not a separator.
        match package.find('/') {
            Some(i) => i + 1,
            None => return package,
        }
    } else {
        0
    };
    match package[start..].find('@') {
        Some(i) => &package[..start + i],
        None => package,
    }
}

/// Names of every dependency the spec-grammar pre-pass materialized itself.
fn special_name_set(specials: &[crate::spec_grammar::SpecialDep]) -> HashSet<String> {
    specials.iter().map(|s| s.name.clone()).collect()
}

/// Registry root set: the requested specs plus the manifest's own dependencies,
/// minus anything the pre-pass already materialized.
///
/// The union exists because a request *adds* to a project rather than replacing
/// its dependencies. Specials must be excluded, because their **name alone is not
/// a registry range**: adding it resolved the registry package of that name, so
/// `{"rimraf":"github:isaacs/rimraf#v3.0.2"}` installed `rimraf@6.1.3` (whose
/// declared `glob ^13` is not even published) and overwrote the git checkout the
/// pre-pass had linked.
fn union_registry_roots<I: Iterator<Item = String>>(
    registry_packages: &[String],
    manifest_dep_names: I,
    special_names: &HashSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = registry_packages.to_vec();
    for name in manifest_dep_names {
        // Compare the dependency *name*, not the raw spec. A manifest entry
        // carries its spec (`my-lodash@npm:lodash@4.17.21`) while `special_names`
        // holds bare names, so a raw comparison never matched an `npm:` alias and
        // the alias leaked into resolution as a package to fetch (HTTP 404).
        if special_names.contains(base_name(&name)) {
            continue;
        }
        if !out.iter().any(|r| base_name(r) == base_name(&name)) {
            out.push(name);
        }
    }
    out
}

/// Exact version from spec, if available (e.g. lodash@4.17.23 -> Some("4.17.23")).
fn exact_version_from_spec(package: &str) -> Option<&str> {
    let idx = package.rfind('@')?;
    if idx == 0 || package[idx + 1..].contains('/') {
        return None;
    }
    let version = package[idx + 1..].trim();
    if version.is_empty()
        || version == "latest"
        || version == "*"
        || version.starts_with('^')
        || version.starts_with('~')
        || version.starts_with('>')
        || version.starts_with('<')
        || version.starts_with('=')
    {
        return None;
    }
    Some(version)
}

/// Read version from node_modules/<base>/package.json (base may be @scope/pkg)
fn read_installed_version(base: &str) -> Option<String> {
    let base = utils::sanitize_package_name(base)?;
    let path = Path::new("node_modules").join(base).join("package.json");
    let s = fs::read_to_string(path).ok()?;
    #[derive(serde::Deserialize)]
    struct FastVersionOnly<'a> {
        #[serde(borrow)]
        version: Option<&'a str>,
    }
    serde_json::from_str::<FastVersionOnly>(&s)
        .ok()?
        .version
        .map(String::from)
}

fn requested_version_spec(package: &str) -> String {
    if let Some(v) = exact_version_from_spec(package) {
        return v.to_string();
    }
    if package.starts_with('@') {
        if let Some(idx) = package.rfind('@') {
            if idx > 0 && !package[idx + 1..].contains('/') {
                let spec = package[idx + 1..].trim();
                if !spec.is_empty() {
                    return spec.to_string();
                }
            }
        }
    }
    if package.contains('@') && !package.starts_with('@') {
        return package
            .split_once('@')
            .map(|x| x.1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "latest".to_string());
    }
    "latest".to_string()
}

fn constraints_satisfied(version: &semver::Version, constraints: &[String]) -> bool {
    let vs = version.to_string();
    constraints.iter().all(|spec| {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed == "latest" || trimmed == "*" {
            return true;
        }
        registry::version_satisfies(trimmed, &vs)
    })
}

fn resolve_version_for_constraints(
    metadata: &serde_json::Value,
    constraints: &[String],
) -> Option<String> {
    let versions = metadata.get("versions")?.as_object()?;
    let mut parsed: Vec<semver::Version> = versions
        .keys()
        .filter_map(|k| semver::Version::parse(k).ok())
        .collect();
    parsed.sort();
    parsed.reverse();

    for version in parsed {
        if constraints_satisfied(&version, constraints) {
            return Some(version.to_string());
        }
    }

    None
}

/// Resolve a version satisfying all constraints, preferring one that also
/// satisfies the peer ranges. If no version satisfies the combined set (a peer
/// conflict), retry with the hard constraints only so the package is still
/// installed at a dependency-valid version (npm's `--legacy-peer-deps`
/// behaviour) rather than being dropped from the tree.
fn resolve_preferring_peers(
    metadata: &serde_json::Value,
    all_constraints: &[String],
    peer_specs: &[String],
) -> Option<String> {
    if let Some(v) = resolve_version_for_constraints(metadata, all_constraints) {
        return Some(v);
    }
    if peer_specs.is_empty() {
        return None;
    }
    let hard: Vec<String> = all_constraints
        .iter()
        .filter(|s| !peer_specs.contains(s))
        .cloned()
        .collect();
    resolve_version_for_constraints(metadata, &hard)
}

fn current_npm_os() -> String {
    crate::utils::target_os()
}

fn current_npm_cpu() -> String {
    crate::utils::target_cpu()
}

fn field_allows_current(value: Option<&serde_json::Value>, current: &str) -> bool {
    let Some(value) = value else {
        return true;
    };
    let Some(list) = value.as_array() else {
        return value.as_str().map(|s| s == current).unwrap_or(true);
    };

    let mut positives = Vec::new();
    for entry in list {
        let Some(raw) = entry.as_str() else {
            continue;
        };
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(neg) = token.strip_prefix('!') {
            if neg == current {
                return false;
            }
        } else {
            positives.push(token);
        }
    }

    positives.is_empty() || positives.contains(&current)
}

pub(crate) fn version_supported_on_current_platform(
    metadata: &serde_json::Value,
    version: &str,
) -> bool {
    let Some(versions) = metadata.get("versions").and_then(|v| v.as_object()) else {
        return true;
    };
    let Some(ver_obj) = versions.get(version) else {
        return true;
    };
    field_allows_current(ver_obj.get("os"), &current_npm_os())
        && field_allows_current(ver_obj.get("cpu"), &current_npm_cpu())
        && match crate::utils::target_libc() {
            Some(libc) => field_allows_current(ver_obj.get("libc"), &libc),
            None => true,
        }
}

/// Drop specs that cannot run on this machine, using the packument the expansion already cached.
///
/// The resolver applies this rule while selecting versions, but the flat *fallback* list can
/// re-introduce one: a **required** `@esbuild/linux-x64` was skipped with a warning and then
/// materialized on macOS/arm64 anyway. The install list is filtered once, here, so no route into
/// it (expansion, fallback, or lockfile reuse) can put a foreign-platform package on disk.
/// Remove leftovers an older jhol (or another package manager) may have written:
/// platform-incompatible packages and dangling symlinks. Without this a Mac can
/// keep `@img/sharp-linuxmusl-arm64` and `@next/swc-win32-x64-msvc` forever - they
/// are never purged because they are no longer part of any resolved tree - and the
/// tree then looks corrupt (symlinks into a pruned store resolve to nothing).
/// Only foreign-platform names and broken links are touched; everything else, the
/// installer owns.
fn prune_platform_incompatible_node_modules(node_modules: &Path) -> usize {
    let mut removed = 0usize;
    let mut visit = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for ent in entries.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if name == ".bin" || name == ".package-lock.json" || name.starts_with('.') {
                continue;
            }
            let path = ent.path();
            // Scoped packages hold the real names one level down.
            if name.starts_with('@') && path.is_dir() {
                let Ok(inner) = std::fs::read_dir(&path) else {
                    continue;
                };
                for sub in inner.flatten() {
                    let scoped = format!("{}/{}", name, sub.file_name().to_string_lossy());
                    let sub_path = sub.path();
                    let foreign = !crate::lockfile::package_name_matches_current_platform(&scoped);
                    let dangling = sub_path
                        .symlink_metadata()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false)
                        && !sub_path.exists();
                    if foreign || dangling {
                        let _ = crate::spec_grammar::remove_path(&sub_path);
                        removed += 1;
                    }
                }
                if std::fs::read_dir(&path)
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_dir(&path);
                }
                continue;
            }
            let foreign = !crate::lockfile::package_name_matches_current_platform(&name);
            let dangling = path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
                && !path.exists();
            if foreign || dangling {
                let _ = crate::spec_grammar::remove_path(&path);
                removed += 1;
            }
        }
    };
    visit(node_modules);
    removed
}

fn drop_platform_incompatible(specs: Vec<String>) -> Vec<String> {
    // Dropping one package is normal (a lockfile records every platform's optional
    // variant) but a 400-package project has ~70 of them, and one warning per drop
    // buried the real output. Collect and report once; list them under verbose.
    let skipped = std::cell::RefCell::new(Vec::<String>::new());
    let kept = specs
        .into_iter()
        .filter(|spec| {
            let name = base_name(spec);
            let version = spec.rsplit_once('@').map(|(_, v)| v).unwrap_or("");
            // Names frequently encode their target (`@esbuild/linux-x64`, `@next/swc-darwin-arm64`),
            // and that signal is available without any metadata. The rule lives in `lockfile`
            // (`package_name_matches_current_platform`) and is used by the lockfile path already —
            // sharing it here is the point: one platform policy, both paths.
            if !crate::lockfile::package_name_matches_current_platform(name) {
                skipped.borrow_mut().push(format!("{}@{}", name, version));
                return false;
            }
            // The in-memory metadata snapshot, locked ONCE for this spec.
            let guard = transitive_metadata_snapshot()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // Name check only - deliberately no network here.
            //
            // This guard used to fall back to `fetch_metadata(name)` for any spec missing from the
            // in-memory snapshot, which meant one *serial* registry round-trip per package: on a
            // 421-dependency project it dominated the run (stack-sampled, ~2600 samples in
            // `drop_platform_incompatible -> fetch_metadata -> get_bytes`) and looked like a hang.
            // The information that matters here is in the name (`@esbuild/linux-x64`,
            // `@next/swc-win32-x64-msvc`), and the resolver already applies the full `os`/`cpu`
            // filtering at selection time (it logs the skip and records the variant). If the
            // expansion happened to cache this package's metadata we use it; otherwise the name wins.
            //
            // NOTE: this locked `transitive_metadata_snapshot()` a SECOND time while `guard`
            // (above) was still alive. `std::sync::Mutex` is not reentrant, so that was a
            // permanent self-deadlock on the one thread doing the work: every cold
            // `jhol install` hung at 0% CPU with the main thread parked in `direct_install`'s
            // `recv_timeout` (main.rs:777) waiting for a result that could never arrive. The
            // already-held `guard` is the same map, so one lock is enough.
            let owned = guard.get(name).cloned();
            let meta = owned.as_ref();
            match meta {
                Some(meta) if !version_supported_on_current_platform(meta, version) => {
                    skipped
                        .borrow_mut()
                        .push(format!("{}@{} (os/cpu)", name, version));
                    false
                }
                _ => true,
            }
        })
        .collect();
    let skipped = skipped.into_inner();
    if !skipped.is_empty() && !crate::utils::is_quiet() {
        let detail = if crate::utils::is_verbose() {
            skipped
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            "run with JHOL_VERBOSE=1 to list them".to_string()
        };
        crate::utils::log(&format!(
            "skipped {} package{} for other platforms ({detail}) · this host is {}-{}",
            skipped.len(),
            if skipped.len() == 1 { "" } else { "s" },
            crate::utils::target_os(),
            crate::utils::target_cpu()
        ));
    }
    kept
}

/// Build the node-keyed nested layout from the metadata this install already
/// fetched, instead of re-resolving the manifest from scratch.
///
/// A version conflict the *flat* model cannot express made `install_package` call
/// `resolve_full_tree(manifest)`: a second full resolution over a graph this
/// install had already walked, with its own metadata pass and its own atlas
/// build. On a real Next.js app that second pass dominated the run — every
/// tarball was already downloaded and unpacked and `node_modules` still stayed
/// empty. The flat pass's packuments are all in the transitive metadata
/// snapshot, and `layout::build_node_layout_with` turns the flat (name-keyed)
/// selection plus those packuments into the node-keyed tree, so the second pass
/// is unnecessary for this case.
///
/// Returns `None` when the snapshot cannot cover the resulting tree (a node's
/// packument, or an edge target, is missing) so the caller can fall back to the
/// full resolver rather than emit an incomplete tree.
fn nested_layout_from_flat_state(
    direct: &HashMap<String, String>,
    selected: &HashMap<String, String>,
) -> Option<std::collections::BTreeMap<String, crate::lockfile_write::ResolvedPackage>> {
    // Take the metadata this install already holds under ONE short lock, and
    // never hold that lock across the fetch below — re-locking the same
    // non-reentrant mutex is exactly the self-deadlock that made every cold
    // install hang (see `drop_platform_incompatible`).
    let mut metas: HashMap<String, std::sync::Arc<serde_json::Value>> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let snapshot = transitive_metadata_snapshot().lock().ok()?;
        for name in selected.keys() {
            match snapshot.get(name) {
                Some(m) => {
                    metas.insert(name.clone(), m.clone());
                }
                None => missing.push(name.clone()),
            }
        }
    }

    // Fill gaps from the install's own packument cache. A package can be in the
    // flat selection yet absent from the snapshot because the flat expansion's
    // binary-index and lattice fast paths select a *version* without recording
    // metadata. `registry::fetch_metadata` is the same abbreviated packument the
    // flat pass already put on disk, so these are cache hits rather than new
    // network round-trips; if one genuinely cannot be obtained the caller falls
    // back to the full resolver.
    for name in missing {
        let Ok(meta) = crate::registry::fetch_metadata(&name) else {
            return None;
        };
        let arc = std::sync::Arc::new(meta);
        metas.insert(name.clone(), arc.clone());
        if let Ok(mut g) = transitive_metadata_snapshot().lock() {
            g.insert(name, arc);
        }
    }

    // The flat selection as ResolvedPackages (no lock held here).
    let mut resolved: HashMap<String, crate::lockfile_write::ResolvedPackage> = HashMap::new();
    for (name, version) in selected {
        let mut meta = metas.get(name)?.clone();
        // `dist` (tarball URL + SRI integrity) only exists in a real packument. A
        // manifest synth carries the package manifest instead, so a package
        // resolved through a synth had `dist` absent and shipped into the
        // lockfile with a URL but **no integrity** — and strict `ci` then refuses
        // the tree ("Strict lockfile: integrity entries missing"). Escalate to the
        // real packument for the package when the version we selected has no
        // `dist`, and use that from then on (also recorded for later readers).
        let has_dist = meta
            .get("versions")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(version))
            .and_then(|vi| vi.get("dist"))
            .is_some();
        if !has_dist {
            if let Ok(full) = crate::registry::fetch_metadata_full_packument(name) {
                let arc = std::sync::Arc::new(full);
                if let Ok(mut g) = transitive_metadata_snapshot().lock() {
                    g.insert(name.clone(), arc.clone());
                }
                metas.insert(name.clone(), arc.clone());
                meta = arc;
            }
        }
        let meta = meta.as_ref();
        let deps: HashMap<String, String> =
            crate::registry::get_version_required_dependencies(meta, version)
                .into_iter()
                .collect();
        let peers = crate::registry::get_version_peer_dependencies(meta, version);
        let peer_meta = crate::registry::get_version_peer_dependencies_meta(meta, version);
        let dist = meta
            .get("versions")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(version))
            .and_then(|vi| vi.get("dist"));
        let url = dist
            .and_then(|d| d.get("tarball"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| crate::lockfile::tarball_url_from_registry(name, version));
        let integrity = dist
            .and_then(|d| d.get("integrity"))
            .and_then(|i| i.as_str())
            .map(|s| s.to_string());
        resolved.insert(
            name.clone(),
            crate::lockfile_write::ResolvedPackage {
                version: version.clone(),
                resolved: url,
                integrity,
                dependencies: deps,
                peer_dependencies: peers,
                peer_dependencies_meta: peer_meta,
            },
        );
    }

    // Manifest-first metadata may cover only the hoisted version. Fetch a full
    // domain only for children whose declared range has no available candidate.
    let missing_domains: HashSet<String> = resolved
        .values()
        .flat_map(|pkg| {
            pkg.dependencies.iter().filter_map(|(child, range)| {
                let covered = metas
                    .get(child)
                    .and_then(|m| m.get("versions"))
                    .and_then(|v| v.as_object())
                    .is_some_and(|versions| {
                        versions
                            .keys()
                            .any(|v| crate::registry::version_satisfies(range, v))
                    });
                (!covered).then(|| child.clone())
            })
        })
        .collect();
    for (name, result) in crate::pool::parallel_map(
        missing_domains.into_iter().collect(),
        crate::registry::packument_concurrency(),
        1,
        |name| {
            let result = crate::registry::fetch_metadata_full_packument(&name);
            (name, result)
        },
    ) {
        let meta = std::sync::Arc::new(result.ok()?);
        metas.insert(name.clone(), meta.clone());
        if let Ok(mut snapshot) = transitive_metadata_snapshot().lock() {
            snapshot.insert(name, meta);
        }
    }

    let layout = crate::layout::build_node_layout_with(direct, &resolved, |n| {
        metas.get(n).map(|m| m.as_ref())
    });

    // Validate declarations, not only emitted edges: the builder cannot emit
    // a dependency when a single-version manifest lacks its required candidate.
    // Peers retain the existing lenient policy; required edges must be exact.
    if !crate::layout::required_edges_satisfied(&layout, |n| metas.get(n).map(|m| m.as_ref())) {
        return None;
    }
    Some(layout)
}

/// An optional dependency whose version was resolved but which does not match the
/// current platform. npm records these in the lockfile (marked `optional` with
/// `os`/`cpu`/`libc`) so the *same* lockfile installs correctly on other
/// platforms; we resolve them but never materialize them. The lockfile reader
/// already skips platform-incompatible optional entries, so adding them is safe.

/// npm `overrides` / yarn `resolutions`, merged with any caller-supplied ones.
fn load_overrides() -> HashMap<String, String> {
    lockfile::read_overrides(Path::new("package.json"))
}

/// Names listed under `optionalDependencies` in `package.json`.
fn optional_dependency_names() -> HashSet<String> {
    lockfile::read_optional_dependency_names(Path::new("package.json"))
}


/// Quiet unless JHOL_VERBOSE is set — this note is diagnostic, not noise.
fn quiet_hint() -> bool {
    std::env::var_os("JHOL_VERBOSE").is_none()
}
fn expand_with_transitive_dependencies(
    packages: &[&str],
    offline: bool,
    option_overrides: Option<&HashMap<String, String>>,
) -> Result<
    (
        Vec<String>,
        Vec<OptionalVariant>,
        Option<(String, String, String, String)>,
    ),
    String,
> {
    // Compatibility-first default: include at least one transitive layer for direct installs.
    // Keeps `axios -> follow-redirects` and similar chains correct without requiring env tuning.
    // Soul Path: default full-depth when streaming; else depth 32 (was 2 — incomplete trees)
    let max_depth = std::env::var("JHOL_TRANSITIVE_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(if crate::soul_path::stream_install_enabled() {
            64
        } else {
            32
        });
    let mut overrides = load_overrides();
    if let Some(extra) = option_overrides {
        for (k, v) in extra {
            overrides.insert(k.clone(), v.clone());
        }
    }
    let optional_direct = optional_dependency_names();
    let registry_identity = registry::registry_configuration_fingerprint(Path::new("."));
    let cache_key = trans_expansion_cache_key(
        &registry_identity,
        max_depth,
        packages,
        offline,
        registry::offline_mode(),
        &overrides,
        &optional_direct,
    );

    // Persistent expansion cache: avoid re-reading every tarball's package.json
    // across installs. Survives process restarts.
    if let Some(hit) = load_trans_expansion_cache(&cache_key) {
        return Ok((hit.packages, hit.optional_variants, None));
    }

    if transitive_cache_enabled() {
        if let Ok(cache) = transitive_memo_cache().lock() {
            if let Some(hit) = cache.get(&cache_key) {
                return Ok((hit.packages.clone(), hit.optional_variants.clone(), None));
            }
        }
    }

    // npm `overrides` / yarn `resolutions`: these win over any range seen for
    // the package, so every read of `constraints` is filtered through here.
    let effective = |name: &str, constraints: &HashMap<String, Vec<String>>| -> Vec<String> {
        match overrides.get(name) {
            Some(spec) => vec![spec.clone()],
            None => constraints.get(name).cloned().unwrap_or_default(),
        }
    };

    // N3: Use Arc<str> in queue to avoid String clones on BFS dequeues.
    // Maps stay HashMap<String, ...> since callers and transitive caches expect String keys.
    let mut constraints: HashMap<String, Vec<String>> = HashMap::new();
    // Ranges that came from `peerDependencies`. Kept separately so a peer
    // conflict can fall back to the hard (dependency) constraints instead of
    // emptying a package's domain and dropping it from the tree.
    let mut peer_specs: HashMap<String, Vec<String>> = HashMap::new();
    let mut required: HashMap<String, bool> = HashMap::new();
    let mut selected: HashMap<String, String> = HashMap::new();
    // Optional deps resolved for OTHER platforms (not installed here, but
    // recorded in the lockfile so one lockfile works on every OS).
    let mut optional_variants: Vec<OptionalVariant> = Vec::new();
    let mut expanded_deps: HashSet<(String, String)> = HashSet::new();
    let mut metadata_cache: HashMap<String, std::sync::Arc<serde_json::Value>> = HashMap::new();
    let mut queue: VecDeque<(Arc<str>, usize)> = VecDeque::new();

    // Optional direct dependencies are skippable: mark them not-required so an
    // unresolvable one is dropped instead of failing the install.
    for package in packages {
        let base: Arc<str> = base_name(package).into();
        let spec = requested_version_spec(package);
        constraints.entry(base.to_string()).or_default().push(spec);
        let is_optional = optional_direct.contains(base.as_ref());
        required.insert(base.to_string(), !is_optional);
        queue.push_back((base, 0));
    }

    // ★ Markov Prediction Pre-fetch: use the installed chain to predict what comes next.
    let markov_enabled = std::env::var("JHOL_MARKOV_PREFETCH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if markov_enabled && !offline && !registry::offline_mode() && !packages.is_empty() {
        let base_names: Vec<String> = packages.iter().map(|p| base_name(p).to_string()).collect();
        let mut predicted_pkgs = Vec::new();
        // Pairwise predictions (second-order)
        if base_names.len() >= 2 {
            let max_pairs = base_names.len().saturating_sub(1).min(4);
            for i in 0..max_pairs {
                let (prev, curr) = (&base_names[i], &base_names[i + 1]);
                let preds = crate::second_order_markov::predict_with_context(prev, curr, 8);
                for p in preds {
                    if !base_names.contains(&p) && !predicted_pkgs.contains(&p) {
                        predicted_pkgs.push(p);
                    }
                }
            }
        }
        // First-order predictions for individual root packages (e.g. express alone, or single direct roots)
        for base in &base_names {
            let preds = crate::second_order_markov::predict_with_context("", base, 12);
            for p in preds {
                if !base_names.contains(&p) && !predicted_pkgs.contains(&p) {
                    predicted_pkgs.push(p);
                }
            }
        }
        // Dispatch non-blocking prefetch on the global thread pool concurrently at t = 0
        for pkg in predicted_pkgs {
            rayon::spawn(move || {
                let _ = registry::fetch_metadata(&pkg);
            });
        }
    }

    // ★ Floodgate Pre-fetch on default path:
    // Pre-populate `metadata_cache` with the floodgate's full transitive packument
    // closure before the BFS runs. The work-stealing pipelined frontier in
    // `fetch_dep_tree` fetches all discovered packages concurrently without level
    // barriers. The BFS then finds everything cached, so its wave fetches never fire.
    // Version selection still happens in the BFS — only the network fetch is swapped,
    // so resolution quality is identical by construction. Any packument the floodgate
    // misses falls through to the existing deferred waves (correctness preserved).
    if !offline
        && !registry::offline_mode()
        && !packages.is_empty()
        && std::env::var("JHOL_NO_FLOODGATE").is_err()
    {
        let mut direct_map: HashMap<String, String> = HashMap::new();
        for package in packages {
            let base = base_name(package);
            let spec = requested_version_spec(package);
            if !spec.starts_with("file:")
                && !spec.starts_with("link:")
                && !spec.starts_with("git")
                && !spec.starts_with("workspace:")
                && !package.starts_with('.')
                && !package.starts_with('/')
            {
                direct_map.insert(base.to_string(), spec);
            }
        }
        if !direct_map.is_empty() {
            if let Ok(tree) = crate::jagr_infinite::fetch_dep_tree(&direct_map) {
                for (name, packument) in tree {
                    let arc_meta = std::sync::Arc::new(packument);
                    metadata_cache.insert(name.clone(), std::sync::Arc::clone(&arc_meta));
                    if transitive_cache_enabled() {
                        if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                            cache.insert(name.clone(), arc_meta);
                        }
                    }
                }
            }
        }
    }

    // First pass: try binary index and cache, collect deferred packages
    // N3: queue uses Arc<str> to avoid String clones on pop/requeue.
    // Deferred packages are re-enqueued after the parallel metadata fetch so
    // their dependencies are expanded too (cold installs must produce the full
    // transitive tree, not just the direct set).
    // Pipelined dependency discovery.
    //
    // The old loop fetched packuments in level-synchronous rounds: drain the
    // queue, fetch every deferred package as a batch, re-enqueue, repeat — one
    // network RTT barrier per dependency level. Here each packument is fetched
    // the moment it is discovered and its dependencies are expanded as soon as
    // the result arrives, so deeper levels overlap with still-in-flight siblings
    // instead of waiting for the whole level to finish.
    // Tarball prefetch: while deeper metadata is still being fetched, start
    // downloading the tarballs of packages that are already resolved, and record
    // them in the store index. The install phase then finds them as cache hits
    // instead of doing that download work serially after resolution. Skipped for
    // packages already in the store (so warm installs stay network-free).
    // Disable with JHOL_NO_PREFETCH=1.
    let prefetch_enabled =
        !offline && !registry::offline_mode() && std::env::var("JHOL_NO_PREFETCH").is_err();
    let prefetch_index: std::sync::Arc<std::sync::Mutex<HashMap<String, String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
    let prefetch_store: std::sync::Arc<HashMap<String, String>> =
        std::sync::Arc::new(crate::utils::read_store_index());
    let prefetch_cache_dir = std::path::PathBuf::from(crate::utils::get_cache_dir());
    let mut prefetch_started: HashSet<String> = HashSet::new();
    let pipeline_result: Result<(), String> = rayon::scope(|s| -> Result<(), String> {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<(String, usize, Result<serde_json::Value, String>)>();
        let mut pending: HashSet<String> = HashSet::new();
        let mut in_flight: usize = 0usize;

        // Runaway guards. On some real projects (e.g. a Next.js app: 12 direct
        // dependencies) this pipelined discovery grew past 4 GB and never
        // finished, which OOM-killed the process and thrashed the machine. These
        // bounds turn that into a fast, explicit error instead of an OOM, and
        // they are far above any genuine graph (no real project has thousands of
        // *distinct* package names, and work items only exceed the bound by a
        // large multiple of the graph when discovery is looping).
        const MAX_GRAPH_PACKAGES: usize = 20_000;
        const MAX_DISCOVERY_WORK: usize = 500_000;
        // Overridable for debugging: JHOL_MAX_GRAPH_PACKAGES / JHOL_MAX_DISCOVERY_WORK.
        let max_graph_packages = std::env::var("JHOL_MAX_GRAPH_PACKAGES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(MAX_GRAPH_PACKAGES);
        let max_discovery_work = std::env::var("JHOL_MAX_DISCOVERY_WORK")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(MAX_DISCOVERY_WORK);
        let mut work_items: usize = 0usize;

        loop {
            while let Some((name_arc, depth)) = queue.pop_front() {
                work_items += 1;
                if work_items > max_discovery_work || constraints.len() > max_graph_packages {
                    return Err(format!(
                        "dependency discovery exceeded safety bounds ({} distinct packages, \
                         {} work items); aborting instead of exhausting memory. \
                         Raise JHOL_MAX_GRAPH_PACKAGES / JHOL_MAX_DISCOVERY_WORK to override.",
                        constraints.len(),
                        work_items
                    ));
                }
                // Convert Arc<str> to String once for all map operations in this iteration
                let name: String = name_arc.to_string();
                let Some(_pkg_constraints) = constraints.get(&name).cloned() else {
                    continue;
                };
                let pkg_constraints = effective(&name, &constraints);

                if !metadata_cache.contains_key(&name) {
                    // Snapshot-backed pre-selection (machine-local binary store and
                    // the remembered-resolution lattice) may only commit a version
                    // where the freshness contract allows it: offline, exact pins,
                    // or JHOL_PREFER_BUNDLED=1. Otherwise a range falls through to
                    // live metadata, so the newest satisfying version wins as in
                    // npm/bun.
                    let snapshot_ok = registry::bundled_index_allowed(&pkg_constraints);
                    if let Some(binary_version) = snapshot_ok
                        .then(|| registry::best_binary_version_matching(&name, &pkg_constraints))
                        .flatten()
                    {
                        if depth >= max_depth {
                            selected.insert(name.clone(), binary_version);
                            continue;
                        } else if let Some(required_deps) =
                            registry::binary_package_dependencies(&name, &binary_version)
                        {
                            selected.insert(name.clone(), binary_version);
                            for (dep_name, dep_spec) in required_deps {
                                let dn: Arc<str> = dep_name.into();
                                let dn_str = dn.to_string();
                                let dep_constraints = constraints.entry(dn_str).or_default();
                                if !dep_constraints.iter().any(|s| s == &dep_spec) {
                                    dep_constraints.push(dep_spec);
                                    required.insert(dn.to_string(), true);
                                    queue.push_back((dn, depth + 1));
                                }
                            }
                            continue;
                        }
                    }

                    // Lattice cache: skip metadata fetch ONLY IF a remembered version
                    // satisfies ALL known constraints for this package AND we can satisfy its dependencies.
                    if let Some(c) = pkg_constraints.first().filter(|_| snapshot_ok) {
                        if let Some(lattice_version) =
                            registry::lookup_lattice_for_current_identity(&name, c)
                        {
                            if pkg_constraints
                                .iter()
                                .all(|req| registry::version_satisfies(req, &lattice_version))
                            {
                                if depth >= max_depth {
                                    selected.insert(name.clone(), lattice_version);
                                    continue;
                                } else if let Some(required_deps) =
                                    registry::binary_package_dependencies(&name, &lattice_version)
                                {
                                    selected.insert(name.clone(), lattice_version);
                                    for (dep_name, dep_spec) in required_deps {
                                        let dn: Arc<str> = dep_name.into();
                                        let dn_str = dn.to_string();
                                        let dep_constraints =
                                            constraints.entry(dn_str).or_default();
                                        if !dep_constraints.iter().any(|s| s == &dep_spec) {
                                            dep_constraints.push(dep_spec);
                                            required.insert(dn.to_string(), true);
                                            queue.push_back((dn, depth + 1));
                                        }
                                    }
                                    continue;
                                }
                            }
                        }
                    }

                    // Check transitive cache and packument read cache
                    let snapshot_hit = if transitive_cache_enabled() {
                        transitive_metadata_snapshot()
                            .lock()
                            .ok()
                            .and_then(|cache| cache.get(&name).map(std::sync::Arc::clone))
                    } else {
                        None
                    };

                    let cached_packument = if snapshot_hit.is_none() {
                        crate::registry::get_cached_packument(&name).map(std::sync::Arc::new)
                    } else {
                        None
                    };

                    if let Some(hit) = snapshot_hit.or(cached_packument) {
                        metadata_cache.insert(name.clone(), hit);
                    } else {
                        // Bundled-index synth is local (no network): insert it and fall
                        // through so it is processed in this iteration.
                        let constraint_for_pkg =
                            pkg_constraints.first().map(|s| s.as_str()).unwrap_or("");
                        // Same rule as `registry::bundled_index_allowed`: the bundled
                        // index is a build-time snapshot, so it may answer only offline
                        // or for an exact pin — never for a live range (it would pin an
                        // outdated version).
                        let bundled_ok = registry::bundled_index_allowed(&pkg_constraints);
                        if let Some(synth) = if bundled_ok {
                            registry::synthesize_packument_from_bundled_index(
                                &name,
                                constraint_for_pkg,
                            )
                        } else {
                            None
                        } {
                            let synth = std::sync::Arc::new(synth);
                            if transitive_cache_enabled() {
                                if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                                    cache.insert(name.clone(), std::sync::Arc::clone(&synth));
                                }
                            }
                            metadata_cache.insert(name.clone(), synth);
                        } else if pending.contains(&name) {
                            // Already being fetched; the result will re-enqueue it.
                            continue;
                        } else {
                            if offline || registry::offline_mode() {
                                if required.get(&name) == Some(&false) {
                                    crate::utils::log(&format!(
                                        "warning: skipping optional dependency {} (offline)",
                                        name
                                    ));
                                    continue;
                                }
                                return Err(format!(
                                    "Offline mode: package(s) not in the content store: {}. Fetch them once with:\n  jhol prefetch\n(keep package-lock.json committed for reproducible offline installs)",
                                    name
                                ));
                            }
                            pending.insert(name.clone());
                            in_flight += 1;
                            let n = name.clone();
                            let d = depth;
                            // Constraint-aware fetch: prefer a tiny manifest synth
                            // when the registry's latest satisfies every known
                            // constraint, and only fall back to the packument.
                            let cons = pkg_constraints.clone();
                            let tx = tx.clone();
                            s.spawn(move |_| {
                                // catch_unwind guarantees we always send a result, so
                                // the coordinator can never block forever.
                                let r =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        registry::fetch_metadata_manifest_first(&n, &cons)
                                    }))
                                    .unwrap_or_else(|_| Err("metadata fetch panicked".to_string()));
                                let _ = tx.send((n, d, r));
                            });
                            continue;
                        }
                    }
                }

                // If a synth (manifest or bundled-index) no longer covers the
                // constraints — e.g. a later dependent added a range its single
                // version does not satisfy — escalate to the full packument
                // before resolving, instead of silently dropping the package.
                let synth_needs_escalation = metadata_cache
                    .get(&name)
                    .map(|m| {
                        (m.get("_vortex_manifest_synth").is_some()
                            || m.get("_bundled_index_synth").is_some())
                            && !crate::registry::synth_satisfies_all(m, &pkg_constraints)
                    })
                    .unwrap_or(false);
                if synth_needs_escalation {
                    if offline || registry::offline_mode() {
                        if required.get(&name) == Some(&false) {
                            crate::utils::log(&format!(
                                "warning: skipping optional dependency {} (offline)",
                                name
                            ));
                            continue;
                        }
                        return Err(format!(
                            "Offline mode: package(s) not in the content store: {}. Fetch them once with:\n  jhol prefetch\n(keep package-lock.json committed for reproducible offline installs)",
                            name
                        ));
                    }
                    // Keep consuming independent results while full domains arrive.
                    // The same pending set bounds duplicate initial/escalation jobs.
                    if pending.insert(name.clone()) {
                        in_flight += 1;
                        let tx = tx.clone();
                        s.spawn(move |_| {
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    registry::fetch_metadata_full_packument(&name)
                                }))
                                .unwrap_or_else(|_| Err("metadata fetch panicked".to_string()));
                            let _ = tx.send((name, depth, result));
                        });
                    }
                    continue;
                }

                // Package is in metadata_cache, process it
                let metadata = metadata_cache
                    .get(&name)
                    .ok_or_else(|| format!("Missing metadata for {}", name))?;

                let peer_constraints: &[String] =
                    peer_specs.get(&name).map(|v| v.as_slice()).unwrap_or(&[]);
                let version =
                    match resolve_preferring_peers(metadata, &pkg_constraints, peer_constraints) {
                        Some(v) => v,
                        None => continue,
                    };

                let is_required = *required.get(&name).unwrap_or(&true);
                if !version_supported_on_current_platform(metadata, &version) {
                    if is_required {
                        crate::utils::log(&format!(
                            "warning: {}@{} skipped — incompatible with current platform",
                            name, version
                        ));
                    } else {
                        // Optional dep for another platform: keep it for the
                        // lockfile (npm parity) but do not install it.
                        optional_variants
                            .push(optional_variant_from_meta(&name, &version, metadata));
                    }
                    continue;
                }

                // Optional dep installed on this platform: also record it so the
                // lockfile marks it `optional` (npm parity) and other platforms
                // skip it.
                if !is_required {
                    optional_variants.push(optional_variant_from_meta(&name, &version, metadata));
                }

                let changed = selected
                    .get(&name)
                    .map(|existing| existing != &version)
                    .unwrap_or(true);

                if changed {
                    selected.insert(name.clone(), version.clone());
                }

                if prefetch_enabled && depth < max_depth && !prefetch_started.contains(&name) {
                    prefetch_started.insert(name.clone());
                    let spec = format!("{}@{}", name, version);
                    if !prefetch_store.contains_key(&spec) {
                        if let (Some(url), integrity) = (
                            registry::get_tarball_url(metadata, &version),
                            registry::get_integrity_for_version(metadata, &version),
                        ) {
                            let pf = prefetch_index.clone();
                            let cd = prefetch_cache_dir.clone();
                            let key = spec.clone();
                            let download = move || {
                                if let Ok(hash) = registry::download_tarball_to_store_hash_only(
                                    &url,
                                    &cd,
                                    &key,
                                    integrity.as_deref(),
                                ) {
                                    if let Ok(mut m) = pf.lock() {
                                        m.insert(key, hash);
                                    }
                                }
                            };
                            if std::env::var("JHOL_PREFETCH_IO_POOL").as_deref() == Ok("1") {
                                crate::pool::global_pool().spawn(download);
                            } else {
                                rayon::spawn(download);
                            }
                        }
                    }
                }

                // Expand dependencies even when the version already selected: the
                // lattice/binary fast-path above pre-selects a version WITHOUT
                // traversing its dependencies, so an unchanged version must still have
                // its dep tree expanded once metadata becomes available.
                if depth < max_depth && expanded_deps.insert((name.clone(), version.clone())) {
                    let required_deps =
                        registry::get_version_required_dependencies(metadata, &version);
                    for (dep_name, dep_spec) in required_deps {
                        // `"alias": "npm:real@range"` — resolve the real
                        // package; the alias path is linked after the install.
                        let (dep_name, dep_spec) =
                            match crate::spec_grammar::classify(&dep_spec) {
                                crate::spec_grammar::Spec::Alias { real, range } => {
                                    record_alias(&dep_name, &real);
                                    (real, range)
                                }
                                _ => (dep_name, dep_spec),
                            };
                        let dn: Arc<str> = dep_name.into();
                        let dn_str = dn.to_string();
                        let dep_constraints = constraints.entry(dn_str.clone()).or_default();
                        if !dep_constraints.iter().any(|s| s == &dep_spec) {
                            dep_constraints.push(dep_spec);
                            required.insert(dn_str, true);
                            queue.push_back((dn, depth + 1));
                        }
                    }

                    let optional_deps =
                        registry::get_version_optional_dependencies(metadata, &version);
                    for (dep_name, dep_spec) in optional_deps {
                        if !crate::lockfile::package_name_matches_current_platform(&dep_name) {
                            let clean_version = dep_spec
                                .trim()
                                .trim_start_matches('=')
                                .trim_start_matches('v')
                                .to_string();
                            let (os_tokens, cpu_tokens) =
                                crate::lockfile::extract_platform_tokens_from_name(&dep_name);
                            optional_variants.push(OptionalVariant::new(
                                dep_name.clone(),
                                clean_version.clone(),
                                crate::lockfile::tarball_url_from_registry(
                                    &dep_name,
                                    &clean_version,
                                ),
                                None,
                                os_tokens,
                                cpu_tokens,
                                Vec::new(),
                            ));
                            continue;
                        }
                        let dn: Arc<str> = dep_name.into();
                        let dn_str = dn.to_string();
                        let dep_constraints = constraints.entry(dn_str.clone()).or_default();
                        if !dep_constraints.iter().any(|s| s == &dep_spec) {
                            dep_constraints.push(dep_spec);
                            required.entry(dn_str.clone()).or_insert(false);
                            queue.push_back((dn, depth + 1));
                        }
                    }

                    // Peers: npm v7+ auto-installs non-optional peers, so a project
                    // that declares only `react-dom` still gets `react`. Optional
                    // peers are not installed, but if the peer is already part of
                    // the tree its range must still be honoured.
                    let peer_deps =
                        registry::resolve_peer_dependencies_with_meta(metadata, &version);
                    let mut peer_deps: Vec<(String, (String, bool))> =
                        peer_deps.into_iter().collect();
                    peer_deps.sort();
                    for (dep_name, (dep_spec, optional)) in peer_deps {
                        if optional {
                            if let Some(c) = constraints.get_mut(&dep_name) {
                                if !c.iter().any(|s| s == &dep_spec) {
                                    c.push(dep_spec);
                                }
                            }
                            continue;
                        }
                        let dn: Arc<str> = dep_name.into();
                        let dn_str = dn.to_string();
                        let dep_constraints = constraints.entry(dn_str.clone()).or_default();
                        if !dep_constraints.iter().any(|s| s == &dep_spec) {
                            peer_specs
                                .entry(dn_str.clone())
                                .or_default()
                                .push(dep_spec.clone());
                            dep_constraints.push(dep_spec);
                            // Non-optional peers are required: elevate a previously
                            // optional entry to required.
                            required.insert(dn_str.clone(), true);
                            queue.push_back((dn, depth + 1));
                        }
                    }
                }
            }

            // No more queued work: wait for the next in-flight fetch. Results
            // are processed as they arrive (not as a batch), which is what
            // removes the per-level barrier.
            // `pending` is the authoritative in-flight set; `in_flight` is a counter that can drift
            // if any path returns without decrementing. Trust the set, and bound the wait so a drifted
            // counter costs a timeout instead of an install that never returns (measured: a bare install
            // in a 3-dependency project with a fresh cache hung here indefinitely, `0/N resolving`,
            // 0 cache, 0 network).
            if in_flight == 0 || pending.is_empty() {
                break;
            }
            let (pkg_name, depth, result) =
                match rx.recv_timeout(std::time::Duration::from_secs(120)) {
                    Ok(v) => v,
                    Err(_) => break,
                };
            in_flight -= 1;
            pending.remove(&pkg_name);
            match result {
                Ok(meta) => {
                    let meta = std::sync::Arc::new(meta);
                    metadata_cache.insert(pkg_name.clone(), std::sync::Arc::clone(&meta));
                    if transitive_cache_enabled() {
                        if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                            cache.insert(pkg_name.clone(), meta);
                        }
                    }
                }
                Err(e) => {
                    // Optional dependencies are skippable by definition (npm warns
                    // and carries on); failing the whole install because one is
                    // unavailable is wrong.
                    if required.get(&pkg_name) == Some(&false) {
                        crate::utils::log(&format!(
                            "warning: skipping optional dependency {} ({})",
                            pkg_name, e
                        ));
                        continue;
                    }
                    return Err(format!("Failed to fetch metadata for {}: {}", pkg_name, e));
                }
            }
            queue.push_back((Arc::from(pkg_name.as_str()), depth));
        }
        Ok(())
    });
    // The scope has joined all prefetch downloads. Publish them to the store
    // index so the install phase classifies them as cache hits.
    if let Ok(pf) = prefetch_index.lock() {
        if !pf.is_empty() {
            let mut idx = crate::utils::read_store_index();
            for (k, v) in pf.iter() {
                idx.insert(k.clone(), v.clone());
            }
            let _ = crate::utils::write_store_index(&idx);
        }
    }
    pipeline_result?;

    // Second pass: now process all packages with metadata available
    for name in constraints.keys() {
        if selected.contains_key(name) {
            continue;
        }

        let Some(mut metadata) = metadata_cache.get(name).cloned() else {
            continue;
        };
        let pkg_constraints: Vec<String> = effective(name, &constraints);
        let version = {
            let peer_constraints: &[String] =
                peer_specs.get(name).map(|v| v.as_slice()).unwrap_or(&[]);
            match resolve_preferring_peers(&metadata, &pkg_constraints, peer_constraints) {
                Some(v) => v,
                None => {
                    if !crate::registry::offline_mode() {
                        if let Ok(fresh_meta) = registry::fetch_metadata_fresh(name) {
                            let fresh_arc = std::sync::Arc::new(fresh_meta);
                            metadata = fresh_arc.clone();
                            metadata_cache.insert(name.to_string(), fresh_arc);
                            if let Some(v) = resolve_preferring_peers(
                                &metadata,
                                &pkg_constraints,
                                peer_constraints,
                            ) {
                                v
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
            }
        };

        let is_required = *required.get(name).unwrap_or(&true);
        if !version_supported_on_current_platform(&metadata, &version) {
            if !is_required {
                optional_variants.push(optional_variant_from_meta(name, &version, &metadata));
            }
            continue;
        }

        // Optional dep installed on this platform: also mark it `optional` in
        // the lockfile so other platforms skip it.
        if !is_required {
            optional_variants.push(optional_variant_from_meta(name, &version, &metadata));
        }

        selected.insert(name.clone(), version.clone());
    }

    while let Some((name_arc, depth)) = queue.pop_front() {
        let name: String = name_arc.to_string();
        let Some(_pkg_constraints) = constraints.get(&name).cloned() else {
            continue;
        };
        let pkg_constraints = effective(&name, &constraints);

        if !metadata_cache.contains_key(&name) {
            if let Some(binary_version) =
                registry::best_binary_version_matching(&name, &pkg_constraints)
            {
                let changed = selected
                    .get(&name)
                    .map(|existing| existing != &binary_version)
                    .unwrap_or(true);

                if changed {
                    selected.insert(name.clone(), binary_version.clone());
                    if depth < max_depth {
                        if let Some(required_deps) =
                            registry::binary_package_dependencies(&name, &binary_version)
                        {
                            for (dep_name, dep_spec) in required_deps {
                                let dep_constraints =
                                    constraints.entry(dep_name.clone()).or_default();
                                if !dep_constraints.iter().any(|s| s == &dep_spec) {
                                    dep_constraints.push(dep_spec);
                                    required.insert(dep_name.clone(), true);
                                    let dn: Arc<str> = dep_name.into();
                                    queue.push_back((dn, depth + 1));
                                }
                            }
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            // Try to get from transitive cache first
            let snapshot_hit = if transitive_cache_enabled() {
                transitive_metadata_snapshot()
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&name).map(std::sync::Arc::clone))
            } else {
                None
            };

            if snapshot_hit.is_some() {
                metadata_cache.insert(name.clone(), snapshot_hit.unwrap());
            } else {
                // Defer fetching - collect all needed packages and fetch in parallel
                continue;
            }
        }

        // Second pass: fetch all deferred packages in parallel
        let deferred_names: Vec<String> = queue
            .iter()
            .filter(|(n, _)| !metadata_cache.contains_key(&n.to_string()))
            .map(|(n, _)| n.to_string())
            .collect();

        if !deferred_names.is_empty() {
            let fetch_results = registry::parallel_fetch_metadata(
                &deferred_names,
                // `deferred_names` was filtered to names NOT in `metadata_cache`,
                // so the fetcher's cached branch cannot hit; passing a clone of the
                // whole map was a deep copy of every packument, per BFS round.
                &std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
            );
            for (pkg_name, result) in fetch_results {
                match result {
                    Ok(meta) => {
                        metadata_cache.insert(pkg_name.clone(), std::sync::Arc::new(meta.clone()));
                        if transitive_cache_enabled() {
                            if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                                cache.insert(pkg_name, std::sync::Arc::new(meta));
                            }
                        }
                    }
                    Err(e) => {
                        if required.get(&pkg_name) == Some(&false) {
                            crate::utils::log(&format!(
                                "warning: skipping optional dependency {} ({})",
                                pkg_name, e
                            ));
                            continue;
                        }
                        return Err(format!("Failed to fetch metadata for {}: {}", pkg_name, e));
                    }
                }
            }
        }

        let mut metadata = match metadata_cache.get(&name).cloned() {
            Some(m) => m,
            None => continue,
        };

        let peer_constraints: &[String] =
            peer_specs.get(&name).map(|v| v.as_slice()).unwrap_or(&[]);
        let version = match resolve_preferring_peers(&metadata, &pkg_constraints, peer_constraints)
        {
            Some(v) => v,
            None => {
                if !crate::registry::offline_mode() {
                    if let Ok(fresh_meta) = registry::fetch_metadata_fresh(&name) {
                        let fresh_arc = std::sync::Arc::new(fresh_meta);
                        metadata = fresh_arc.clone();
                        metadata_cache.insert(name.clone(), fresh_arc.clone());
                        if transitive_cache_enabled() {
                            if let Ok(mut cache) = transitive_metadata_snapshot().lock() {
                                cache.insert(name.clone(), fresh_arc);
                            }
                        }
                        if let Some(v) =
                            resolve_preferring_peers(&metadata, &pkg_constraints, peer_constraints)
                        {
                            v
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        };

        let is_required = *required.get(&name).unwrap_or(&true);
        if !version_supported_on_current_platform(&metadata, &version) {
            if !is_required {
                optional_variants.push(optional_variant_from_meta(&name, &version, &metadata));
            }
            continue;
        }

        // Optional dep installed on this platform: also mark it `optional` in
        // the lockfile so other platforms skip it.
        if !is_required {
            optional_variants.push(optional_variant_from_meta(&name, &version, &metadata));
        }

        let changed = selected
            .get(&name)
            .map(|existing| existing != &version)
            .unwrap_or(true);

        if !changed {
            continue;
        }

        selected.insert(name.clone(), version.clone());
    }

    // Peer consistency: warn (or fail under `--strict-peer-deps`) when a
    // selected peer version violates a dependent's peer range. Version
    // selection tries to avoid this; when it can't (a genuine conflict) we keep
    // the dependency-valid version and surface the problem instead of silently
    // dropping the package.
    let mut peer_conflicts: Vec<String> = Vec::new();
    let mut peer_names: Vec<&String> = peer_specs.keys().collect();
    peer_names.sort();
    for pkg in peer_names {
        let Some(selected_version) = selected.get(pkg) else {
            peer_conflicts.push(format!("missing peer {}", pkg));
            continue;
        };
        for spec in &peer_specs[pkg] {
            if !registry::version_satisfies(spec, selected_version) {
                peer_conflicts.push(format!(
                    "peer {} {} not satisfied by {}",
                    pkg, spec, selected_version
                ));
            }
        }
    }
    if !peer_conflicts.is_empty() {
        let strict = std::env::var("JHOL_STRICT_PEER_DEPS")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let msg = format!("peer dependency conflicts: {}", peer_conflicts.join("; "));
        if strict {
            return Err(msg);
        }
        crate::utils::log(&format!("warning: {}", msg));
    }

    // Validate the flat (one-version-per-name) result *here*, where both the chosen
    // versions and every package's manifest are in hand. Doing it in the caller
    // failed: the global metadata snapshot was empty at that point, so the check
    // silently saw nothing and the wrong tree was installed anyway.
    // ── JAGR-∞ is the resolver, not an advisor ───────────────────────────
    //
    // Every version choice above is an *estimate* kept for scheduling: the
    // binary/lattice fast paths and the greedy candidate pick exist to decide
    // what to fetch next. The authoritative answer comes from one place — the
    // chart solver — so the flat path cannot disagree with the nested path
    // about the same graph (that disagreement was a real bug: `p-locate
    // requires p-limit ^2.2.0, resolved 3.1.0`, then the nested layout
    // re-solved it).
    //
    // If the chart cannot be built or the solve fails, the estimate stands and
    // the conflict check below still runs. A sole resolver must not be able to
    // turn a working install into a failed one.
    // `metadata_cache` holds `Arc<Value>`s; the solver wants the values.
    let packument_values: HashMap<String, serde_json::Value> = metadata_cache
        .iter()
        .map(|(k, v)| (k.clone(), (**v).clone()))
        .collect();
    let direct_specs: HashMap<String, String> = packages
        .iter()
        .map(|p| (base_name(p).to_string(), requested_version_spec(p)))
        .collect();
    let jagr_choice = crate::jagr_infinite::select_versions(&packument_values, &direct_specs);
    if let Some(choice) = jagr_choice {
        let mut corrected = 0usize;
        for (name, version) in choice.iter() {
            if let Some(prev) = selected.get(name) {
                if prev != version {
                    corrected += 1;
                }
            }
        }
        if corrected > 0 && !quiet_hint() {
            crate::utils::log(&format!(
                "JAGR-∞: {} version(s) differ from the fetch estimate; using the chart solution",
                corrected
            ));
        }
        selected = choice;
    }

    let flat_conflict = flat_constraint_conflict(&selected, &metadata_cache);

    let mut specs: Vec<String> = selected
        .iter()
        .map(|(name, version)| format!("{}@{}", name, version))
        .collect();
    // `npm:` aliases resolve to the REAL package, which may not be pulled in by
    // anything else. Add it by its resolved version — `real@*` produced a spec
    // with no tarball (`react-is@*: download: HTTP 404`), because a literal
    // `*` is not something the downloader can pin.
    for (_alias, real) in recorded_aliases() {
        if specs.iter().any(|s| base_name(s) == real) {
            continue;
        }
        if let Some(version) = selected.get(&real) {
            specs.push(format!("{}@{}", real, version));
        }
    }
    specs.sort();
    specs.dedup();
    // A conflicting flat tree must never be cached: a later hit would skip this
    // check and could install the same incompatible package selections again.
    // The caller handles the conflict through the node-keyed layout instead.
    if flat_conflict.is_none() {
        let cache_entry = TransitiveExpansionCacheEntry {
            packages: specs.clone(),
            optional_variants: optional_variants.clone(),
        };
        save_trans_expansion_cache(&cache_key, &cache_entry);
        if transitive_cache_enabled() {
            if let Ok(mut cache) = transitive_memo_cache().lock() {
                cache.insert(cache_key, cache_entry);
            }
        }
    }
    Ok((specs, optional_variants, flat_conflict))
}

/// SHA-512 SRI for a store tarball, memoised in a sidecar file.
///
/// Writing a lockfile from a warm cache needs the integrity of every package, and
/// re-hashing all of them on every run cost ~296 ms on the ~300-package fixture
/// (measured on Apple Silicon). A content-addressed tarball's digest never changes, so
/// it is stored next to it (`<hash>.tgz.sri`) and reused on later runs.
fn cached_tarball_sri(tarball: &std::path::Path) -> Option<String> {
    let sidecar = tarball.with_extension("tgz.sri");
    if let Ok(text) = std::fs::read_to_string(&sidecar) {
        let text = text.trim();
        if text.starts_with("sha512-") {
            return Some(text.to_string());
        }
    }
    let sri = utils::sri_sha512_of_file(tarball).ok()?;
    let _ = std::fs::write(&sidecar, &sri);
    Some(sri)
}

/// Read all installed packages from node_modules
pub fn read_all_installed_packages(
    node_modules: &Path,
) -> Result<HashMap<String, crate::lockfile_write::ResolvedPackage>, String> {
    // Collect every installed package.json path, then read + parse them in
    // parallel (this was a serial fs::read_to_string + serde parse per package).
    let mut pkg_jsons: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(node_modules) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == ".bin" {
                continue;
            }
            if name.starts_with('@') && entry.path().is_dir() {
                if let Ok(scoped) = fs::read_dir(entry.path()) {
                    for scoped_entry in scoped.flatten() {
                        pkg_jsons.push(scoped_entry.path().join("package.json"));
                    }
                }
                continue;
            }
            pkg_jsons.push(entry.path().join("package.json"));
        }
    }
    let mut packages: HashMap<String, crate::lockfile_write::ResolvedPackage> =
        if pkg_jsons.len() <= 8 {
            pkg_jsons
                .iter()
                .filter_map(|p| load_installed_package(p))
                .collect()
        } else {
            crate::pool::parallel_map(pkg_jsons, 64, 8, |p| load_installed_package(&p))
                .into_iter()
                .flatten()
                .collect()
        };

    // Prefer metadata from an existing lockfile (registry-provided integrity and
    // resolved URL) over hashes derived from the local store. A self-computed
    // digest only proves "this is what is on disk", not "this is what the
    // registry published"; reusing upstream SRI keeps the lockfile verifiable by
    // npm and `jhol ci`.
    let prior = existing_lockfile_entries(node_modules);
    for (name, entry) in &prior {
        if let Some(pkg) = packages.get_mut(name) {
            if pkg.version != entry.version {
                continue;
            }
            if pkg.resolved.is_empty() {
                if let Some(r) = &entry.resolved {
                    pkg.resolved = r.clone();
                }
            }
            if pkg.integrity.is_none() {
                if let Some(i) = &entry.integrity {
                    pkg.integrity = Some(i.clone());
                }
            }
        }
    }

    // Reuse upstream descriptors before deriving a digest or guessing a URL.
    // Existing lockfile entries above remain authoritative for matching versions.
    for (name, pkg) in &mut packages {
        if let Some(item) = cached_resolved_fetch(&format!("{}@{}", name, pkg.version)) {
            if pkg.resolved.is_empty() {
                pkg.resolved = item.url;
            }
            if pkg.integrity.is_none() {
                pkg.integrity = item.integrity;
            }
        }
    }

    // Fall back to the content-addressed store for anything the lockfile did not
    // provide. Default materialization is an APFS clonefile / copy (a real dir),
    // so there is often no symlink to read the content hash from — look the hash
    // up in store_index under name@version, then hash store/<hash>.tgz to SRI.
    let cache_dir = PathBuf::from(utils::get_cache_dir());
    let store_dir = crate::content::store_dir(&cache_dir);
    let store_index = utils::read_store_index();
    let work: Vec<(String, String, bool, bool)> = packages
        .iter()
        .filter(|(_, p)| p.resolved.is_empty() || p.integrity.is_none())
        .map(|(n, p)| {
            (
                n.clone(),
                p.version.clone(),
                p.resolved.is_empty(),
                p.integrity.is_none(),
            )
        })
        .collect();
    let nm = node_modules.to_path_buf();
    let resolve_one = {
        let store_dir = store_dir.clone();
        let store_index = store_index.clone();
        let nm = nm.clone();
        move |(name, version, needs_resolved, needs_integrity): (String, String, bool, bool)| {
            let install_dir = if name.starts_with('@') {
                match name.find('/') {
                    Some(slash) => nm.join(&name[..slash]).join(&name[slash + 1..]),
                    None => nm.join(&name),
                }
            } else {
                nm.join(&name)
            };
            let mut resolved = None;
            let mut integrity = None;
            if let Some(tarball_hash) =
                store_hash_for_package(&install_dir, &name, &version, &store_index)
            {
                let tarball = store_dir.join(format!("{}.tgz", tarball_hash));
                if tarball.exists() {
                    if needs_resolved {
                        // Honour JHOL_REGISTRY / .npmrc / scoped registries.
                        resolved =
                            Some(crate::lockfile::tarball_url_from_registry(&name, &version));
                    }
                    if needs_integrity {
                        integrity = cached_tarball_sri(&tarball);
                    }
                }
            }
            (name, resolved, integrity)
        }
    };
    let updates: Vec<(String, Option<String>, Option<String>)> = if work.len() <= 16 {
        work.into_iter().map(resolve_one).collect()
    } else {
        crate::pool::parallel_map(work, 64, 4, resolve_one)
    };
    for (name, resolved, integrity) in updates {
        if let Some(pkg) = packages.get_mut(&name) {
            if pkg.resolved.is_empty() {
                if let Some(r) = resolved {
                    pkg.resolved = r;
                }
            }
            if pkg.integrity.is_none() {
                if let Some(i) = integrity {
                    pkg.integrity = Some(i);
                }
            }
        }
    }

    Ok(packages)
}

/// Registry-provided metadata recovered from an existing `package-lock.json`
/// (v3 `packages` form), keyed by package name.
struct ExistingLockfileEntry {
    version: String,
    resolved: Option<String>,
    integrity: Option<String>,
}

/// Read `node_modules/../package-lock.json` and index top-level entries by
/// package name. Nested trees (`.../node_modules/...`) are skipped here; they
/// are handled by the node-keyed layout work, not flattened into this map.
fn existing_lockfile_entries(node_modules: &Path) -> HashMap<String, ExistingLockfileEntry> {
    let mut out = HashMap::new();
    let Some(project_dir) = node_modules.parent() else {
        return out;
    };
    let path = project_dir.join("package-lock.json");
    let Ok(data) = fs::read_to_string(&path) else {
        return out;
    };
    #[derive(serde::Deserialize)]
    struct FastLockRoot<'a> {
        #[serde(borrow)]
        packages: Option<HashMap<&'a str, FastLockEntry<'a>>>,
    }
    #[derive(serde::Deserialize)]
    struct FastLockEntry<'a> {
        #[serde(borrow)]
        version: Option<&'a str>,
        #[serde(borrow)]
        resolved: Option<&'a str>,
        #[serde(borrow)]
        integrity: Option<&'a str>,
    }

    let Ok(lock) = serde_json::from_str::<FastLockRoot>(&data) else {
        return out;
    };
    let Some(pkgs) = lock.packages else {
        return out;
    };
    for (key, info) in pkgs {
        if key.is_empty() {
            continue;
        }
        let Some(name) = key.strip_prefix("node_modules/") else {
            continue;
        };
        if name.contains("/node_modules/") {
            continue;
        }
        let Some(version) = info.version else {
            continue;
        };
        let resolved = info.resolved.filter(|s| !s.is_empty()).map(str::to_string);
        let integrity = info.integrity.filter(|s| !s.is_empty()).map(str::to_string);
        out.insert(
            name.to_string(),
            ExistingLockfileEntry {
                version: version.to_string(),
                resolved,
                integrity,
            },
        );
    }
    out
}

/// Build the registry URL name segment and the tarball filename tail.
/// For `@scope/name`, returns `("@scope/name", "name")`. Otherwise returns
/// `(name, name)`.
#[allow(dead_code)]
fn url_parts_for(name: &str) -> (String, String) {
    if let Some(slash) = name.strip_prefix('@').and_then(|s| s.find('/')) {
        // name = "@scope/...rest", slash points within the rest; convert to
        // the absolute index in the original string.
        let abs = slash + 1; // strip_prefix consumed the '@'
        let tail = name[abs + 1..].to_string();
        (name.to_string(), tail)
    } else {
        (name.to_string(), name.to_string())
    }
}

/// Extract the content-addressed hash from an installed package's store link.
/// `node_modules/<pkg>` is a symlink to `store_unpacked/<shard>/<hash>/`; the
/// last path component is the hash. Returns None if the install isn't a link
/// into the content-addressed store.
fn store_hash_for_installed(install_dir: &Path) -> Option<String> {
    let meta = install_dir.symlink_metadata().ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = std::fs::read_link(install_dir).ok()?;
    target
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

/// Resolve the store tarball hash for an installed package.
/// Prefer the symlink target (link mode); otherwise use `store_index[name@version]`
/// (clonefile / copy mode — the default on APFS).
fn store_hash_for_package(
    install_dir: &Path,
    name: &str,
    version: &str,
    store_index: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(hash) = store_hash_for_installed(install_dir) {
        return Some(hash);
    }
    store_index.get(&format!("{}@{}", name, version)).cloned()
}

fn load_installed_package(
    pkg_json: &Path,
) -> Option<(String, crate::lockfile_write::ResolvedPackage)> {
    if !pkg_json.exists() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct FastInstalledManifest<'a> {
        #[serde(borrow)]
        name: Option<&'a str>,
        #[serde(borrow)]
        version: Option<&'a str>,
        #[serde(default, rename = "peerDependencies")]
        peer_dependencies: Option<HashMap<String, serde_json::Value>>,
        #[serde(default, rename = "peerDependenciesMeta")]
        peer_dependencies_meta: Option<HashMap<String, serde_json::Value>>,
    }

    let content = fs::read_to_string(pkg_json).ok()?;
    let manifest = serde_json::from_str::<FastInstalledManifest>(&content).ok()?;
    let version = manifest.version?;
    let name = manifest.name?;

    let mut peer_dependencies = HashMap::new();
    if let Some(peers) = manifest.peer_dependencies {
        for (k, v) in peers {
            peer_dependencies.insert(k, v.as_str().unwrap_or("*").to_string());
        }
    }
    let peer_dependencies_meta = manifest.peer_dependencies_meta.unwrap_or_default();

    Some((
        name.to_string(),
        crate::lockfile_write::ResolvedPackage {
            version: version.to_string(),
            resolved: String::new(),
            integrity: None,
            dependencies: HashMap::new(),
            peer_dependencies,
            peer_dependencies_meta,
        },
    ))
}

/// Source-of-truth check: is `node_modules/<pkg>` already a valid symlink
/// into store_unpacked with a `package.json` whose `version` matches?
///
/// This bypasses the store_index entirely. The store is content-addressed;
/// if the symlink target exists and the package.json version matches, the
/// package is correctly installed. This is the self-healing path that keeps
/// warm installs fast when the store_index is stale, unwritable, or missing.
fn entry_is_already_installed(project_root: &Path, install_path: &str, version: &str) -> bool {
    if !safe_lockfile_install_path(install_path) {
        return false;
    }
    let install_path = project_root.join(install_path);
    if install_path.symlink_metadata().is_err() {
        return false;
    }
    // Resolve symlinks to reject dangling targets. Ordinary package directories
    // are valid too; correctness comes from the exact path and package identity.
    let target = match install_path.canonicalize() {
        Ok(t) => t,
        Err(_) => return false,
    };
    // Target must be a directory
    if !target.is_dir() {
        return false;
    }
    // Read package.json and check version
    let pkg_json = target.join("package.json");
    let Ok(content) = std::fs::read_to_string(&pkg_json) else {
        return false;
    };
    #[derive(serde::Deserialize)]
    struct FastPkgVersion<'a> {
        #[serde(borrow)]
        version: Option<&'a str>,
    }
    if let Ok(parsed) = serde_json::from_str::<FastPkgVersion>(&content) {
        if let Some(v) = parsed.version {
            return v == version;
        }
    }
    false
}

fn safe_lockfile_install_path(value: &str) -> bool {
    if value.contains('\\') || value.contains(':') {
        return false;
    }
    let raw_parts: Vec<&str> = value.split('/').collect();
    if raw_parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return false;
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return false;
    }
    let mut components = path.components();
    matches!(components.next(), Some(std::path::Component::Normal(first)) if first == "node_modules")
        && components.next().is_some()
}

/// Remove a file, directory, or symlink without following links.
fn remove_any_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => std::fs::remove_file(path),
        Ok(_) => std::fs::remove_dir_all(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Restore a package moved aside by a process interrupted during publication.
/// Only backups carrying this target's filename are considered; ambiguous or
/// unexpected backup entries fail closed and remain untouched.
fn recover_interrupted_package_publish(target: &Path) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or("package install path has no parent")?;
    let package_name = target
        .file_name()
        .ok_or("package install path has no name")?
        .to_string_lossy();
    let backup_prefix = format!(".jhol-old-{package_name}-");

    match std::fs::symlink_metadata(target) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect package install path: {error}")),
    }

    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect package recovery directory: {error}")),
    };
    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read package recovery entry: {error}"))?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(&backup_prefix)
        {
            continue;
        }
        let backup = entry.path();
        let metadata = match std::fs::symlink_metadata(&backup) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect package backup {backup:?}: {error}")),
        };
        if !metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing to recover unexpected package backup type at {backup:?}"
            ));
        }
        backups.push(backup);
    }

    match backups.as_slice() {
        [] => Ok(()),
        [backup] => {
            match std::fs::symlink_metadata(target) {
                Ok(_) => {
                    return Err(format!(
                        "package output appeared during recovery; previous output remains at {backup:?}"
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("inspect package install path: {error}")),
            }
            std::fs::rename(backup, target)
                .map_err(|error| format!("restore interrupted package output: {error}"))
        }
        _ => Err(format!(
            "multiple interrupted package outputs match {target:?}; leaving all backups untouched"
        )),
    }
}

fn publish_extracted_package(candidate: &Path, target: &Path) -> Result<(), String> {
    recover_interrupted_package_publish(target)?;
    publish_extracted_package_with_hook(candidate, target, || {})
}

fn publish_extracted_package_with_hook(
    candidate: &Path,
    target: &Path,
    before_publish: impl FnOnce(),
) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let parent = target
        .parent()
        .ok_or("package install path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create package parent: {e}"))?;
    let package_name = target
        .file_name()
        .ok_or("package install path has no name")?
        .to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let backup = parent.join(format!(
        ".jhol-old-{package_name}-{}-{}-{nonce}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let had_target = target.symlink_metadata().is_ok();
    if had_target {
        std::fs::rename(target, &backup).map_err(|e| format!("preserve current package: {e}"))?;
    }
    before_publish();
    if let Err(error) = std::fs::rename(candidate, target) {
        if had_target && target.symlink_metadata().is_err() {
            if let Err(restore) = std::fs::rename(&backup, target) {
                return Err(format!(
                    "publish extracted package: {error}; restore previous package: {restore}; previous output remains at {backup:?}"
                ));
            }
            return Err(format!(
                "publish extracted package: {error}; previous output restored"
            ));
        }
        if had_target {
            return Err(format!(
                "publish extracted package: {error}; competing output preserved at {target:?}; previous output remains at {backup:?}"
            ));
        }
        return Err(format!(
            "publish extracted package: {error}; existing output, if any, was preserved at {target:?}"
        ));
    }
    if had_target {
        if let Err(error) = remove_any_path(&backup) {
            // The new target is already valid. A stale owned backup is safe to
            // remove later; do not roll back a completed publication here.
            crate::utils::log(&format!(
                "warning: could not remove old package backup {backup:?}: {error}"
            ));
        }
    }
    Ok(())
}

/// Faster cache lookup that reuses a preloaded store index (avoids repeated JSON parse per package).
/// Supports offline mode with normalized cache keys for reliable lookups.
fn get_cached_tarball_fast(
    package: &str,
    cache_dir: &std::path::Path,
    store_index: &HashMap<String, String>,
) -> Option<std::path::PathBuf> {
    if !cache_dir.exists() {
        return None;
    }

    let package = utils::sanitize_package_name(package)?;

    let store_dir = crate::content::store_dir(cache_dir);

    // Normalize package@version for consistent cache lookups (supports scoped packages)
    let normalized_key = if let Some(idx) = package.rfind('@') {
        if idx > 0 && !package[idx + 1..].contains('/') {
            let pkg_name = &package[..idx];
            let pkg_version = package[idx + 1..]
                .trim_start_matches('^')
                .trim_start_matches('~')
                .trim_start_matches('>')
                .trim_start_matches('<')
                .trim_start_matches('=')
                .trim_start_matches('v')
                .trim();
            format!("{}@{}", pkg_name, pkg_version)
        } else {
            package.to_string()
        }
    } else {
        package.to_string()
    };

    // Try normalized key first
    if let Some(hash) = store_index.get(&normalized_key) {
        let store_file = store_dir.join(format!("{}.tgz", hash));
        if store_file.exists() {
            return Some(store_file);
        }
        // Also check JAGR-S objects store
        if crate::jagrs::has_tarball(hash) {
            if let Some(bytes) = crate::jagrs::get_tarball(hash) {
                if std::fs::write(&store_file, bytes).is_ok() {
                    return Some(store_file);
                }
            }
        }
    }

    // Try original key
    if package.contains('@') {
        if let Some(hash) = store_index.get(&package) {
            let store_file = store_dir.join(format!("{}.tgz", hash));
            if store_file.exists() {
                return Some(store_file);
            }
        }
    } else {
        // Backward-compatible fast path for older index entries keyed by bare package name.
        if let Some(hash) = store_index.get(&package) {
            let store_file = store_dir.join(format!("{}.tgz", hash));
            if store_file.exists() {
                return Some(store_file);
            }
        }
        // Build prefix index once, then O(1) lookup (P1)
        let mut prefix_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for (k, hash) in store_index {
            if let Some(at) = k.rfind('@') {
                prefix_map.entry(&k[..at]).or_default().push(hash);
            }
        }
        // Only when the request names no version. Returning an arbitrary cached
        // tarball for a name is wrong-version behaviour: a request for
        // `@swc/helpers@0.5.23` was served this package's cached `0.5.15` tarball
        // here, and the following link failed, aborting the whole install with
        // "could not link or extract from cache for: @swc/helpers@0.5.23" (a fresh
        // cache worked, because nothing matched and it downloaded instead).
        let requests_a_version = normalized_key
            .rfind('@')
            .map(|i| {
                i > 0
                    && !normalized_key[i + 1..].is_empty()
                    && !normalized_key[i + 1..].contains('/')
            })
            .unwrap_or(false);
        if !requests_a_version {
            if let Some(hashes) = prefix_map.get(package.as_str()) {
                for &hash in hashes {
                    let store_file = store_dir.join(format!("{}.tgz", hash));
                    if store_file.exists() {
                        return Some(store_file);
                    }
                }
            }
        }
    }

    // ── Bundled package materialisation ──────────────────────────────────
    // If the package@version is one of the 200 pre-packed binary packages,
    // materialise it to the store on first access.  This avoids downloading
    // from npm and populates the store for subsequent runs.
    if !package.contains('/') || package.starts_with('@') {
        let mut batch_updates: Option<HashMap<String, String>> = None;
        for candidate in [normalized_key.as_str(), package.as_str()] {
            if let Some(tgz) = crate::registry::get_bundled_tarball(candidate) {
                use sha2::{Digest, Sha256};
                let hash = hex::encode(Sha256::digest(&tgz));
                let store_file = store_dir.join(format!("{}.tgz", hash));
                if !store_file.exists() {
                    if let Some(parent) = store_file.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::write(&store_file, tgz).is_ok() {
                        let updates = batch_updates.get_or_insert_with(HashMap::new);
                        updates.insert(candidate.to_string(), hash);
                    }
                }
                return Some(store_file);
            }
            // PRW deliberately embeds NO tarballs (binary stays small) and
            // get_bundled_tarball always returns None. The on-disk
            // binary-packages/index.json + *.jhol archives still hold 200
            // pre-packed packages — materialise from those so bundled packages
            // install without any network fetch (and work in --offline).
            if let Some((pkg, ver)) = crate::registry::split_binary_spec(candidate) {
                if let Some(hash) = crate::registry::binary_package_hash(&pkg, &ver) {
                    let store_file = store_dir.join(format!("{}.tgz", hash));
                    if store_file.exists() {
                        return Some(store_file);
                    }
                    if let Ok(Some(tgz)) = crate::registry::load_binary_tarball(&pkg, &ver) {
                        if let Some(parent) = store_file.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if std::fs::write(&store_file, &tgz).is_ok() {
                            let updates = batch_updates.get_or_insert_with(HashMap::new);
                            updates.insert(candidate.to_string(), hash);
                            return Some(store_file);
                        }
                    }
                }
            }
        }
        // Flush batch index writes once (P2)
        if let Some(updates) = batch_updates {
            let mut index = crate::utils::read_store_index();
            index.extend(updates);
            let _ = crate::utils::write_store_index(&index);
        }
    }

    // Legacy fallback: pkg-version.tgz files in cache root.
    let legacy_exact = cache_dir.join(format!("{}.tgz", utils::format_cache_name(&package)));
    if legacy_exact.exists() {
        return Some(legacy_exact);
    }

    if !package.contains('@') {
        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{}-", package))
                    && name.ends_with(".tgz")
                    && !name.contains("store")
                {
                    return Some(e.path());
                }
            }
        }
    }

    None
}

/// Check if a package is already resolved locally (installed or cached).
/// True if `node_modules/<pkg>` exists (already installed) OR if the
/// content-addressed store has a matching `store_unpacked` dir. Used to
/// skip npm validation for packages we already trust — the local store is
/// the source of truth and a tarball we already have is by definition valid.
fn package_already_resolved(package: &str, cache_dir: &Path) -> bool {
    let (name, req_version) = if let Some(idx) = package.rfind('@') {
        if idx > 0 && !package[idx + 1..].contains('/') {
            (&package[..idx], Some(&package[idx + 1..]))
        } else {
            (package, None)
        }
    } else {
        (package, None)
    };

    // node_modules check (covers scoped packages)
    let nm = PathBuf::from("node_modules");
    let install_path = if name.starts_with('@') {
        match name.find('/') {
            Some(slash) => nm.join(&name[..slash]).join(&name[slash + 1..]),
            None => nm.join(name),
        }
    } else {
        nm.join(name)
    };
    if install_path.exists() {
        if let Some(req_ver) = req_version {
            let pkg_json = install_path.join("package.json");
            if let Ok(content) = std::fs::read_to_string(&pkg_json) {
                #[derive(serde::Deserialize)]
                struct FastPkgVer<'a> {
                    #[serde(borrow)]
                    version: Option<&'a str>,
                }
                if let Ok(parsed) = serde_json::from_str::<FastPkgVer>(&content) {
                    if let Some(v) = parsed.version {
                        if v == req_ver || crate::registry::version_satisfies(req_ver, v) {
                            return true;
                        }
                    }
                }
            }
        } else {
            return true;
        }
    }
    // Content-addressed store: ONE walk per cache dir per process, then O(1)
    // lookups.
    //
    // This used to re-walk every shard and re-read + re-parse every
    // `package.json` on every call. With ~560 store dirs and one call per
    // lockfile entry that is ~15k file reads and JSON parses for a 26-package
    // install, and it grows with the store — so warm installs degraded the more
    // you had installed. Measured (medians of 5 interleaved runs) that walk was
    // the whole warm gap against Bun: 38 ms vs 23 ms. Indexing the store once
    // turns the per-entry cost into a hash lookup.
    //
    // Keyed by cache dir so separate caches (JHOL_CACHE_DIR, tests) cannot
    // poison each other. A stale positive only skips npm validation for a
    // package the store already holds; a stale negative just re-downloads.
    static IDX: OnceLock<Mutex<HashMap<PathBuf, HashMap<String, Vec<String>>>>> = OnceLock::new();
    let idx = IDX.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match idx.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let versions = guard.entry(cache_dir.to_path_buf()).or_insert_with(|| {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(shards) = std::fs::read_dir(crate::content::unpacked_root(cache_dir)) {
            for shard in shards.flatten() {
                let Ok(entries) = std::fs::read_dir(shard.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Ok(content) = std::fs::read_to_string(entry.path().join("package.json"))
                    else {
                        continue;
                    };
                    #[derive(serde::Deserialize)]
                    struct FastPkgNameVer<'a> {
                        #[serde(borrow)]
                        name: Option<&'a str>,
                        #[serde(borrow)]
                        version: Option<&'a str>,
                    }
                    let Ok(parsed) = serde_json::from_str::<FastPkgNameVer>(&content) else {
                        continue;
                    };
                    if let (Some(n), Some(v)) = (parsed.name, parsed.version) {
                        out.entry(n.to_string()).or_default().push(v.to_string());
                    }
                }
            }
        }
        out
    });
    match (versions.get(name), req_version) {
        (Some(vs), Some(req)) => vs
            .iter()
            .any(|v| v == req || crate::registry::version_satisfies(req, v)),
        (Some(_), None) => true,
        _ => false,
    }
}

fn lockfile_link_root_for_install_path(
    install_path: &Path,
    package: &str,
    node_modules: &Path,
) -> PathBuf {
    let parent = install_path.parent().unwrap_or(node_modules);
    if package.starts_with('@') {
        parent.parent().unwrap_or(node_modules).to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

fn install_lockfile_layout(options: &InstallOptions) -> Result<bool, JholError> {
    let mut profiler = InstallProfiler::new();
    let Some(entries) = lockfile::read_lockfile_install_entries_from_dir(Path::new(".")) else {
        return Ok(false);
    };
    if entries.is_empty() {
        return Ok(false);
    }

    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| {
        crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e)
    })?;
    std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| {
        crate::error_handling::utils::io_error("create_bin_dir", Some("node_modules/.bin"), e)
    })?;

    let store_index = utils::read_store_index();

    let mut to_fetch: Vec<crate::lockfile::LockfileInstallEntry> = Vec::new();
    let mut cache_hits: Vec<(crate::lockfile::LockfileInstallEntry, std::path::PathBuf)> =
        Vec::new();
    let mut already_installed: usize = 0;
    let mut missing_offline: Vec<String> = Vec::new();

    // Env-gated: why a nested/hoisted entry is or is not materialized.
    let debug_layout = std::env::var("JHOL_DEBUG_LAYOUT").is_ok();
    if debug_layout {
        crate::utils::log(&format!(
            "[layout] {} entries from the lockfile",
            entries.len()
        ));
        for e in &entries {
            crate::utils::log(&format!(
                "[layout]   {} install_path={} top_level={} resolved={}",
                e.spec,
                e.install_path,
                e.top_level,
                &e.resolved[..e.resolved.len().min(60)]
            ));
        }
    }

    let installed_layout_trusted =
        crate::instant_lock_proof::prepared_tree_is_valid(Path::new("."));
    let entries: Vec<_> = entries
        .into_iter()
        .map(|entry| {
            let installed = installed_layout_trusted
                && entry_is_already_installed(Path::new("."), &entry.install_path, &entry.version);
            (entry, installed)
        })
        .collect();
    let replacements: std::collections::HashSet<_> = entries
        .iter()
        .filter(|(_, installed)| !installed)
        .map(|(entry, _)| entry.install_path.clone())
        .collect();

    for (entry, installed) in entries {
        // Source-of-truth check: if node_modules/<pkg> is already a valid
        // symlink to a store_unpacked dir whose package.json version matches
        // entry.version, the package is correctly installed regardless of
        // whether the store_index knows about it. This is the self-healing
        // path that keeps warm installs fast even when the index is stale
        // or unwritable.
        if !safe_lockfile_install_path(&entry.install_path) {
            return Err(crate::error_handling::utils::application_error(
                "install_lockfile_layout_path",
                Some("invalid_install_path"),
                &format!("Unsafe install path in lockfile: {}", entry.install_path),
            ));
        }
        // Replacing an ancestor removes its installed descendants too.
        let parent_replaced = Path::new(&entry.install_path)
            .ancestors()
            .skip(1)
            .any(|parent| {
                parent
                    .to_str()
                    .is_some_and(|path| replacements.contains(path))
            });
        if installed && !parent_replaced {
            if debug_layout {
                crate::utils::log(&format!(
                    "[layout]   SKIP {} (hoisted node_modules/{} already at {})",
                    entry.spec, entry.package, entry.version
                ));
            }
            already_installed += 1;
            continue;
        }

        // Local-origin harvest: durable stores already hold a complete unpacked
        // tree for this integrity. Symlink into this cache → cache hit.
        if let Some(sri) = entry.integrity.as_deref() {
            if let Some((hash, foreign_unpacked, foreign_tgz)) = crate::content::find_unpacked(sri)
            {
                let local_unpacked = crate::content::unpacked_dir(&cache_dir, &hash);
                let store_dir = crate::content::store_dir(&cache_dir);
                let _ = std::fs::create_dir_all(&store_dir);
                if let Some(parent) = local_unpacked.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let mut ready = registry::store_extract_complete(&local_unpacked);
                if !ready {
                    registry::remove_store_dir_pub(&local_unpacked);
                    #[cfg(unix)]
                    {
                        ready =
                            std::os::unix::fs::symlink(&foreign_unpacked, &local_unpacked).is_ok();
                    }
                }
                if ready {
                    if debug_layout {
                        crate::utils::log(&format!(
                            "[layout]   SOURCE harvest-unpacked {} integrity={}",
                            entry.spec, sri
                        ));
                    }
                    let local_tgz = crate::content::tarball_path(&cache_dir, &hash);
                    if !local_tgz.exists()
                        && foreign_tgz.is_file()
                        && std::fs::hard_link(&foreign_tgz, &local_tgz).is_err()
                    {
                        let _ = std::fs::copy(&foreign_tgz, &local_tgz);
                    }
                    let tarball = if local_tgz.exists() {
                        local_tgz
                    } else {
                        foreign_tgz
                    };
                    cache_hits.push((entry, tarball));
                    continue;
                }
            }
        }

        // A verified unpacked harvest avoids tarball extraction. If none is
        // available, reuse a verified local tarball before considering network.
        if let Some(tarball) = get_cached_tarball_fast(&entry.spec, &cache_dir, &store_index) {
            if debug_layout {
                crate::utils::log(&format!(
                    "[layout]   SOURCE local-tarball {} path={}",
                    entry.spec,
                    tarball.display()
                ));
            }
            cache_hits.push((entry, tarball));
            continue;
        }

        if options.offline {
            missing_offline.push(entry.spec.clone());
            continue;
        }
        to_fetch.push(entry);
    }

    profiler.mark("layout_lookup");
    if options.offline && !missing_offline.is_empty() {
        return Err(crate::error_handling::utils::cache_error(
            "offline_lockfile_layout",
            None,
            &format!(
                "Offline mode: package(s) not in the content store: {}. Fetch them once with:\n  jhol prefetch\n(keep package-lock.json committed for reproducible offline installs)",
                missing_offline.join(", ")
            ),
        ));
    }

    if debug_layout && already_installed > 0 {
        crate::utils::log(&format!(
            "[layout] {} entries already installed",
            already_installed
        ));
    }

    let install_entry = |entry: &crate::lockfile::LockfileInstallEntry,
                         tarball_path: &Path|
     -> Result<(), String> {
        let install_path = Path::new(&entry.install_path);
        let spec = format!("{}@{}", entry.package, entry.version);
        if std::env::var("JHOL_DEBUG_LAYOUT").is_ok() {
            crate::utils::log(&format!(
                "[layout]   MATERIALIZE {} -> {} via tarball {}",
                spec,
                entry.install_path,
                tarball_path.display()
            ));
        }

        // Single-pass decode cache: if the bundled tarball was decoded during
        // expansion, extract directly from cached bytes — zero decompression,
        // zero store I/O.  Falls through to the store path on miss.
        // Only for top-level entries: this path installs to node_modules/<pkg>,
        // which would ignore a nested `install_path`.
        if entry.top_level && try_install_bundled_from_cache(&spec, &entry.package, node_modules) {
            materialize::finish(node_modules, &entry.package)?;
            crate::progress::mark_from_cache();
            crate::progress::mark_linked();
            return Ok(());
        }

        // Idempotency: if install_path is already a symlink to a store dir
        // containing this tarball's hash, the package is correctly installed.
        if install_path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            if let Ok(target) = std::fs::read_link(install_path) {
                if let Some(tail) = target.file_name().and_then(|n| n.to_str()) {
                    if let Some(tar_hash) = tarball_path.file_stem().and_then(|n| n.to_str()) {
                        if tail == tar_hash {
                            if entry.top_level {
                                materialize::finish(node_modules, &entry.package)?;
                            }
                            crate::progress::mark_from_cache();
                            crate::progress::mark_linked();
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Fast path: if store_unpacked already has the extracted files, link directly
        let hash = tarball_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let direct_unpacked = crate::content::unpacked_root(&cache_dir).join(hash);
        let sharded_unpacked = crate::content::unpacked_dir(&cache_dir, hash);
        let unpacked_opt = if direct_unpacked.exists() {
            Some(direct_unpacked)
        } else if sharded_unpacked.exists() {
            Some(sharded_unpacked)
        } else {
            None
        };

        if let Some(unpacked_dir) = unpacked_opt {
            let link_root =
                lockfile_link_root_for_install_path(install_path, &entry.package, node_modules);
            if utils::link_package_from_store(&unpacked_dir, &link_root, &entry.package).is_ok() {
                if entry.top_level {
                    materialize::finish(node_modules, &entry.package)?;
                }
                crate::progress::mark_from_cache();
                crate::progress::mark_linked();
                return Ok(());
            }
        }

        // Normal path: extract from store tarball
        let parent = install_path
            .parent()
            .ok_or("package install path has no parent")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let candidate = parent.join(format!(
            ".jhol-stage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let extract_result = materialize::extract(tarball_path, &candidate);
        if let Err(error) = extract_result {
            let _ = std::fs::remove_dir_all(&candidate);
            return Err(error);
        }
        publish_extracted_package(&candidate, install_path)?;
        if entry.top_level {
            materialize::finish(node_modules, &entry.package)?;
        }
        crate::progress::mark_extracted();
        crate::progress::mark_from_cache();
        crate::progress::mark_linked();
        Ok(())
    };

    let verify_store_tarball_sri = |tarball_path: &Path, expected_sri: &str| -> bool {
        let expected_sri = expected_sri.trim();
        if expected_sri.is_empty() {
            return true;
        }
        if let Some(cached) = cached_tarball_sri(tarball_path) {
            if cached.trim() == expected_sri {
                return true;
            }
        }
        utils::verify_sri(tarball_path, expected_sri)
    };

    use rayon::prelude::*;
    let mut index_batch: HashMap<String, String> = HashMap::new();
    let concurrency = download_concurrency();
    // Each immutable source is fetched once; placement is handled separately.
    let mut unique_inputs: HashMap<String, (String, String, Option<String>)> = HashMap::new();
    for entry in &to_fetch {
        if let Some((_, resolved, integrity)) = unique_inputs.get(&entry.spec) {
            if resolved != &entry.resolved || integrity != &entry.integrity {
                return Err(crate::error_handling::utils::application_error(
                    "install_lockfile_layout_source",
                    Some("conflicting_source"),
                    &format!("Conflicting locked sources for {}", entry.spec),
                ));
            }
        } else {
            unique_inputs.insert(
                entry.spec.clone(),
                (
                    entry.spec.clone(),
                    entry.resolved.clone(),
                    entry.integrity.clone(),
                ),
            );
        }
    }
    let cache_dir_clone = cache_dir.clone();
    let results = run_worker_pool(
        unique_inputs.into_values().collect(),
        concurrency,
        1,
        move |(spec, resolved, integrity)| match registry::download_tarball_to_store_hash_only(
            &resolved,
            &cache_dir_clone,
            &spec,
            integrity.as_deref(),
        ) {
            Ok(hash) => Ok((spec, hash)),
            Err(error) => Err(format!("{spec}: {error}")),
        },
    );
    profiler.mark("layout_fetch_extract");
    for result in results {
        let (spec, hash) = result.map_err(|error| {
            crate::error_handling::utils::application_error(
                "install_lockfile_layout_fetch",
                Some("download_failed"),
                &error,
            )
        })?;
        index_batch.insert(spec, hash);
    }
    for entry in to_fetch {
        let hash = index_batch.get(&entry.spec).ok_or_else(|| {
            crate::error_handling::utils::application_error(
                "install_lockfile_layout_fetch",
                Some("missing_source"),
                &entry.spec,
            )
        })?;
        let path = crate::content::tarball_path(&cache_dir, hash);
        cache_hits.push((entry, path));
    }
    // Validate cached provenance before publishing any project package.
    for (entry, tarball_path) in &cache_hits {
        if let Some(sri) = entry.integrity.as_deref() {
            if !sri.trim().is_empty() && !verify_store_tarball_sri(tarball_path, sri) {
                return Err(crate::error_handling::utils::application_error(
                    "install_lockfile_layout_integrity",
                    Some("integrity_failed"),
                    &format!(
                        "Integrity check failed for {}: cached tarball differs from lockfile",
                        entry.spec
                    ),
                ));
            }
        }
    }
    // Parents complete before children across *all* source types. Independent
    // packages at one depth still publish in parallel.
    cache_hits.sort_by_key(|(entry, _)| entry.install_path.matches("/node_modules/").count());
    for level in cache_hits.chunk_by(|(a, _), (b, _)| {
        a.install_path.matches("/node_modules/").count()
            == b.install_path.matches("/node_modules/").count()
    }) {
        level.par_iter().try_for_each(|(entry, tarball_path)| {
            install_entry(entry, tarball_path).map_err(|error| {
                crate::error_handling::utils::application_error(
                    "install_lockfile_layout_projection",
                    Some("extract_failed"),
                    &format!("{} -> {error}", entry.spec),
                )
            })
        })?;
    }

    profiler.mark("layout_projection");
    if !index_batch.is_empty() {
        let mut index = utils::read_store_index();
        index.extend(index_batch);
        if let Err(e) = utils::write_store_index(&index) {
            crate::utils::log(&format!(
                "warning: store index update failed ({}); continuing — index will be lazily repaired",
                e
            ));
        }
    }

    // Clones resolve through the authoritative project ancestry; only explicit
    // store symlinks need dependency links at their store realpaths.
    if utils::use_symlink() {
        utils::relink_store_dependencies(node_modules);
    }
    profiler.mark("layout_index_relink");

    Ok(true)
}

#[derive(Clone)]
pub struct InstallOptions {
    pub no_cache: bool,
    pub quiet: bool,
    pub backend: Backend,
    pub lockfile_only: bool,
    pub offline: bool,
    pub strict_lockfile: bool,
    pub strict_peer_deps: bool,
    /// When true, specs came from lockfile; skip npm show and use tarball URLs only (no packument).
    pub from_lockfile: bool,
    /// When true, never call Bun/npm; fail with clear error if native install fails.
    pub native_only: bool,
    /// When true, skip lifecycle scripts in backend fallback paths.
    pub no_scripts: bool,
    /// Resolver strategy string (from JHOL_RESOLVER env).
    pub resolver: String,
    /// Overrides from package.json (name → version spec).
    pub overrides: Option<std::collections::HashMap<String, String>>,
    /// Enable telemetry for this install.
    pub telemetry: bool,
    /// Optional allowlist for packages allowed to run scripts in backend fallback mode.
    pub script_allowlist: Option<std::collections::HashSet<String>>,
    /// Dependency classes to leave out (`--omit=dev|optional|peer`).
    /// The lockfile always records every class; only materialisation is
    /// filtered, so a later plain install restores the full tree.
    pub omit: crate::omit::Omit,
    /// Fail (instead of warn) when an installed package's `engines.node`
    /// excludes this runtime (`--engine-strict`).
    pub engine_strict: bool,
}

/// Lockfile entries carry exact tarball URLs, integrity values, and placement
/// paths. Use their materializer before capsule replay because it fills a cold
/// content store; the capsule path only reuses material already in that store.
fn prefer_lockfile_materialization(options: &InstallOptions) -> bool {
    options.from_lockfile
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            no_cache: false,
            quiet: false,
            backend: Backend::Npm, // lazy: avoid bun_available() subprocess at Default
            lockfile_only: false,
            offline: false,
            strict_lockfile: false,
            strict_peer_deps: false,
            from_lockfile: false,
            native_only: true,
            no_scripts: true,
            script_allowlist: None,
            resolver: String::new(),
            overrides: None,
            telemetry: false,
            omit: crate::omit::Omit::NONE,
            engine_strict: false,
        }
    }
}

fn can_use_instant_lock_proof(
    packages: &[&str],
    options: &InstallOptions,
    project_dir: &Path,
) -> bool {
    can_reuse_prepared_layout(packages, options)
        && !options.telemetry
        && crate::instant_lock_proof::prepared_tree_is_valid(project_dir)
}

fn can_reuse_prepared_layout(packages: &[&str], options: &InstallOptions) -> bool {
    (packages.is_empty() || options.from_lockfile)
        && !options.lockfile_only
        && !options.no_cache
        && !options.offline
        && !options.strict_lockfile
        && !options.strict_peer_deps
        && options.native_only
        && options.no_scripts
        && options.resolver.is_empty()
        && options.overrides.is_none()
        && options.script_allowlist.is_none()
}

/// Package name for a node-keyed tree path (`node_modules/a/node_modules/b` -> `b`).
fn tree_key_package_name(raw: &str) -> String {
    if let Some(i) = raw.rfind("node_modules/") {
        return raw[i + "node_modules/".len()..].to_string();
    }
    raw.trim_start_matches("node_modules/").to_string()
}

/// Highest version in `meta` satisfying `range`, or `None`.
fn highest_satisfying_version(meta: &serde_json::Value, range: &str) -> Option<String> {
    let versions = meta.get("versions")?.as_object()?;
    let mut best: Option<semver::Version> = None;
    for v in versions.keys() {
        let Ok(parsed) = semver::Version::parse(v) else {
            continue;
        };
        if !crate::registry::version_satisfies(range, v) {
            continue;
        }
        if best.as_ref().map(|b| parsed > *b).unwrap_or(true) {
            best = Some(parsed);
        }
    }
    best.map(|v| v.to_string())
}

/// Record every other-platform variant of an optional dependency (npm parity).
///
/// npm writes `@esbuild/linux-x64`, `@next/swc-win32-x64-msvc`, … into
/// `package-lock.json` as `optional` entries carrying `os`/`cpu`/`libc`, so one
/// lockfile drives `ci` on Linux, macOS and Windows. The JAGR resolver
/// deliberately does not walk `optionalDependencies`, so `--lockfile-only` — the
/// path meant to *produce* that portable lockfile — was writing none of them and
/// a lockfile generated here left `ci` on another OS without its binary.
///
/// This mirrors what the full install path already does: for each resolved
/// package, read its `optionalDependencies` and, for any whose selected version
/// does not run on this platform, add it as an `optional` entry. Nothing here is
/// materialized.
/// Which optional-variant tiers `collect_optional_variants` records.
///
/// npm writes every platform's binary into the lockfile so one lockfile can
/// `ci` anywhere. Those entries are dead weight on the machine that wrote them
/// — install already filters them by `os`/`cpu` — and they cost ~10-30s of
/// packument fetches on hard graphs (measured: 31s of a 34s
/// `--lockfile-only` on hard-peer-cycles). The default keeps everything this
/// machine can actually install — including its OWN platform binary, which
/// `ci` needs — and drops the rest. `JHOL_LOCKFILE_VARIANTS=all` restores full
/// npm parity (generate once, ship to every CI); `=0` records none.
fn lockfile_variants_mode() -> u8 {
    match std::env::var("JHOL_LOCKFILE_VARIANTS").as_deref() {
        Ok("1") | Ok("true") | Ok("all") | Ok("ALL") => 2,
        Ok("0") | Ok("false") | Ok("off") | Ok("none") => 0,
        _ => 1,
    }
}

/// True when the name follows the platform-binary convention (`…-linux-x64`,
/// `…-darwin-arm64`, `…-win32-x64`, `fsevents`, …) for a platform that is NOT
/// this machine. Only consulted in current-platform mode, to skip the network
/// round trip for a tarball this machine can never install. False positives are
/// harmless: the entry is an *optional* variant, which the lockfile format and
/// `ci` already permit to be absent (E40).
fn variant_name_is_foreign_platform(dep: &str) -> bool {
    if lockfile_variants_mode() == 2 {
        return false;
    }
    let name = dep.rsplit('/').next().unwrap_or(dep).to_ascii_lowercase();
    let cur_os = match std::env::consts::OS {
        "macos" => &["darwin", "macos", "apple", "osx"][..],
        "linux" => &["linux", "musl"][..],
        "windows" => &["win32", "windows"][..],
        _ => &[][..],
    };
    let cur_arch = match std::env::consts::ARCH {
        "aarch64" => &["arm64", "aarch64"][..],
        "x86_64" => &["x64", "x86_64", "amd64"][..],
        _ => &[][..],
    };
    let os_tokens = [
        &["darwin", "macos", "apple", "osx"][..],
        &["linux", "musl"][..],
        &["win32", "windows"][..],
    ];
    let arch_tokens = [&["arm64", "aarch64"][..], &["x64", "x86_64", "amd64"][..]];
    let has_foreign_os = os_tokens.iter().any(|toks| {
        toks.iter().any(|t| name.contains(t)) && !cur_os.iter().any(|c| toks.contains(c))
    });
    let has_foreign_arch = arch_tokens.iter().any(|toks| {
        toks.iter().any(|t| name.contains(t)) && !cur_arch.iter().any(|c| toks.contains(c))
    });
    has_foreign_os || has_foreign_arch
}

/// Accurate platform filter on a constructed variant (npm negation
/// semantics lives on the type itself). `all` mode records everything.
fn variant_matches_current_platform(v: &OptionalVariant) -> bool {
    if lockfile_variants_mode() == 2 {
        return true;
    }
    v.matches_current_platform()
}

fn collect_optional_variants(
    tree: &std::collections::HashMap<String, crate::lockfile_write::ResolvedPackage>,
) -> Vec<OptionalVariant> {
    // MEASURED (2026-10-08, hard-peer-cycles fixture): this phase is the real
    // cost of `--lockfile-only` on hard graphs — 31s of a 34s run, essentially
    // all registry round trips (the resolver itself is 1.9s, RVM solve 1.9ms).
    // Parallelizing the fetches was tried and reverted (0.99× warm, cold noise;
    // bursts hit registry throttling — the E5 wall). What works is fetching
    // FEWER packuments: platform-binary variants for other systems are skipped
    // before their network call by naming convention, and by their packument's
    // own `os`/`cpu` after it. See `lockfile_variants_mode` for the full-parity
    // and off switches.
    let mode = lockfile_variants_mode();
    if mode == 0 {
        return Vec::new();
    }
    let mut out: Vec<OptionalVariant> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (node_path, pkg) in tree {
        let name = tree_key_package_name(node_path);
        if name.is_empty() {
            continue;
        }
        // A cache hit: the resolver just fetched this package's packument.
        let Ok(meta) = crate::registry::fetch_metadata(&name) else {
            continue;
        };
        for (dep, range) in crate::registry::get_version_optional_dependencies(&meta, &pkg.version)
        {
            if dep.is_empty() {
                continue;
            }
            if mode == 1 && variant_name_is_foreign_platform(&dep) {
                continue; // other system's binary — never fetch its packument
            }
            let Ok(dep_meta) = crate::registry::fetch_metadata(&dep) else {
                continue;
            };
            let Some(version) = highest_satisfying_version(&dep_meta, &range) else {
                continue;
            };
            // Record *every* optional dependency, including this platform's own
            // variant: `--lockfile-only` materializes nothing, so npm parity means
            // the current platform's binary must be in the lockfile too or `ci`
            // finds no entry to install. `ci` filters the others out by
            // `os`/`cpu` at install time. Other platforms' entries are recorded
            // only in `all` mode (see the comment on the filter fns).
            if !seen.insert((dep.clone(), version.clone())) {
                continue;
            }
            let variant = optional_variant_from_meta(&dep, &version, &dep_meta);
            if mode == 1 && !variant_matches_current_platform(&variant) {
                continue;
            }
            out.push(variant);
        }
    }
    out
}

/// Only update lockfile (no node_modules). Uses native resolver and lockfile writer.
pub fn install_lockfile_only(_backend: Backend) -> Result<(), JholError> {
    let _project_lock = match crate::project_lock::ProjectLock::acquire(Path::new(".")) {
        Ok(l) => Some(l),
        Err(e) => {
            return Err(crate::error_handling::utils::config_error(
                "another install is in progress",
                Some("install lock"),
                &e,
            ))
        }
    };
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(crate::error_handling::utils::io_error(
            "install_lockfile_only",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"),
        ));
    }
    // Peer validation is strict only when the user asked for it — the same rule
    // `install_package` applies. Enforcing it here unconditionally made
    // `jhol install --lockfile-only` fail outright on a project whose direct
    // dependency has a *required* peer (`next` requires `react`/`react-dom`)
    // instead of auto-installing that peer the way npm does — and the way the
    // full install path already did.
    let strict_peer_deps = std::env::var("JHOL_STRICT_PEER_DEPS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if strict_peer_deps {
        crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
            crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e)
        })?;
    }

    let dbg_lo = std::env::var("JHOL_DEBUG_LOCKONLY").is_ok();
    let t = std::time::Instant::now();
    let tree = crate::lockfile_write::resolve_full_tree(pj)
        .map_err(|e| crate::error_handling::utils::config_error("resolve_full_tree", None, &e))?;
    if dbg_lo {
        eprintln!("[lockonly] resolve_full_tree in {:?}", t.elapsed());
    }
    let lock_path = Path::new("package-lock.json");
    crate::lockfile_write::write_package_lock(lock_path, pj, &tree).map_err(|e| {
        crate::error_handling::utils::io_error(
            "write_package_lock",
            Some("package-lock.json"),
            std::io::Error::other(e.to_string()),
        )
    })?;
    if dbg_lo {
        eprintln!("[lockonly] write_package_lock in {:?}", t.elapsed());
    }
    // Cross-platform portability: record the other-platform variants of every
    // optional dependency, exactly as the full install path does, so this
    // lockfile also drives `ci` on Linux/Windows (npm parity).
    let variants = collect_optional_variants(&tree);
    append_optional_variants_to_lockfile(lock_path, &variants);
    if dbg_lo {
        eprintln!("[lockonly] variants+append in {:?}", t.elapsed());
    }
    // Provenance: append one chained entry for this install (IPL).
    crate::origin::log_install(lock_path);
    if dbg_lo {
        eprintln!("[lockonly] log_install in {:?}", t.elapsed());
    }
    // Soul Path: record resolution in JMR + GRM + lattice
    let fp = crate::resolution_mirror::fingerprint_deps(
        &lockfile::read_package_json_deps(pj).unwrap_or_default(),
    );
    crate::resolution_mirror::record_resolution(fp, &tree);
    if dbg_lo {
        eprintln!("[lockonly] record_resolution in {:?}", t.elapsed());
    }
    crate::soul_path::remember_all_packages(&tree);
    // The integrity promise, in this lane too: the lockfile we just wrote gets
    // SRI from every byte source jhol owns (local store + bundled store — see
    // backfill_lockfile_integrity). `--lockfile-only` used to exit before this,
    // leaving synth-served entries permanently integrity-less and `ci`/`--frozen`
    // dead-ended on them.
    backfill_lockfile_integrity_after_install();
    if dbg_lo {
        eprintln!("[lockonly] integrity backfill in {:?}", t.elapsed());
    }
    Ok(())
}

fn check_script_allowlist(
    packages: &[String],
    allowlist: &std::collections::HashSet<String>,
) -> Result<(), JholError> {
    let mut denied = Vec::new();
    for p in packages {
        let name = base_name(p).to_string();
        if !allowlist.contains(&name) {
            denied.push(name);
        }
    }
    if denied.is_empty() {
        Ok(())
    } else {
        denied.sort();
        denied.dedup();
        Err(crate::error_handling::utils::security_error(
            "check_script_allowlist",
            None,
            &format!(
                "Scripts are only allowed for allowlisted packages. Denied: {}",
                denied.join(", ")
            ),
        ))
    }
}

/// Store currently installed packages in GRM + lattice memory.
fn remember_installed_packages(node_modules: &Path) {
    let nm = node_modules.to_path_buf();
    std::thread::Builder::new()
        .name("jhol-markov-learner".into())
        .spawn(move || {
            if let Ok(pkgs) = read_all_installed_packages(&nm) {
                crate::soul_path::remember_all_packages(&pkgs);
                // Train Markov predictor with topological install order
                let names: Vec<String> = pkgs.keys().cloned().collect();
                if names.len() >= 3 {
                    crate::second_order_markov::learn_from_install(&names);
                }
            }
        })
        .ok();
}

fn sync_hidden_lockfile() {
    let src = Path::new("package-lock.json");
    if !src.exists() {
        return;
    }
    let dest = Path::new("node_modules").join(".package-lock.json");
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&dest);
    if std::fs::hard_link(src, &dest).is_err() {
        let _ = std::fs::copy(src, dest);
    }
}

/// Install dependencies from package.json (and optional package-lock.json or bun.lock). Returns list of specs to install.
/// If strict_lockfile is true, requires lockfile to exist and all deps to be in lockfile.
/// True when the lockfile's pinned versions satisfy every range the manifest declares.
///
/// `npm install` repairs a lockfile that has drifted from `package.json`; jhol reused it verbatim. A
/// real project whose manifest said `eslint: ^9` while the lockfile pinned `10.0.2` therefore kept
/// installing eslint 10 — and `jhol run lint` failed on a rule that only exists in that major. Only
/// registry ranges are compared: `file:`/`link:`/`workspace:`/git specifiers are materialised, not
/// resolved, so they are not the lockfile's business.
pub fn lockfile_matches_manifest(project_dir: &Path) -> bool {
    let deps = match crate::lockfile::registry_only_deps(&project_dir.join("package.json")) {
        Some(d) if !d.is_empty() => d,
        _ => return true,
    };
    let Some(resolved) = crate::lockfile::read_resolved_from_dir(project_dir) else {
        return true;
    };
    if resolved.is_empty() {
        return true;
    }
    for (name, range) in &deps {
        let Some(version) = resolved.get(name) else {
            continue;
        };
        if !range.is_empty() && !crate::registry::version_satisfies(range, version) {
            crate::utils::log(&format!(
                "warning: lockfile pins {}@{} but package.json asks for {} — re-resolving",
                name, version, range
            ));
            return false;
        }
    }
    true
}

/// Bare-metal fast warm materialization:
/// When package-lock.json matches package.json and all resolved packages already exist in
/// store_unpacked, directly clones the packages into node_modules via APFS clonefile / CoW
/// with batch parent directory creation, bypassing the full CLI argument parser and config searches.
pub fn fast_direct_warm_install() -> Option<Result<(), String>> {
    let root = Path::new(".");
    if !crate::instant_lock_proof::shortcut_policy_allows(root) {
        return None;
    }
    if !lockfile_matches_manifest(root) {
        return None;
    }
    let entries = crate::lockfile::read_lockfile_install_entries_from_dir(root)?;
    if entries.is_empty() {
        return None;
    }

    let cache_dir = crate::instant_lock_proof::cache_dir();
    let mut tasks = Vec::with_capacity(entries.len());
    let mut parent_dirs = std::collections::HashSet::new();
    parent_dirs.insert(PathBuf::from("node_modules"));
    parent_dirs.insert(PathBuf::from("node_modules/.bin"));

    for entry in &entries {
        let mut unpacked_opt = None;
        if let Some(sri) = entry.integrity.as_deref() {
            if let Some((_, foreign_unpacked, _)) = crate::content::find_unpacked(sri) {
                if foreign_unpacked.join(".jhol-extract-complete").is_file() {
                    unpacked_opt = Some(foreign_unpacked);
                }
            }
        }
        if unpacked_opt.is_none() {
            if let Some(h) = crate::registry::store_or_binary_hash(&entry.spec) {
                let unpacked = crate::content::unpacked_dir(&cache_dir, &h);
                if unpacked.join(".jhol-extract-complete").is_file() {
                    unpacked_opt = Some(unpacked);
                }
            }
        }

        let Some(unpacked) = unpacked_opt else {
            return None;
        };

        let dest = PathBuf::from(&entry.install_path);
        if let Some(parent) = dest.parent() {
            parent_dirs.insert(parent.to_path_buf());
        }
        tasks.push((unpacked, dest, entry.package.clone(), entry.top_level));
    }

    // 1. Batch pre-create all parent directories
    for parent in &parent_dirs {
        let _ = std::fs::create_dir_all(parent);
    }

    // 2. Parallel clone using Rayon
    use rayon::prelude::*;
    let clone_err = tasks
        .par_iter()
        .find_map_any(|(src, dest, pkg_name, top_level)| {
            if let Ok(meta) = std::fs::symlink_metadata(dest) {
                if meta.file_type().is_dir() {
                    let _ = std::fs::remove_dir_all(dest);
                } else {
                    let _ = std::fs::remove_file(dest);
                }
            }
            #[cfg(feature = "pipeline")]
            if let Err(e) = crate::jgrid::materialize::Cloner::clone_directory(src, dest) {
                return Some(e);
            }
            #[cfg(not(feature = "pipeline"))]
            if let Err(e) = crate::materialize::place(src, dest) {
                return Some(e);
            }

            if *top_level {
                if let Err(e) = crate::materialize::finish(Path::new("node_modules"), pkg_name) {
                    return Some(e);
                }
            }
            None
        });

    if let Some(err) = clone_err {
        return Some(Err(err));
    }

    // 3. Complete markers
    if let Some(identity) = crate::instant_lock_proof::project_identity(root) {
        let _ = std::fs::write(root.join("node_modules/.jhol-ok"), identity);
    }
    let _ = crate::capsule::write_capsule_from_project(root);
    write_install_markers();

    Some(Ok(()))
}

fn can_reuse_lockfile_for_requested(packages: &[&str]) -> Option<Vec<String>> {
    can_reuse_lockfile_for_requested_in(Path::new("."), packages)
}

/// Fresh-project L0 replay — the bundle lane for a project with NO lockfile yet.
///
/// A byte-identical `package.json` was fully installed on this platform before
/// (complete resolution, law layer, specials) and published under the
/// deps-manifest key with the publisher's lockfile riding inside the bundle.
/// Project it and restore that lockfile: identical reuse contract to a committed
/// lockfile — same staleness rule, and the same validation invariant (every
/// direct range must be satisfied by the bundled resolution) is re-checked here.
/// Count the package directories present in `root/node_modules`, including
/// nested copies and `@scope` children — what the installer really
/// materialized, as opposed to the requested set. Shared by the CLI summary
/// and the pre-clap up-to-date lane. Symlinks are counted but not descended
/// (loop safety); scan depth is capped as a defensive bound.
pub fn count_installed_packages(root: &Path) -> usize {
    fn walk(dir: &Path, depth: usize, out: &mut usize) {
        if depth > 32 {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            let path = entry.path();
            if std::env::var("JHOL_DEBUG_COUNT").is_ok() {
                eprintln!(
                    "[count] d{depth} entry {:?} symlink={}",
                    entry.file_name(),
                    is_symlink
                );
            }
            let name = entry.file_name();
            let name = name.to_string_lossy().into_owned();
            if name == ".bin" || name.starts_with('.') {
                continue;
            }
            if name.starts_with('@') {
                // Scope container; its children are the actual packages.
                walk(&path, depth + 1, out);
                continue;
            }
            if path.join("package.json").is_file() {
                *out += 1;
                if !is_symlink {
                    let nested = path.join("node_modules");
                    if nested.is_dir() {
                        walk(&nested, depth + 1, out);
                    }
                }
            }
        }
    }
    let nm = root.join("node_modules");
    if !nm.is_dir() {
        return 0;
    }
    let mut n = 0;
    walk(&nm, 0, &mut n);
    n
}

#[cfg(test)]
mod package_count_tests {
    use super::*;

    #[test]
    fn counts_nested_copies_and_scope_children_skips_symlink_walk() {
        let dir =
            std::env::temp_dir().join(format!("jhol-count-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let nm = dir.join("node_modules");
        for p in ["a", "@x/y", "b", "b/node_modules/c"] {
            let full = nm.join(p);
            std::fs::create_dir_all(&full).unwrap();
            std::fs::write(full.join("package.json"), b"{}").unwrap();
        }
        std::fs::create_dir_all(nm.join(".bin")).unwrap();
        // A symlinked package counts once but is not descended.
        let link_src = nm.join("a");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_src, nm.join("linked")).unwrap();

        let expected = if cfg!(unix) { 5 } else { 4 };
        assert_eq!(count_installed_packages(&dir), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_node_modules_is_zero() {
        let dir = std::env::temp_dir().join(format!("jhol-count-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(count_installed_packages(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub fn try_fresh_project_bundle_replay(root: &Path) -> bool {
    use crate::jgrid::{BundleCache, Cloner};

    if !root.join("package.json").is_file() {
        return false;
    }
    if root.join("package-lock.json").is_file() || root.join("npm-shrinkwrap.json").is_file() {
        return false; // lockfile-bound lanes own this case
    }
    let nm = root.join("node_modules");
    if let Ok(mut entries) = std::fs::read_dir(&nm) {
        if entries.next().is_some() {
            return false;
        }
    }
    if matches!(
        std::env::var("JHOL_JGRID").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    ) || matches!(
        std::env::var("JHOL_JGRID_L0").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    ) {
        return false;
    }

    let Ok(pj) = std::fs::read(root.join("package.json")) else {
        return false;
    };
    let deps_key = BundleCache::deps_manifest_key(&pj);
    let grid_dir = std::path::PathBuf::from(crate::utils::get_cache_dir()).join("grid");
    let bundles = BundleCache::new(&grid_dir);
    if !bundles.has_bundle(&deps_key) {
        return false;
    }
    let source = bundles.bundle_node_modules_path(&deps_key);
    let Some(bundle_dir) = source.parent().map(|p| p.to_path_buf()) else {
        return false;
    };
    // No lockfile here, so this replay is the only thing standing between a
    // `^` range and the registry. A bundle older than the metadata TTL may hold
    // superseded picks: fall through to a real (conditional-GET) resolution.
    if !crate::origin::within_metadata_ttl(&bundle_dir.join(".jhol-complete")) {
        return false;
    }
    let Ok(lock_bytes) = std::fs::read(bundle_dir.join("package-lock.json")) else {
        return false; // bundle predates lockfile carriage — fall through
    };
    if !bundle_lock_satisfies_manifest(root, &lock_bytes) {
        return false;
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let candidate = root.join(format!(".jhol-replay-{}-{nonce}", std::process::id()));
    if Cloner::clone_directory(&source, &candidate).is_err() {
        let _ = std::fs::remove_dir_all(&candidate);
        return false;
    }
    if std::fs::write(root.join("package-lock.json"), &lock_bytes).is_err() {
        let _ = std::fs::remove_dir_all(&candidate);
        return false;
    }
    if let Some(identity) = crate::instant_lock_proof::project_identity(root) {
        let _ = std::fs::write(candidate.join(".jhol-ok"), identity);
    }
    if std::fs::rename(&candidate, &nm).is_err() {
        let _ = std::fs::remove_dir_all(&candidate);
        return false;
    }
    write_install_markers();
    true
}

/// True when every registry direct dependency of the current manifest is
/// satisfied by the bundled lockfile's resolution. Specials (`file:`, `link:`,
/// `npm:`, git) are not the lockfile's business — same rule as
/// `lockfile_matches_manifest`.
fn bundle_lock_satisfies_manifest(root: &Path, lock_bytes: &[u8]) -> bool {
    let Some(deps) = crate::lockfile::registry_only_deps(&root.join("package.json")) else {
        return true;
    };
    if deps.is_empty() {
        return true;
    }
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(lock_bytes) else {
        return false;
    };
    let Some(packages) = lock.get("packages").and_then(|p| p.as_object()) else {
        return false;
    };
    for (name, range) in deps {
        let key = format!("node_modules/{}", name);
        let Some(version) = packages
            .get(&key)
            .and_then(|e| e.get("version"))
            .and_then(|v| v.as_str())
        else {
            return false;
        };
        if !range.is_empty() && !crate::registry::version_satisfies(&range, version) {
            return false;
        }
    }
    true
}

fn can_reuse_lockfile_for_requested_in(
    project_dir: &Path,
    packages: &[&str],
) -> Option<Vec<String>> {
    // When the caller passes an empty package list, the intent is "install
    // from package.json" — re-resolve from the manifest, don't reuse the
    // capsule (which may predate the latest package.json change). This is
    // what `jhol add` relies on: it writes the new dep to package.json and
    // calls install with an empty list, expecting the new dep to be picked up.
    if packages.is_empty() {
        return None;
    }
    // A lockfile that no longer matches package.json must not be reused: re-resolve instead (npm
    // does the same on `install`; `ci`/`--frozen` stay strict and report the mismatch).
    if !lockfile_matches_manifest(project_dir) {
        return None;
    }
    // Try capsule first — zero-parse mmap walk.
    // Only reuse the capsule if its stored manifest hash matches the current
    // package.json + package-lock.json hash. If the manifest changed (e.g.
    // `jhol add` just wrote a new dep), the capsule is stale and we must
    // re-resolve so the new dep is picked up.
    let cap_path = crate::capsule::capsule_path(project_dir);
    if cap_path.exists() {
        if let Ok(view) = crate::capsule::read_capsule(&cap_path) {
            if let Ok(current_hash) = crate::capsule::compute_manifest_hash(project_dir) {
                if view.manifest_hash() == current_hash {
                    let n = view.package_count();
                    if n > 0 {
                        let mut specs: Vec<String> = Vec::with_capacity(n);
                        let mut names: std::collections::HashMap<String, String> =
                            std::collections::HashMap::with_capacity(n);
                        for i in 0..n {
                            let e = view.entry(i);
                            let name = view.entry_name(e);
                            let version = view.entry_version(e);
                            names.insert(name.to_string(), version.to_string());
                            specs.push(format!("{}@{}", name, version));
                        }
                        // The capsule describes the *previous* resolution, so it may only
                        // answer a request whose packages it actually contains, AND exact version
                        // requested must match the capsule's version.
                        let all_match = packages.iter().all(|p| {
                            let name = base_name(p);
                            if let Some(capsule_ver) = names.get(name) {
                                if let Some(exact) = exact_version_from_spec(p) {
                                    exact == capsule_ver
                                } else {
                                    true
                                }
                            } else {
                                false
                            }
                        });
                        if all_match {
                            specs.sort();
                            specs.dedup();
                            return Some(specs);
                        }
                    }
                }
            }
        }
    }

    // Fallback: lockfile-based lookup
    let resolved = lockfile::read_resolved_from_dir(project_dir)?;

    for pkg in packages {
        let name = base_name(pkg);
        let Some(locked) = resolved.get(name) else {
            return None;
        };

        if let Some(exact) = exact_version_from_spec(pkg) {
            if exact != locked {
                return None;
            }
        }
    }

    let mut specs = lockfile::read_all_resolved_specs_from_dir(project_dir)?;
    if specs.is_empty() {
        return None;
    }
    specs.sort();
    specs.dedup();
    Some(specs)
}

/// Whether KCC³ CGA capsule path is enabled.
#[allow(dead_code)]
fn kcc3_cga_enabled() -> bool {
    std::env::var("JHOL_CGA")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

/// Whether KCC³ SNR speculation path is enabled.
#[allow(dead_code)]
fn kcc3_snr_enabled() -> bool {
    std::env::var("JHOL_SNR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

/// Phase 5: DSSC-First Resolution — check pre-computed spectral certificates
/// before running the full Knot/topological pipeline.
///
/// If ALL root packages have Stable DSSC certs, compute a quick spectral gap
/// lower bound. If the bound is large enough (gap ≥ 0.5), the dependency graph
/// is provably conflict-free and we can skip resolution entirely — just
/// extract from cache or fetch + install directly.
///
/// This is the fastest install path: O(1) cert lookup + O(packages) cache check.
/// No mmap, no Knot, no SAT, no topology.
fn dssc_first_install(
    packages: &[&str],
    options: &InstallOptions,
    profiler: &mut InstallProfiler,
) -> Result<bool, JholError> {
    // Only for empty-package (install from package.json) calls
    if !packages.is_empty() {
        return Ok(false);
    }

    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Ok(false);
    }

    let deps = match lockfile::read_package_json_deps(pj_path) {
        Some(d) if !d.is_empty() => d,
        _ => return Ok(false),
    };

    // Look up DSSC certs for all root packages
    let (all_stable, quick_gap) = crate::jagr::resolution_differential::dssc_all_stable(&deps);

    if !all_stable {
        return Ok(false);
    }

    // Quick spectral gap check: if gap ≥ 0.5, graph is provably stable
    if quick_gap < 0.5 {
        return Ok(false);
    }

    if !options.quiet {
        crate::utils::log(&format!(
            "DSSC First: all {} packages certified Stable (gap={:.3}) — greedy path",
            deps.len(),
            quick_gap
        ));
    }

    // Check if ALL packages are in cache — if so, install directly
    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let store_index = if options.no_cache {
        HashMap::new()
    } else {
        utils::read_store_index()
    };

    let mut all_cached = true;
    let mut cache_entries: Vec<(String, std::path::PathBuf)> = Vec::new(); // (spec, tarball_path)

    for (name, spec) in &deps {
        let base = if name.starts_with('@') {
            let parts: Vec<&str> = name.split('/').collect();
            if parts.len() >= 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                name.clone()
            }
        } else {
            name.clone()
        };
        let spec_str = format!("{}@{}", base, spec);

        if let Some(tarball) = get_cached_tarball_fast(&spec_str, &cache_dir, &store_index) {
            cache_entries.push((spec_str, tarball));
        } else if options.offline {
            all_cached = false;
            break;
        } else {
            // Not in cache — need to fetch
            all_cached = false;
            break;
        }
    }

    if !all_cached {
        // Some packages missing from cache — fall through to full pipeline
        return Ok(false);
    }

    // All packages in cache — parallel extract with store-link fast path
    if !options.quiet {
        crate::utils::log(&format!(
            "DSSC First: {} packages all in cache — parallel extract",
            cache_entries.len()
        ));
    }

    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| {
        crate::error_handling::utils::io_error("dssc_first_install", Some("node_modules"), e)
    })?;

    // Pre-create all parent directories (scoped packages need scope dirs)
    let mut parent_dirs = std::collections::HashSet::new();
    parent_dirs.insert(node_modules.to_path_buf());
    for (spec, _) in &cache_entries {
        let name = base_name(spec);
        if name.starts_with('@') {
            if let Some(slash) = name.find('/') {
                parent_dirs.insert(node_modules.join(&name[..slash]));
            }
        }
    }
    for parent in &parent_dirs {
        let _ = std::fs::create_dir_all(parent);
    }

    // Parallel extract: try store-link first, fall back to selective extract
    use rayon::prelude::*;
    let extract_results: Vec<(String, bool)> = cache_entries
        .par_iter()
        .map(|(spec, tarball)| {
            let name = base_name(spec);
            let pkg_dir = if name.starts_with('@') {
                if let Some(slash) = name.find('/') {
                    node_modules.join(&name[..slash]).join(&name[slash + 1..])
                } else {
                    node_modules.join(name)
                }
            } else {
                node_modules.join(name)
            };

            if pkg_dir.exists() {
                return (spec.clone(), true);
            }

            // Try store-link first (clonefile/CoW — near-zero cost)
            let tarball_path = std::path::PathBuf::from(tarball);
            if let Some(hash) = crate::registry::store_or_binary_hash(spec) {
                let unpacked = crate::content::unpacked_dir(&cache_dir, &hash);
                if unpacked.join(".jhol-extract-complete").is_file() {
                    if utils::link_package_from_store(&unpacked, node_modules, &name).is_ok() {
                        return (spec.clone(), true);
                    }
                }
            }

            // Fall back to selective extract from tarball
            let ok =
                selective_extract::extract_selective(&tarball_path, node_modules, &name).is_ok();
            (spec.clone(), ok)
        })
        .collect();

    let all_ok = extract_results.iter().all(|(_, ok)| *ok);
    if !all_ok {
        let failed: Vec<_> = extract_results
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(spec, _)| spec.clone())
            .collect();
        return Err(JholError::Application {
            operation: "dssc_first_install".to_string(),
            details: Some(failed.join(", ")),
            source: "parallel extract failed for some packages".to_string(),
        });
    }

    profiler.mark("dssc_first_path");
    // Markers written by the caller (install_package) via write_install_markers().
    Ok(true)
}

/// KCC³ classifier cascade: try fast paths first.
/// Returns Ok(true) if a path succeeded, Ok(false) to fall through, Err(e) on failure.
#[allow(dead_code)]
fn kcc3_cascade(
    packages: &[&str],
    options: &InstallOptions,
    profiler: &mut InstallProfiler,
) -> Result<bool, JholError> {
    let cap_path = crate::capsule::capsule_path(Path::new("."));
    let lockfile_present =
        Path::new("package-lock.json").exists() || Path::new("bun.lock").exists();
    if !cap_path.exists() && !lockfile_present {
        return Ok(false);
    }

    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Ok(false);
    }
    let deps = match lockfile::read_package_json_deps(pj_path) {
        Some(d) => d,
        None => return Ok(false),
    };
    if deps.is_empty() {
        return Ok(false);
    }

    // Compute knot fingerprint from deps for classifier lookup
    let deps_fp = crate::resolution_mirror::fingerprint_deps(&deps);
    let current_pbf = crate::jagr::resolution_differential::pbf_from_deps(&deps);

    // Path 0 (KPSL): knot_fp match → optional PBF trailer → manifest hash → project links
    if kcc3_cga_enabled() && cap_path.exists() {
        if let Some(view) = crate::capsule::classifier_lookup(Path::new("."), deps_fp) {
            let pbf_ok = match view.pbf() {
                Some(stored) => {
                    crate::jagr::resolution_differential::pbf_filter(stored, current_pbf)
                }
                // Legacy capsules without trailer: knot_fp already matched
                None => true,
            };
            if !pbf_ok {
                if !options.quiet {
                    crate::utils::log("KCC³ Path 0: PBF mismatch — capsule dropped");
                }
            } else {
                // Manifest match when lock present; if lock deleted (warm reinstall),
                // knot_fp + PBF alone authorize KPSL projection.
                let lock_present = Path::new("package-lock.json").exists();
                let manifest_ok = if lock_present {
                    crate::capsule::compute_manifest_hash(Path::new("."))
                        .map(|h| view.manifest_hash() == h)
                        .unwrap_or(false)
                } else {
                    true
                };
                if manifest_ok {
                    if !options.quiet {
                        crate::utils::log("KCC³ Path 0: KPSL projection — installing");
                    }
                    let result = crate::capsule::install_from_capsule(&view, Path::new("."))
                        .map_err(|e| {
                            crate::error_handling::utils::application_error(
                                "kcc3_cga_install",
                                Some("install_failed"),
                                &e,
                            )
                        });
                    if result.is_ok() {
                        profiler.mark("kcc3_path0_cga");
                        return Ok(true);
                    }
                }
            }
        }
    }

    // Path S: SNR — check JMR for this edit descriptor (add/update/remove)
    if kcc3_snr_enabled() && packages.len() <= 5 && crate::daemon_mode() {
        let edit_desc = if packages.is_empty() || packages[0].contains("remove") {
            format!("remove:{}", packages.first().unwrap_or(&""))
        } else {
            format!("add:{}", packages.join(","))
        };
        if let Some(snr_result) = crate::resolution_mirror::snr_get(deps_fp, &edit_desc) {
            if !snr_result.is_empty() && !options.quiet {
                crate::utils::log("KCC³ Path S: SNR hit — using predicted resolution");
                profiler.mark("kcc3_pathS_snr");
            }
        }
    }

    // Path B: spectral gate — skip full pipeline if graph is stable
    let (can_skip, predicted_beta1) = crate::jagr::resolution_differential::classify_gate(&deps);
    if can_skip && predicted_beta1 == 0 && !options.quiet {
        crate::utils::log("KCC³ Path B: spectral gate — graph stable, skipping full pipeline");
    }

    // Path A: lockfile validation — handled by existing capsule + lockfile paths below
    // Path C: star delta — handled by existing install logic below (knot fallback)
    Ok(false)
}

/// Build a minimal KnotRequest from package.json deps for knot graph construction.
#[allow(dead_code)]
fn build_resolve_request_from_deps(
    deps: &std::collections::HashMap<String, String>,
) -> Result<crate::jagr::knot::KnotRequest, String> {
    let mut available = std::collections::HashMap::new();
    for name in deps.keys() {
        available.insert(name.clone(), vec!["0.0.0".to_string()]);
    }
    Ok(crate::jagr::knot::KnotRequest {
        root_package: "root".into(),
        root_version: "0.0.0".into(),
        available_versions: available,
        dependencies: std::collections::HashMap::new(),
        conflicts: vec![],
        focus_packages: None,
    })
}

/// Spec list for `jhol install` with no arguments, honouring `--omit`.
/// `resolve_install_from_package_json` keeps the full-manifest behaviour for
/// callers that need every class (doctor, ci's completeness check).
pub fn resolve_install_from_package_json_omit(
    strict_lockfile: bool,
    omit: crate::omit::Omit,
) -> Result<Vec<String>, JholError> {
    if omit.is_empty() {
        return resolve_install_from_package_json(strict_lockfile);
    }
    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return resolve_install_from_package_json(strict_lockfile);
    }
    let deps = lockfile::deps_for_install(pj_path, omit).ok_or_else(|| {
        crate::error_handling::utils::config_error(
            "read_manifest_deps",
            Some("dependencies"),
            "Could not read package.json dependencies",
        )
    })?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    if strict_lockfile && resolved.is_none() {
        return Err(crate::error_handling::utils::config_error(
            "strict_lockfile_check",
            Some("package-lock.json"),
            "No lockfile found; run `jhol install` first to create one",
        ));
    }
    let mut specs: Vec<String> = Vec::with_capacity(deps.len());
    for (name, range) in deps.iter() {
        // Prefer the locked version when it satisfies the (possibly omitted)
        // manifest range and is still part of this install's class set.
        let locked = resolved
            .as_ref()
            .and_then(|m| m.get(name))
            .cloned()
            .filter(|v| crate::registry::version_satisfies(range, v));
        match locked {
            Some(v) => specs.push(format!("{}@{}", name, v)),
            None => specs.push(format!("{}@{}", name, range)),
        }
    }
    Ok(specs)
}

pub fn resolve_install_from_package_json(strict_lockfile: bool) -> Result<Vec<String>, JholError> {
    let pj_path = Path::new("package.json");
    if !pj_path.exists() {
        return Err(crate::error_handling::utils::io_error(
            "resolve_install_from_package_json",
            Some("package.json"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"),
        ));
    }
    let deps = lockfile::read_package_json_deps(pj_path).ok_or_else(|| {
        crate::error_handling::utils::config_error(
            "read_package_json_deps",
            Some("dependencies"),
            "Could not read package.json dependencies",
        )
    })?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = lockfile::read_resolved_from_dir(Path::new("."));
    if strict_lockfile {
        if resolved.is_none() {
            return Err(crate::error_handling::utils::config_error(
                "strict_lockfile_check",
                Some("lockfile"),
                "Strict lockfile required but no lockfile found (package-lock.json, npm-shrinkwrap.json, bun.lock, yarn.lock, pnpm-lock.yaml). Run install without --frozen first."
            ));
        }
        if !lockfile::lockfile_integrity_complete(Path::new(".")) {
            return Err(crate::error_handling::utils::config_error(
                "lockfile_integrity_check",
                Some("integrity"),
                "Strict lockfile: integrity entries missing. Run install without --frozen to regenerate lockfile with integrity."
            ));
        }
        let r = resolved.as_ref().ok_or_else(|| {
            crate::error_handling::utils::config_error(
                "strict_lockfile_validation",
                Some("lockfile"),
                "Lockfile data not available",
            )
        })?;
        // Two kinds of dependency legitimately have no resolved entry:
        //  * an optional dependency the resolver skipped because it does not exist
        //    (or is for another platform) — npm treats a failed optional dep as
        //    non-fatal, so strict mode must not turn it into a hard failure;
        //  * a materialized special (`file:`/`link:`/`workspace:`/git/alias), which
        //    the spec-grammar pre-pass lays down instead of resolving.
        // Requiring either made `jhol ci` / `install --frozen` fail on a project
        // that installs perfectly.
        let optional = lockfile::read_optional_dependency_names(pj_path);
        let materialized = lockfile::materialized_package_names(Path::new("."));
        for name in deps.keys() {
            if optional.contains(name) || materialized.contains(name) {
                continue;
            }
            if !r.contains_key(name) {
                return Err(crate::error_handling::utils::detailed_resolution_error(
                "strict_lockfile_validation",
                name,
                "dependency not in lockfile",
                "Strict lockfile: dependency not in lockfile. Run install without --frozen to update lockfile."
            ));
            }
        }
    }

    // When lockfile URLs are available, prefer full resolved spec list (top-level + transitive)
    // so native lockfile/offline installs can be deterministic and complete.
    let mut lockfile_needs_rebuild = false;
    if let Some(mut specs) = lockfile::read_all_resolved_specs_from_dir(Path::new(".")) {
        if !specs.is_empty() {
            // A lockfile legitimately records every platform's optional variant
            // (`@img/sharp-linuxmusl-arm64`, `@next/swc-win32-x64-msvc`, ...) so one
            // lockfile works everywhere. They must not enter the install list here:
            // it is what the user sees ("Installing: [all platforms]") and what the
            // installer is asked to fetch. Same rule as the lockfile path.
            specs = drop_platform_incompatible(specs);
            // Law layer: a locked version the manifest no longer allows must never be
            // installed. Without this, `doctor --fix` moving a package onto a major
            // outside its range was *sticky*: every later install kept the bad version
            // (verified on a real project - eslint@10.0.2 and @types/node@25.3.2 stayed
            // put while the manifest said ^9 / ^20). Replace the offending spec with the
            // declared range so the resolver picks the best in-range version.
            if !Path::new("npm-shrinkwrap.json").exists() {
                if let Some(deps) = lockfile::read_package_json_deps(Path::new("package.json")) {
                    for (name, range) in &deps {
                        if range.is_empty() {
                            continue;
                        }
                        let locked = specs
                            .iter()
                            .find(|s| base_name(s) == name.as_str())
                            .and_then(|s| s.rsplit_once('@').map(|(_, v)| v.to_string()));
                        let Some(version) = locked else { continue };
                        if crate::registry::version_satisfies(range, &version) {
                            continue;
                        }
                        crate::utils::log(&format!(
                            "lockfile pins {}@{} but package.json asks for {} - re-resolving",
                            name, version, range
                        ));
                        specs.retain(|s| base_name(s) != name.as_str());
                        specs.push(format!("{}@{}", name, range));
                        // The repaired spec list fixes the *request*, but the lockfile-layout
                        // path materialises whatever the lockfile says, so the bad pin came
                        // straight back (verified twice: eslint@10.0.2 survived both a
                        // re-resolve and a `doctor` run claiming success). A lockfile that
                        // contradicts the manifest is invalid: move it aside and let the
                        // resolver rebuild it from package.json.
                        lockfile_needs_rebuild = true;
                    }
                }
            }
            specs.sort();
            specs.dedup();
            if lockfile_needs_rebuild {
                let path = Path::new("package-lock.json");
                if path.exists() {
                    let aside = format!(
                        "package-lock.json.invalid-{}",
                        chrono::Local::now().format("%Y%m%d%H%M%S")
                    );
                    if std::fs::rename(path, &aside).is_ok() {
                        crate::utils::log(&format!(
                            "lockfile contradicted package.json - moved to {aside}; rebuilding it from the manifest"
                        ));
                    }
                }
            }
            return Ok(specs);
        }
    }

    Ok(lockfile::resolve_deps_for_install(&deps, resolved.as_ref()))
}

/// Returns Some(current_hash) if .nm_fingerprint matches current manifest hash (node_modules fresh).
/// Returns None if fingerprint missing or stale.
/// Uses OnceLock to avoid re-reading package.json/package-lock.json when mtime unchanged.
fn nm_fresh() -> Option<[u8; 8]> {
    static CACHED: std::sync::OnceLock<(u64, [u8; 8])> = std::sync::OnceLock::new();

    let read_fp = |bytes: &[u8]| -> Option<[u8; 8]> {
        // Canonical form: 8 raw hash bytes (daemon_cache::write_nm_fingerprint).
        if bytes.len() >= 8 && std::str::from_utf8(&bytes[..8]).is_err() {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&bytes[..8]);
            return Some(arr);
        }
        if bytes.len() == 8 {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(bytes);
            return Some(arr);
        }
        // Legacy: hex text.
        let stored = std::str::from_utf8(bytes).ok()?.trim();
        let decoded = hex::decode(stored).ok()?;
        if decoded.len() < 8 {
            return None;
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&decoded[..8]);
        Some(arr)
    };

    // Check mtime cache first — avoids re-reading package.json/package-lock.json
    if let Some(entry) = CACHED.get() {
        let mtime = entry.0;
        let hash = entry.1;
        if let Ok(m) = std::path::Path::new("package.json").metadata() {
            if let Ok(cm) = m.modified() {
                let cutoff = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime);
                if cm <= cutoff {
                    let fp = std::path::Path::new("node_modules/.nm_fingerprint");
                    if let Ok(bytes) = std::fs::read(fp) {
                        if let Some(arr) = read_fp(&bytes) {
                            if arr == hash {
                                return Some(hash);
                            }
                        }
                    }
                }
            }
        }
    }

    // Compute fresh hash and cache it
    let current_hash = crate::capsule::compute_manifest_hash(std::path::Path::new(".")).ok()?;
    if let Ok(m) = std::path::Path::new("package.json").metadata() {
        if let Ok(cm) = m.modified() {
            let secs = cm
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = CACHED.set((secs, current_hash));
        }
    }

    let fp = std::path::Path::new("node_modules/.nm_fingerprint");
    match std::fs::read(fp) {
        Ok(bytes) => {
            if let Some(arr) = read_fp(&bytes) {
                if arr == current_hash {
                    return Some(current_hash);
                }
            }
            None
        }
        Err(_) => None,
    }
}

/// Returns the current manifest hash as a hex string (for display/storage).
#[allow(dead_code)]
fn cached_manifest_hash() -> Option<String> {
    nm_fresh().map(hex::encode)
}

pub fn write_install_markers() {
    // Compute the manifest hash once. nm_fresh() also calls compute_manifest_hash
    // internally and then validates the fingerprint file — but on a fresh warm
    // install (node_modules just rebuilt) the fingerprint hasn't been written yet,
    // so nm_fresh() would return None and then compute_manifest_hash would run
    // a SECOND time in the or_else arm. Compute once, use everywhere.
    let hash = crate::capsule::compute_manifest_hash(std::path::Path::new(".")).ok();
    write_install_markers_light(hash);
    if let Some(h) = hash {
        // Only rewrite the capsule when missing or the embedded manifest hash
        // differs. Manifest hash sits at byte offset 24 in CapsuleHeader.
        let cap_path = crate::capsule::capsule_path(std::path::Path::new("."));
        let cap_valid = if let Ok(mut f) = std::fs::File::open(&cap_path) {
            use std::io::{Read, Seek, SeekFrom};
            let mut hdr = [0u8; 32];
            let hdr_ok = f.read_exact(&mut hdr).is_ok()
                && hdr[..8] == crate::capsule::MAGIC
                && hdr[24..32] == h;
            if !hdr_ok {
                false
            } else {
                // Reject capsules whose entries have empty tarball hashes
                // (write_capsule_from_tree used to write these for sha512 SRI).
                let pkg_count =
                    u32::from_le_bytes(hdr[12..16].try_into().unwrap_or([0; 4])) as usize;
                let mut ok = pkg_count > 0;
                let _ = f.seek(SeekFrom::Start(64));
                for _ in 0..pkg_count {
                    let mut entry = [0u8; 64];
                    if f.read_exact(&mut entry).is_err() {
                        ok = false;
                        break;
                    }
                    if entry[8..40].iter().all(|&b| b == 0) {
                        ok = false;
                        break;
                    }
                }
                ok
            }
        } else {
            false
        };
        if !cap_valid {
            let _ =
                crate::capsule::write_capsule_from_project_with_hash(std::path::Path::new("."), h);
        }
    }
    // Shared-tree prebuilds are disabled: hardlinks and symlinked project trees
    // let writes in one project mutate another project's install.
}

/// Fingerprint + status only — used after a capsule hit whose manifest hash was
/// already verified (skip re-hash and capsule rewrite).
fn write_install_markers_light(hash: Option<[u8; 8]>) {
    let Some(h) = hash else {
        return;
    };
    crate::daemon_cache::store_last_hash(h);
    // Must be synchronous: a detached thread is killed when the process exits
    // (release builds return in a few ms on the capsule path), so status +
    // fingerprint never landed and try_exit_before_clap could never fire.
    if std::path::Path::new("node_modules").exists() {
        crate::daemon_cache::write_nm_fingerprint(std::path::Path::new("."), h);
        if let Some(identity) =
            crate::instant_lock_proof::project_identity(std::path::Path::new("."))
        {
            let _ = crate::file_lock::AtomicWrite::write_to_file(
                std::path::Path::new("node_modules/.jhol-ok"),
                identity.as_bytes(),
            );
        }
    }
    crate::daemon_cache::write_status(h, std::path::Path::new("."));
}

/// Copy node_modules to a content-hash-keyed prebuilt cache directory.
/// The trampoline checks this before exec'ing the Rust binary.
#[allow(dead_code)]
fn write_prebuilt_cache() {
    let pj = std::path::Path::new("package.json");
    let nm = std::path::Path::new("node_modules");
    if !pj.exists() || !nm.exists() {
        return;
    }
    // Opt out with JHOL_PREBUILT=0. Default ON: hardlink (`cp -al`) is near-free
    // and feeds the trampoline L0 path (paper Instant Lock / whole-tree reuse).
    if std::env::var("JHOL_PREBUILT")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let pj_bytes = match std::fs::read(pj) {
        Ok(b) => b,
        Err(_) => return,
    };
    // sha256 of package.json — matches trampoline + `jhol.sh` keying
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(&pj_bytes));
    let prebuilt_dir =
        std::env::var("JHOL_PREBUILT_DIR").unwrap_or_else(|_| "/tmp/jhol-prebuilt".to_string());
    // Layout v2: <prebuilt>/<sha256(package.json)>-v2/node_modules is a
    // hardlinked copy of node_modules, and the trampoline symlinks
    // `$PWD/node_modules` to exactly that path. (v1 used `cp -r node_modules
    // <dest>` into an already-created `dest`, which nested the tree as
    // `.../node_modules/node_modules`, and the trampoline linked the *parent* —
    // so the fast path produced a broken node_modules and cost a full copy.)
    let key = format!("{}-v2", hash);
    let dest = std::path::Path::new(&prebuilt_dir)
        .join(&key)
        .join("node_modules");
    // Sidecar for the trampoline (no sha2 in that binary): always refresh.
    let _ = std::fs::write(".jhol-prebuilt-key", &hash);
    // Only build the hardlinked tree once per package.json.
    if dest.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_dir_all(&dest);
    // Hardlink the tree — near-instant, no data copy. Fall back to a real copy
    // when hardlinks are unavailable (e.g. /tmp on a different filesystem).
    let linked = std::process::Command::new("cp")
        .arg("-al")
        .arg(nm)
        .arg(&dest)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !linked {
        // Do not pay a full `cp -r` on the default path — that was a measured
        // multi-second tax. Leave prebuilt empty; trampoline falls through.
        let _ =
            std::fs::remove_dir_all(dest.parent().unwrap_or(std::path::Path::new(&prebuilt_dir)));
        let _ = std::fs::remove_file(".jhol-prebuilt-key");
    }
}

/// Capsule-first fast path: if the project capsule exists and its manifest hash
/// matches the current package.json + package-lock.json, install directly from
/// it. This is the single warm path; KCC³ / Soul Path / GRM are kept off the
/// hot install path.
fn try_capsule_install(
    project_dir: &Path,
    quiet: bool,
    packages: &[&str],
) -> Result<Option<[u8; 8]>, JholError> {
    let t_cap = std::time::Instant::now();
    let cap_path = crate::capsule::capsule_path(project_dir);
    if !cap_path.exists() {
        return Ok(None);
    }
    // Capsule identity predates policy/platform-bound proofs. Locked projects
    // may use its warm path only while the shared proof remains valid.
    if crate::instant_lock_proof::project_identity(project_dir).is_some()
        && !crate::instant_lock_proof::prepared_tree_is_valid(project_dir)
    {
        return Ok(None);
    }
    // Warm bypass: the manifest hash matches AND the recorded filesystem state (package.json, package-lock.json, node_modules mtime) still matches. The status
    // file is jhol's own; without it a deleted or added package directory would be
    // silently accepted as "already fresh".
    // skip ALL install work — packages and bin links are already up to date.
    // This saves 15-20ms of filesystem syscalls on every warm install.
    if nm_fresh().is_some() && crate::daemon_cache::can_skip_install_for_dir(project_dir) {
        // The fingerprint proves nothing *around* the tree changed. It does not
        // prove the tree is still what the lockfile authorises: a package.json
        // edited on disk and a directory the lockfile never mentions both
        // survive it. Ask the tree, not the fingerprint.
        let state = crate::tree_state::verify_tree(project_dir);
        if state.is_match() {
            return Ok(Some([0u8; 8])); // sentinel: skip marker writes (already fresh)
        }
        if std::env::var("JHOL_VERBOSE").is_ok() {
            eprintln!("[jhol] warm tree rejected: {}", state.describe());
        }
        // Unauthorised directories need no resolution to remove, so prune them
        // and re-check: a tree that was only dirty is then genuinely up to date.
        if matches!(state, crate::tree_state::TreeState::Extraneous { .. }) {
            let _ = crate::tree_state::prune_extraneous(project_dir);
            if crate::tree_state::verify_tree(project_dir).is_match() {
                return Ok(Some([0u8; 8]));
            }
        }
        // Anything else (missing package, wrong version) needs a real install.
    }
    if std::env::var("JHOL_PROFILE_INSTALL").is_ok() {
        eprintln!(
            "[jhol-profile] stage=cap_nm_fresh delta_ms={} total_ms={}",
            t_cap.elapsed().as_millis(),
            t_cap.elapsed().as_millis()
        );
    }

    let view = match crate::capsule::read_capsule(&cap_path) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if std::env::var("JHOL_PROFILE_INSTALL").is_ok() {
        eprintln!(
            "[jhol-profile] stage=cap_read delta_ms={} total_ms={}",
            t_cap.elapsed().as_millis(),
            t_cap.elapsed().as_millis()
        );
    }

    // Capsule staleness check: the capsule stores a manifest hash (xxh3 of
    // package.json || package-lock.json) at build time. If the current manifest
    // hash differs, the capsule is stale — deleting it forces the full pipeline
    // to re-resolve, which is what `jhol add` and any package.json change need.
    // Without this guard, the capsule fast path would return the pre-change
    // tree and silently drop newly added packages.
    let current_hash = crate::capsule::compute_manifest_hash(project_dir).ok();
    if std::env::var("JHOL_PROFILE_INSTALL").is_ok() {
        eprintln!(
            "[jhol-profile] stage=cap_manifest_hash delta_ms={} total_ms={}",
            t_cap.elapsed().as_millis(),
            t_cap.elapsed().as_millis()
        );
    }

    let stored_hash = Some(view.manifest_hash());
    let verified_hash = match (current_hash, stored_hash) {
        (Some(curr), Some(stored)) if curr == stored => curr,
        _ => {
            // Stale (or missing) — invalidate and fall through to full resolve.
            let _ = std::fs::remove_file(&cap_path);
            return Ok(None);
        }
    };
    // An explicit request must be satisfied by this capsule, else fall through to the
    // full pipeline. `jhol install is-number@7.0.0` does not change the manifest, so the
    // hash check below considered the older capsule fresh and replayed it, installing
    // everything except the package that was asked for.
    if !packages.is_empty() {
        let n = view.package_count();
        let mut capsule_versions: std::collections::HashMap<String, String> =
            std::collections::HashMap::with_capacity(n);
        for i in 0..n {
            let e = view.entry(i);
            capsule_versions.insert(
                view.entry_name(e).to_string(),
                view.entry_version(e).to_string(),
            );
        }
        let all_match = packages.iter().all(|p| {
            let name = base_name(p);
            if let Some(capsule_ver) = capsule_versions.get(name) {
                if let Some(exact) = exact_version_from_spec(p) {
                    exact == capsule_ver
                } else {
                    true
                }
            } else {
                false
            }
        });
        if !all_match {
            return Ok(None);
        }
    }
    if !quiet {
        crate::utils::log("Capsule hit — installing from capsule");
    }
    if std::env::var("JHOL_PROFILE_INSTALL").is_ok() {
        eprintln!(
            "[jhol-profile] stage=cap_pre_install delta_ms={} total_ms={}",
            t_cap.elapsed().as_millis(),
            t_cap.elapsed().as_millis()
        );
    }
    crate::capsule::install_from_capsule(&view, project_dir).map_err(|e| {
        crate::error_handling::utils::application_error(
            "capsule_install",
            Some("install_failed"),
            &e,
        )
    })?;
    // Markers (write_status, store_last_hash, fingerprint) are written by the
    // caller (install_package) via write_install_markers_light(verified_hash).
    Ok(Some(verified_hash))
}

/// True when `package.json` declares any dependency the spec grammar has to
/// materialize itself — an `npm:` alias, `file:`, `link:`, `workspace:`, or a
/// git URL.
///
/// The resolution-free fast paths (JGrid store replay, capsule replay) do no
/// spec handling: they cannot create an alias link and they write no lockfile.
/// A project with specials must therefore never take them.
fn package_json_has_special_deps(path: &Path) -> bool {
    crate::lockfile::read_package_json_deps(path)
        .map(|deps| {
            deps.values().any(|spec| {
                !matches!(
                    crate::spec_grammar::classify(spec),
                    crate::spec_grammar::Spec::Registry(_)
                )
            })
        })
        .unwrap_or(false)
}

/// Try JGrid pipeline fast path — content-addressed, clone-ready materialization.
/// Returns Ok(true) if materialized, Ok(false) if cache miss.
#[cfg(feature = "pipeline")]
fn try_jgrid_install(packages: &[String], options: &InstallOptions) -> Result<bool, JholError> {
    use crate::jgrid::{BundleCache, JGridPipeline, LayoutPackage, ResolvedPackage};

    if packages.is_empty()
        || options.strict_lockfile
        || !options.no_scripts
        || options.strict_peer_deps
        || options.lockfile_only
        || options.no_cache
        || matches!(
            std::env::var("JHOL_JGRID").as_deref(),
            Ok("0") | Ok("false") | Ok("no")
        )
    {
        return Ok(false);
    }

    // Declines rather than silently half-installing: this path replays a
    // resolved tree from the JGrid store without running the spec grammar, so a
    // project with an `npm:` alias (or `file:`/`link:`/`workspace:`/git dep)
    // would come out with the package missing, no alias link and no lockfile,
    // exiting 0. With the guard the install falls through to the real path.
    if package_json_has_special_deps(Path::new("package.json")) {
        return Ok(false);
    }

    // Prefer attempting when a durable L0/L1 artifact already exists. Opening an
    // empty store.jgrid just to miss is the measured ~880 ms tax (E37/E39).
    let force = matches!(
        std::env::var("JHOL_JGRID").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    let grid_dir = Path::new(&crate::utils::get_cache_dir()).join("grid");
    let store_path = grid_dir.join("store.jgrid");
    let populated_store = store_path
        .metadata()
        .map(|m| m.len() > 4096)
        .unwrap_or(false);
    let has_bundles = grid_dir.join("bundles").is_dir();
    if let Some(identity) = crate::instant_lock_proof::project_identity(Path::new(".")) {
        if !has_bundles
            || matches!(
                std::env::var("JHOL_JGRID_L0").as_deref(),
                Ok("0") | Ok("false") | Ok("no")
            )
        {
            return Ok(false);
        }
        let bundles = BundleCache::new(&grid_dir);
        let source = bundles.bundle_node_modules_path(&identity);
        if !bundles.has_bundle(&identity)
            || !crate::instant_lock_proof::locked_layout_is_valid(Path::new("."), &source)
        {
            return Ok(false);
        }
        let nm = Path::new("node_modules");
        let _lock = crate::file_lock::FileLock::lock(nm).map_err(|error| {
            crate::error_handling::utils::application_error(
                "jgrid_publication",
                None,
                &error.to_string(),
            )
        })?;
        let candidate = crate::jgrid::materialize::unique_path(Path::new("."), "node_modules")
            .map_err(|error| {
                crate::error_handling::utils::application_error("jgrid_candidate", None, &error)
            })?;
        let result = crate::jgrid::Cloner::clone_directory(&source, &candidate)
            .and_then(|()| crate::jgrid::materialize::publish_directory(&candidate, nm));
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&candidate);
            return Err(crate::error_handling::utils::application_error(
                "jgrid_projection",
                None,
                &error,
            ));
        }
        crate::jgrid::materialize::emit_route("L0-verified");
        return Ok(true);
    }

    let layout: Vec<LayoutPackage> = packages
        .iter()
        .map(|s| {
            if let Some((name, version)) = s.rsplit_once('@') {
                if !name.is_empty() {
                    let resolved_version =
                        if version == "latest" || version == "*" || version.is_empty() {
                            registry::lookup_lattice_for_current_identity(name, "latest")
                                .unwrap_or_else(|| version.to_string())
                        } else {
                            version.to_string()
                        };
                    return LayoutPackage {
                        name: name.to_string(),
                        version: resolved_version,
                        install_path: name.to_string(),
                    };
                }
            }
            let resolved_version = registry::lookup_lattice_for_current_identity(s, "latest")
                .unwrap_or_else(|| "latest".to_string());
            LayoutPackage {
                name: s.clone(),
                version: resolved_version,
                install_path: s.clone(),
            }
        })
        .collect();

    let has_unpacked = !layout.is_empty()
        && layout.iter().all(|pkg| {
            crate::jgrid::materialize::store_bridge_unpacked_ready(&pkg.name, &pkg.version)
        });

    if !force && !populated_store && !has_bundles && !has_unpacked {
        return Ok(false);
    }

    let nm = Path::new("node_modules");
    let _ = std::fs::create_dir_all(&grid_dir);

    // Fast path: L0 / L1-store without opening store.jgrid (no 880 ms empty-store tax).
    if has_bundles || has_unpacked {
        let bundles = BundleCache::new(&grid_dir);
        match crate::jgrid::materialize::materialize_from_store_unpacked(&layout, nm, &bundles) {
            Ok(true) => {
                return Ok(true);
            }
            Ok(false) => {}
            Err(e) => {
                if std::env::var("JHOL_VERBOSE").is_ok() {
                    crate::utils::log(&format!("JGrid L1-store failed: {e}"));
                }
            }
        }
        // Light L0-only hit (bundles present but store_unpacked incomplete for this set).
        if has_bundles {
            let light_refs: Vec<(&str, &str)> = layout
                .iter()
                .map(|p| (p.name.as_str(), p.version.as_str()))
                .collect();
            let light_hash = BundleCache::compute_dep_set_hash(&light_refs);
            let hit_hash = if bundles.has_bundle(&light_hash) {
                Some(light_hash.clone())
            } else {
                let spec_refs: Vec<(&str, &str)> = packages
                    .iter()
                    .map(|s| {
                        if let Some((n, v)) = s.rsplit_once('@') {
                            (n, v)
                        } else {
                            (s.as_str(), "latest")
                        }
                    })
                    .collect();
                let spec_hash = BundleCache::compute_dep_set_hash(&spec_refs);
                if bundles.has_bundle(&spec_hash) {
                    Some(spec_hash)
                } else if let Ok(mh) = crate::capsule::compute_manifest_hash(Path::new(".")) {
                    let mh_str = format!("{:016x}", u64::from_le_bytes(mh));
                    if bundles.has_bundle(&mh_str) {
                        Some(mh_str)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(bundle_key) = hit_hash {
                let bundle_nm = bundles.bundle_node_modules_path(&bundle_key);
                crate::jgrid::Cloner::clone_directory(&bundle_nm, nm).map_err(|e| {
                    crate::error_handling::utils::application_error("jgrid_l0_clone", None, &e)
                })?;
                crate::jgrid::materialize::emit_route("L0");
                let _ = crate::bin_links::rebuild_bin_links(nm);
                let _ = bundles.alias_bundle(&bundle_key, &light_hash);
                let spec_refs: Vec<(&str, &str)> = packages
                    .iter()
                    .map(|s| {
                        if let Some((n, v)) = s.rsplit_once('@') {
                            (n, v)
                        } else {
                            (s.as_str(), "latest")
                        }
                    })
                    .collect();
                let spec_hash = BundleCache::compute_dep_set_hash(&spec_refs);
                let _ = bundles.alias_bundle(&bundle_key, &spec_hash);
                if let Ok(mh) = crate::capsule::compute_manifest_hash(Path::new(".")) {
                    let mh_str = format!("{:016x}", u64::from_le_bytes(mh));
                    let _ = bundles.alias_bundle(&bundle_key, &mh_str);
                }
                return Ok(true);
            }
        }
    }

    if !force && !populated_store {
        return Ok(false);
    }

    let pipeline = JGridPipeline::new(&grid_dir)
        .map_err(|e| crate::error_handling::utils::application_error("jgrid_init", None, &e))?;

    let resolved: Vec<ResolvedPackage> = layout
        .iter()
        .map(|p| ResolvedPackage {
            name: p.name.clone(),
            version: p.version.clone(),
        })
        .collect();

    let materialized = pipeline.materialize(&resolved, nm).map_err(|e| {
        crate::error_handling::utils::application_error("jgrid_materialize", None, &e)
    })?;

    if materialized {
        let report = crate::bin_links::rebuild_bin_links(nm).map_err(|e| {
            crate::error_handling::utils::application_error("jgrid_bin_links", None, &e)
        })?;
        if report.links_created > 0 && std::env::var("JHOL_VERBOSE").is_ok() {
            crate::utils::log(&format!(
                "JGrid: linked {} bin entr{}",
                report.links_created,
                if report.links_created == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }
    }

    Ok(materialized)
}

/// After a successful traditional install: publish a durable L0 clonefile bundle
/// (always — one APFS clone of node_modules), and optionally ingest L2 blocks.
#[cfg(feature = "pipeline")]
fn post_install_jgrid_ingest(
    known_packages: Option<&HashMap<String, crate::lockfile_write::ResolvedPackage>>,
) {
    use crate::jgrid::BundleCache;
    use crate::jgrid::{JGridPipeline, PackageIngest, ResolvedPackage};
    use std::time::Instant;

    if matches!(
        std::env::var("JHOL_JGRID").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    ) {
        return;
    }
    let prepared_identity = crate::instant_lock_proof::project_identity(Path::new("."));
    let route = std::env::var_os("JHOL_JGRID_ROUTE").is_some()
        || std::env::var_os("JHOL_DEBUG_JGRID").is_some();
    if route {
        eprintln!("[JGRID] post-install");
    }

    let start = Instant::now();
    let nm = Path::new("node_modules");
    if !nm.is_dir() {
        return;
    }

    let packages_storage;
    let packages = match known_packages {
        Some(pkgs) => pkgs,
        None => {
            let Ok(pkgs) = read_all_installed_packages(nm) else {
                if route {
                    eprintln!("[JGRID] post-install: read installed packages failed");
                }
                return;
            };
            packages_storage = pkgs;
            &packages_storage
        }
    };
    if packages.is_empty() {
        if route {
            eprintln!("[JGRID] post-install: no installed packages");
        }
        return;
    }

    let pipeline_dir = Path::new(&crate::utils::get_cache_dir()).join("grid");
    // L0 publish only needs BundleCache — do NOT open/create store.jgrid here.
    // Creating an empty store on every install was the E37 disk tax, and a create
    // failure used to skip L0 entirely (silent world-class miss).
    let l0_off = matches!(
        std::env::var("JHOL_JGRID_L0").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    );
    if !l0_off {
        let _ = std::fs::create_dir_all(&pipeline_dir);
        let bundles = BundleCache::new(&pipeline_dir);
        let pairs: Vec<(String, String)> = packages
            .iter()
            .map(|(n, info)| (n.clone(), info.version.clone()))
            .collect();
        match crate::jgrid::materialize::publish_l0_from_node_modules(
            &pairs,
            nm,
            &bundles,
            prepared_identity.as_deref(),
        ) {
            Ok(published) => {
                if published && (route || std::env::var("JHOL_VERBOSE").is_ok()) {
                    crate::utils::log(&format!(
                        "JGrid: published L0 bundle ({} packages) in {:?}",
                        pairs.len(),
                        start.elapsed()
                    ));
                }
                let refs: Vec<(&str, &str)> = pairs
                    .iter()
                    .map(|(n, v)| (n.as_str(), v.as_str()))
                    .collect();
                let full_hash = BundleCache::compute_dep_set_hash(&refs);

                // Fresh-project replay key: a byte-identical package.json on THIS
                // platform maps to this bundle even before the new project has a
                // lockfile. The bundle also carries the publisher's lockfile so the
                // replay lane can restore a complete, valid project state — same
                // reuse contract (and staleness rule) as a committed lockfile.
                // Manifest-keyed aliasing is only sound for a request that IS
                // the manifest. With CLI packages the bundle answers a
                // different question, and aliasing it here made an unrelated
                // project with the same package.json replay this tree.
                let manifest_only = !request_has_packages();
                if manifest_only {
                    let pj = std::fs::read("package.json").unwrap_or_default();
                    let deps_key = crate::jgrid::BundleCache::deps_manifest_key(&pj);
                    let mut targets: Vec<String> = vec![full_hash.clone()];
                    if let Some(id) = prepared_identity.as_ref() {
                        if !id.is_empty() && *id != full_hash {
                            targets.push(id.clone());
                        }
                    }
                    for t in &targets {
                        let _ = bundles.alias_bundle(t, &deps_key);
                    }
                    if let Ok(lock_bytes) = std::fs::read("package-lock.json") {
                        for t in &targets {
                            if let Some(bdir) = bundles
                                .bundle_node_modules_path(t)
                                .parent()
                                .map(|p| p.to_path_buf())
                            {
                                if bdir.is_dir() {
                                    let lock_in_bundle = bdir.join("package-lock.json");
                                    let up_to_date = std::fs::read(&lock_in_bundle)
                                        .map(|b| b == lock_bytes)
                                        .unwrap_or(false);
                                    if !up_to_date {
                                        let _ = std::fs::write(&lock_in_bundle, &lock_bytes);
                                    }
                                }
                            }
                        }
                    }
                }

                // Legacy aliases remain only for projects without authoritative locks.
                if prepared_identity.is_none() {
                    if let Some(deps) = lockfile::read_package_json_deps(Path::new("package.json"))
                    {
                        // 1. Root spec hash (e.g. [("react", "latest"), ("typescript", "latest"), ...])
                        let root_spec_refs: Vec<(&str, &str)> = deps
                            .iter()
                            .map(|(n, v)| {
                                (n.as_str(), if v.is_empty() { "latest" } else { v.as_str() })
                            })
                            .collect();
                        let root_spec_hash = BundleCache::compute_dep_set_hash(&root_spec_refs);
                        let _ = bundles.alias_bundle(&full_hash, &root_spec_hash);

                        // 2. Root resolved hash (e.g. [("react", "19.0.0"), ("typescript", "5.8.2"), ...])
                        let root_resolved_pairs: Vec<(String, String)> = deps
                            .iter()
                            .map(|(n, _)| {
                                let ver = packages
                                    .get(n)
                                    .map(|info| info.version.clone())
                                    .unwrap_or_else(|| "latest".to_string());
                                (n.clone(), ver)
                            })
                            .collect();
                        let root_resolved_refs: Vec<(&str, &str)> = root_resolved_pairs
                            .iter()
                            .map(|(n, v)| (n.as_str(), v.as_str()))
                            .collect();
                        let root_resolved_hash =
                            BundleCache::compute_dep_set_hash(&root_resolved_refs);
                        let _ = bundles.alias_bundle(&full_hash, &root_resolved_hash);

                        // 3. Manifest hash alias
                        if let Ok(mh) = crate::capsule::compute_manifest_hash(Path::new(".")) {
                            let mh_str = format!("{:016x}", u64::from_le_bytes(mh));
                            let _ = bundles.alias_bundle(&full_hash, &mh_str);
                        }
                    }
                }
            }
            Err(error) => {
                if route || std::env::var("JHOL_VERBOSE").is_ok() {
                    crate::utils::log(&format!("JGrid L0 publish failed: {error}"));
                }
            }
        }
    }

    // Heavy L2 ingest stays opt-in (disk + rewrite cost).
    if !matches!(
        std::env::var("JHOL_JGRID_INGEST").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    ) {
        return;
    }

    let Ok(mut pipeline) = JGridPipeline::new(&pipeline_dir) else {
        if route {
            eprintln!("[JGRID] ingest: failed to open store.jgrid");
        }
        return;
    };

    let mut pending: Vec<(String, String, PathBuf)> = Vec::new();
    for (name, info) in packages {
        let resolved = [ResolvedPackage {
            name: name.clone(),
            version: info.version.clone(),
        }];
        if pipeline.has_bundle(&resolved)
            || pipeline.manifest_count() > 0
                && pipeline
                    .store()
                    .find_manifest(name, &info.version)
                    .is_some()
        {
            continue;
        }

        let pkg_dir = nm.join(name);
        if !pkg_dir.is_dir() {
            continue;
        }
        pending.push((name.clone(), info.version.clone(), pkg_dir));
    }

    let batch =
        pending.into_iter().filter_map(
            |(name, version, pkg_dir)| match read_package_files_for_jgrid(&pkg_dir) {
                Ok(files) if !files.is_empty() => Some(PackageIngest {
                    name,
                    version,
                    files,
                }),
                _ => None,
            },
        );
    let ingested = match pipeline.ingest_batch(batch) {
        Ok(count) => count as u32,
        Err(error) => {
            if std::env::var("JHOL_VERBOSE").is_ok() || route {
                crate::utils::log(&format!("JGrid batch ingest failed: {error}"));
            }
            return;
        }
    };

    if ingested > 0 {
        let elapsed = start.elapsed();
        crate::utils::log(&format!(
            "JGrid: ingested {ingested} packages in {elapsed:?}"
        ));
    }
}

/// Walk a package directory and collect (relative_path, mode, content) for all regular files.
fn read_package_files_for_jgrid(pkg_dir: &Path) -> Result<Vec<(String, u16, Vec<u8>)>, String> {
    let mut entries = Vec::new();
    collect_files_recursive(pkg_dir, pkg_dir, &mut entries, 0)?;
    Ok(entries)
}

fn collect_files_recursive(
    base_dir: &Path,
    dir: &Path,
    entries: &mut Vec<(String, u16, Vec<u8>)>,
    depth: usize,
) -> Result<(), String> {
    if depth > 20 {
        return Ok(()); // safety limit
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("read dir: {e}"))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| format!("file type: {e}"))?;

        if file_type.is_dir() {
            collect_files_recursive(base_dir, &path, entries, depth + 1)?;
        } else if file_type.is_file() {
            let rel_path = path
                .strip_prefix(base_dir)
                .map_err(|_| "strip prefix".to_string())?
                .to_string_lossy()
                .to_string();

            let metadata = fs::metadata(&path).map_err(|e| format!("metadata: {e}"))?;

            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() as u16
            };
            #[cfg(not(unix))]
            let mode = 0o644u16;

            let content = fs::read(&path).map_err(|e| format!("read: {e}"))?;

            entries.push((rel_path, mode, content));
        }
    }
    Ok(())
}

/// Return true if `package.json` is NOT strictly newer than the project
/// status file (`.jhol/status.bin`). Used to gate the Lane 0 warm bypass:
/// if a caller (e.g. `jhol add`) just wrote package.json after the status
/// was recorded, the bypass must miss so the full install re-resolves.
#[allow(dead_code)]
fn package_json_not_newer_than_status() -> bool {
    let pj = std::path::Path::new("package.json");
    let status = std::path::Path::new(".jhol/status.bin");
    let pj_mtime = match std::fs::metadata(pj).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return true, // no package.json → nothing to install anyway
    };
    let status_mtime = match std::fs::metadata(status).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return true, // no status file → bypass already missed
    };
    pj_mtime <= status_mtime
}

/// Install packages. Uses parallel validation, cache (content-addressable), native registry with backend fallback.
/// Run native lifecycle scripts for allowlisted packages, at most once per process.
///
/// The executor (`lifecycle.rs`) has always existed and compiles, but was never wired in,
/// so `--scripts`, `--script-allowlist` and `trustedDependencies` silently ran nothing —
/// verified: a package with a `postinstall` did not execute it even with the allowlist set.
/// The allowlist stays the security gate (empty means no scripts at all), and the
/// once-per-process guard keeps scripts from running twice when several exit paths are
/// taken or a fast path re-enters.
/// Record this install in the rolling JSONL log (`install_log.rs`).
///
/// That module was complete and tested but never called, so nothing ever wrote or read the
/// log — the "Recent Installs" data did not exist. Written once per process, at the same
/// successful materialization exits as the lifecycle scripts.
fn record_install_log(packages: &[String], elapsed_ms: u64) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let (requests, _success, _failed, _down, _up) = crate::http_client::get_http_metrics();
    let entry = crate::install_log::InstallLogEntry {
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        project_dir: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        packages: packages.to_vec(),
        elapsed_ms,
        // Packages that were already in the store are cache hits; the rest needed the network.
        cache_hits: (packages.len() as u64).saturating_sub(requests),
        network_requests: requests,
        success: true,
        error: None,
    };
    crate::install_log::append(&entry);
}

/// Public lifecycle entry point for lanes that complete an install without
/// reaching `install_package` (the fresh-bundle replay lane in the CLI).
pub fn run_post_install_lifecycle(options: &InstallOptions) -> Result<(), String> {
    run_native_lifecycle_scripts(options)
}

/// Lifecycle phase for the pre-clap fast lanes: they finish an install before
/// CLI options exist, so this runs with defaults (receipts consent only —
/// the legacy allowlist is a CLI-flag concept).
pub fn run_post_install_lifecycle_default() -> Result<(), String> {
    run_native_lifecycle_scripts(&InstallOptions::default())
}

fn run_native_lifecycle_scripts(options: &InstallOptions) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);

    if options.lockfile_only {
        return Ok(());
    }
    if DONE.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    // Legacy one-shot consent: an explicit allowlist trusts names for this
    // install only.
    let allowlist = options.script_allowlist.clone().unwrap_or_default();

    // Content-addressed consent: receipts from .jhol/script-receipts.json.
    let project = Path::new(".");
    let receipts = crate::lifecycle::load_receipts(project);

    // Without either consent source there is nothing to run — but packages
    // with scripts are still surfaced below so the skip is never silent.
    let policy = crate::lifecycle::ScriptPolicy {
        allowlist,
        receipts,
    };

    // Integrity lookup from the lockfile: exact "node_modules/<name>" key
    // only. A miss yields an empty integrity, which still hashes — the
    // receipt just binds to scripts+version instead of tarball bytes, and a
    // later integrity-presence change invalidates the approval (fail-safe).
    let lock_integrity = load_lockfile_integrity_map(project);
    let integrity_of = |name: &str, _version: &str| -> Option<String> {
        lock_integrity
            .get(&format!("node_modules/{}", name))
            .cloned()
    };

    let results = crate::lifecycle::run_all_install_scripts(
        Path::new("node_modules"),
        project,
        &integrity_of,
        &policy,
    );

    // npm semantics: scripts execute when a package is (re)installed, not on
    // every no-op install. The run marker is keyed to the receipt, so a
    // changed artifact (new version / new script text / new tarball) re-runs.
    let mut markers = crate::lifecycle::load_run_markers(project);
    let version_of = |pkg: &str| -> String {
        pkg.rsplit_once('@')
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    };

    let mut ran = Vec::new();
    let mut needs: Vec<(String, String)> = Vec::new();
    let mut failed = Vec::new();
    for (pkg, outcome) in &results {
        let key = format!("{}@{}", pkg, version_of(pkg));
        match outcome {
            crate::lifecycle::ScriptOutcome::Ran => {
                // Key includes version; the receipt hash itself is the marker
                // value so tamper re-runs automatically.
                let integrity = integrity_of(pkg, "");
                let receipt_for_marker = policy
                    .receipts
                    .get(&format!("{}@{}", pkg, version_of(pkg)))
                    .cloned()
                    .unwrap_or_else(|| "allowlisted".to_string());
                let _ = &integrity;
                if markers.get(&key) == Some(&receipt_for_marker) && !policy.force_all() {
                    continue; // already ran for this exact artifact
                }
                ran.push(pkg.clone());
                markers.insert(key, receipt_for_marker);
            }
            crate::lifecycle::ScriptOutcome::NeedsApproval(hash) => {
                needs.push((pkg.clone(), hash.clone()))
            }
            crate::lifecycle::ScriptOutcome::Failed(e) => failed.push((pkg.clone(), e.clone())),
            _ => {}
        }
    }
    crate::lifecycle::save_run_markers(project, &markers);

    if !options.quiet {
        for pkg in &ran {
            // Visibility for executed scripts too: the user should always be
            // able to see what ran on their machine, on every lane.
            eprintln!("Ran install scripts for {}", pkg);
        }
        for (pkg, e) in &failed {
            eprintln!("Script failed for {}: {}", pkg, e);
        }
        if !needs.is_empty() {
            // Security message: bypass output silencing. A skipped install
            // script the user never hears about is the npm supply-chain
            // failure mode this feature exists to prevent.
            eprintln!(
                "{} package(s) have install scripts that were NOT run (no approval):",
                needs.len()
            );
            for (pkg, _) in needs.iter().take(8) {
                eprintln!("  - {}", pkg);
            }
            if needs.len() > 8 {
                eprintln!("  ... and {} more", needs.len() - 8);
            }
            eprintln!("Review them, then run:  jhol approve-scripts <package ...> | --all");
        }
    }

    if let Some((pkg, e)) = failed.first() {
        // A failed approved script fails the install, exactly as npm treats
        // it: the package may be broken (a build step did not complete).
        return Err(format!("lifecycle script for {} failed: {}", pkg, e));
    }
    Ok(())
}

/// "node_modules/<name>" -> integrity SRI, from package-lock.json.
fn load_lockfile_integrity_map(project: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(project.join("package-lock.json")) else {
        return map;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return map;
    };
    if let Some(packages) = v.get("packages").and_then(|p| p.as_object()) {
        for (k, entry) in packages {
            if let Some(integ) = entry.get("integrity").and_then(|i| i.as_str()) {
                map.insert(k.clone(), integ.to_string());
            }
        }
    }
    map
}

fn materialize_downloaded_package(
    store_file: &Path,
    unpacked_dir: &Path,
    node_modules: &Path,
    package: &str,
) -> Result<(), String> {
    let base = base_name(package);
    if let Err(link_error) = utils::link_package_from_store(unpacked_dir, node_modules, base) {
        selective_extract::extract_selective(store_file, node_modules, base)
            .map_err(|extract_error| {
                format!(
                    "store materialization failed ({link_error}); archive extraction failed ({extract_error})"
                )
            })?;
    }
    bin_links::link_bins_for_package(node_modules, base)
}

/// Entry point for every install lane (capsule, L0, lane0-bypass, layout, DSSC).
/// The wrapper exists for one post-install promise: a legacy lockfile without
/// `integrity` must gain it on any successful plain install — `jhol ci`/`--frozen`
/// reject integrity-less lockfiles telling users to "run install without
/// --frozen to regenerate", and before this wrapper that instruction was a dead
/// end (no lane ever wrote integrity back). One convergence point covers all
/// lanes now and any lane added later. No-op when integrity already present.
pub fn install_package(packages: &[&str], options: &InstallOptions) -> Result<(), JholError> {
    // One install per project. Two concurrent runs interleave their writes to
    // node_modules and package-lock.json, and the tree ends up matching neither
    // — a state the verifier can then only report, not prevent.
    let _project_lock = match crate::project_lock::ProjectLock::acquire(Path::new(".")) {
        Ok(l) => Some(l),
        Err(e) => {
            return Err(crate::error_handling::utils::config_error(
                "another install is in progress",
                Some("install lock"),
                &e,
            ))
        }
    };
    let result = install_package_inner(packages, options);
    if std::env::var("JHOL_DEBUG_INTEGRITY").is_ok() {
        eprintln!("[integrity] wrapper: ok={}", result.is_ok());
    }
    if result.is_ok() {
        // npm: aliases resolved during expansion get their alias path linked
        // to the real package (node_modules/@jest/react-is-19 -> react-is).
        let aliased = materialize_aliases(Path::new("."));
        if aliased > 0 && !options.quiet {
            crate::utils::log(&format!("Linked {} npm: alias(es)", aliased));
        }
        // engines: warn by default, fail under --engine-strict. Runs before
        // patches so a mismatch is reported for the published package.
        {
            let mismatches = crate::engines::scan_installed(Path::new("."));
            if !mismatches.is_empty() {
                if options.engine_strict {
                    let list = mismatches
                        .iter()
                        .take(5)
                        .map(|(n, req, act)| format!("{} (needs {}, have {})", n, req, act))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(crate::error_handling::utils::config_error(
                        "engine_strict",
                        Some("engines"),
                        &format!(
                            "{} package(s) require a newer node: {}",
                            mismatches.len(),
                            list
                        ),
                    ));
                }
                if !options.quiet {
                    let list = mismatches
                        .iter()
                        .take(5)
                        .map(|(n, req, act)| format!("{} needs {} (have {})", n, req, act))
                        .collect::<Vec<_>>()
                        .join("; ");
                    crate::utils::log(&format!(
                        "warning: {} package(s) declare engines.node above this runtime: {}{}",
                        mismatches.len(),
                        list,
                        if mismatches.len() > 5 { "; ..." } else { "" }
                    ));
                }
            }
        }
        // Re-apply recorded patches: extraction materialised pristine files,
        // and a patch is the user's own fix for a broken dependency.
        if let Ok((pkgs, files)) = crate::patch::apply_all(Path::new(".")) {
            if pkgs > 0 && !options.quiet {
                crate::utils::log(&format!(
                    "Applied {} patch(es) to {} file(s)",
                    pkgs, files
                ));
            }
        }
        backfill_lockfile_integrity_after_install();
        // The lifecycle phase belongs at the convergence point: every lane
        // (warm bypass, capsule replay, bare-metal, full pipeline) reaches
        // here, so scripts are never silently skipped by a fast path. A
        // failed approved script fails the install, as in npm.
        run_native_lifecycle_scripts(options).map_err(|e| {
            crate::error_handling::utils::security_error("run install scripts", None, &e)
        })?;
    }
    result
}

/// Shared post-install promise: a legacy lockfile without `integrity` gains it
/// after any successful plain install, no matter which lane served it (clap
/// dispatch, bare-metal fast lane, L0 projection, or the pre-clap status bypass).
pub fn backfill_lockfile_integrity_after_install() {
    if Path::new("package-lock.json").exists() {
        let cache_dir = crate::utils::get_cache_dir();
        backfill_lockfile_integrity(Path::new("package-lock.json"), Path::new(&cache_dir));
    }
}

fn install_package_inner(packages: &[&str], options: &InstallOptions) -> Result<(), JholError> {
    let mut profiler = InstallProfiler::new();

    // --omit: build the filtered spec list before any lane can act on it.
    //
    // Two rules matter here:
    //  1. Explicitly requested packages (`jhol install foo`) are never
    //     omitted — the user asked for them.
    //  2. A warm-bypass proof, capsule, or L0 bundle describes the FULL tree.
    //     Reusing one for a reduced tree would silently install the dev
    //     dependencies the caller asked to leave out, so every fast lane is
    //     disabled when anything is omitted. Correctness over speed.
    if std::env::var_os("JHOL_OMIT_DEBUG").is_some() {
        eprintln!("[omit] install_package_inner omit={:?} packages={:?}", options.omit, packages);
    }
    // Scope the omission set for the whole install: the materialisation layer
    // consults it, and the guard restores the previous value on exit.
    let _omit_guard = crate::omit::scoped(options.omit);
    set_request_key(packages);
    let omit_filtered: Vec<String> = if options.omit.is_empty() {
        Vec::new()
    } else {
        let mut specs: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        let pj = Path::new("package.json");
        if specs.is_empty() && pj.exists() {
            let deps = lockfile::deps_for_install(pj, options.omit).unwrap_or_default();
            let resolved = lockfile::read_resolved_from_dir(Path::new("."));
            for (name, range) in deps.iter() {
                let locked = resolved
                    .as_ref()
                    .and_then(|m| m.get(name))
                    .cloned()
                    .filter(|v| crate::registry::version_satisfies(range, v));
                specs.push(match locked {
                    Some(v) => format!("{}@{}", name, v),
                    None => format!("{}@{}", name, range),
                });
            }
        }
        specs
    };
    let omit_strs: Vec<&str> = omit_filtered.iter().map(|s| s.as_str()).collect();
    let packages: &[&str] = if options.omit.is_empty() {
        packages
    } else {
        &omit_strs
    };

    let allow_prepared_replay =
        options.omit.is_empty() && can_reuse_prepared_layout(packages, options);

    // Lane 0 is limited to the plain default install and uses the same proof
    // as the front door. Mtime-only status markers do not bind lockfiles,
    // install paths, policy, or platform and therefore cannot authorize reuse.
    if options.omit.is_empty() && can_use_instant_lock_proof(packages, options, Path::new(".")) {
        // The proof says the prepared tree was valid when it was written. It
        // cannot say the tree is still what the lockfile authorises: a
        // package.json edited on disk and a directory the lockfile never
        // mentions both satisfy it. Ask the tree itself before skipping work.
        let state = crate::tree_state::reconcile_tree(Path::new("."));
        if state.is_match() {
            profiler.mark("lane0_warm_bypass");
            return Ok(());
        }
        if std::env::var("JHOL_VERBOSE").is_ok() {
            eprintln!("[jhol] lane0 rejected: {}", state.describe());
        }
        // Missing or wrong versions fall through to the real install.
    }

    // Capsule-first fast path: mmap'd zero-parse graph → install.
    // Safe for explicit requests because capsule reuse verifies the requested packages.
    if allow_prepared_replay {
        profiler.mark("before_capsule");
        match try_capsule_install(Path::new("."), options.quiet, packages) {
            Ok(Some(verified_hash)) => {
                profiler.mark("capsule_fast_path");
                // [0;8] sentinel = already-fresh bypass — no marker rewrite needed.
                if verified_hash != [0u8; 8] {
                    let pj = Path::new("package.json");
                    // This used to be a hand-written substring list that omitted
                    // `npm:`, so a project whose only special was an alias took
                    // the capsule fast path, which does no spec handling: exit 0
                    // with the aliased package missing, no alias link, and no
                    // lockfile — silently. Ask the classifier instead.
                    let has_specials = package_json_has_special_deps(pj);
                    if has_specials {
                        let ld = crate::spec_grammar::materialize_special_deps(
                            Path::new("."),
                            Path::new("node_modules"),
                        );
                        finalize_special_deps(Path::new("package-lock.json"), &ld.specials);
                    }
                    write_install_markers_light(Some(verified_hash));
                }
                #[cfg(feature = "pipeline")]
                post_install_jgrid_ingest(None);
                return Ok(());
            }
            Ok(None) => {}
            Err(e) => {
                if !options.quiet {
                    crate::utils::log(&format!("Capsule fast path failed: {}", e));
                }
            }
        }
    }

    // JGrid L0 Direct Lane: Invariant dual-projection bundle materialization.
    // If the requested or manifest dependency set has an idempotent L0 bundle in
    // ~/.jhol-cache/grid/bundles/{hash}, project it via APFS clonefile / ioctl_ficlone in O(1).
    #[cfg(feature = "pipeline")]
    if allow_prepared_replay {
        let early_jgrid_specs: Vec<String> = if !packages.is_empty() {
            packages.iter().map(|p| (*p).to_string()).collect()
        } else {
            lockfile::read_all_resolved_specs_from_dir(Path::new("."))
                .or_else(|| {
                    lockfile::read_package_json_deps(Path::new("package.json")).map(|deps| {
                        deps.into_iter()
                            .map(|(n, v)| {
                                if v.is_empty() {
                                    n
                                } else {
                                    format!("{}@{}", n, v)
                                }
                            })
                            .collect()
                    })
                })
                .unwrap_or_default()
        };
        if !early_jgrid_specs.is_empty() {
            profiler.mark("before_jgrid_early_lane");
            match try_jgrid_install(&early_jgrid_specs, options) {
                Ok(true) => {
                    profiler.mark("jgrid_fast_path");
                    let ld = crate::spec_grammar::materialize_special_deps(
                        Path::new("."),
                        Path::new("node_modules"),
                    );
                    finalize_special_deps(Path::new("package-lock.json"), &ld.specials);
                    write_install_markers();
                    return Ok(());
                }
                Ok(false) => {}
                Err(e) => {
                    if !options.quiet && crate::utils::is_verbose() {
                        crate::utils::log(&format!("JGrid early lane miss: {}", e));
                    }
                }
            }
        }
    }

    // Temporary diagnostic: JHOL_HTTP_METRICS=1 prints request counts on exit.
    struct HttpMetricsOnDrop;
    impl Drop for HttpMetricsOnDrop {
        fn drop(&mut self) {
            if std::env::var("JHOL_HTTP_METRICS").is_ok() {
                crate::http_client::print_http_metrics();
            }
        }
    }
    let _http_metrics_guard = HttpMetricsOnDrop;

    // Slower path: prune stale tree state for foreign platforms and audit packages
    if Path::new("node_modules").is_dir() {
        let purged = prune_platform_incompatible_node_modules(Path::new("node_modules"));
        if purged > 0 && !options.quiet {
            crate::utils::log(&format!(
                "pruned {} platform-incompatible entr{} from node_modules",
                purged,
                if purged == 1 { "y" } else { "ies" }
            ));
        }
    }
    if !packages.is_empty() && !options.quiet {
        let pkg_strings: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
        crate::security_guard::audit_packages_for_typosquatting(&pkg_strings);
    }

    // Computed once and consulted by every early-exit path: a lockfile that no longer matches
    // package.json must never be treated as "nothing to do" (npm repairs it on `install`; jhol's
    // warm bypass returned Ok(()) instead, so a project pinning `eslint: ^9` kept the lockfile's
    // eslint 10 and `jhol run lint` failed on a rule from another major).
    let lockfile_matches = lockfile_matches_manifest(std::path::Path::new("."));

    // Warm binary/bundled indexes in memory (no store_index.json rewrite — that
    // was ~150 ms of parse+write every cold process for data download lookups
    // already read via store_or_binary_hash). Then prewarm the HTTP agent.
    profiler.mark("install_pkg_start");
    if options.no_cache {
        crate::registry::clear_session_tarball_hashes();
        let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
        let _ = std::fs::remove_file(cache_dir.join("store_index.json"));
        let _ = std::fs::remove_file(cache_dir.join("store_index"));
    }
    crate::registry::preseed_store_index();
    // Do not eagerly prewarm HTTP/2 here: lockfile + harvest installs need no
    // network, and the handshake was ~100-150 ms of every cold start. The first
    // real request still warms the pool. (Tried moving this onto a background
    // thread so it overlapped local setup - measured no effect: `prewarm()` holds
    // the single pooled connection for its own HEAD, so the first download simply
    // waited on that instead. 634 ms vs 616 ms baseline, n=7.)
    profiler.mark("after_preseed");

    let download_conc = download_concurrency();
    let cache_install_conc = cache_install_concurrency().max(1);
    // Warm cache install concurrency defaults higher (was 1)
    let cache_install_conc =
        if cache_install_conc == 1 && std::env::var("JHOL_CACHE_INSTALL_CONCURRENCY").is_err() {
            std::thread::available_parallelism()
                .map(|n| n.get().clamp(4, 32))
                .unwrap_or(8)
        } else {
            cache_install_conc
        };
    let worker_pool_seq_threshold = worker_pool_sequential_threshold();
    let _legacy_chunk_scheduler = use_legacy_chunk_scheduler();

    let can_use_lockfile_layout = prefer_lockfile_materialization(options)
        || (packages.is_empty() && !options.no_cache && lockfile_matches);

    // Durable L0 / L1-store before lockfile layout: one clonefile of a graph-keyed
    // bundle beats walking every lockfile entry. Capsule already ran above; this
    // is the cross-project / cache-dir durable Instant Lock.
    #[cfg(feature = "pipeline")]
    if can_use_lockfile_layout && allow_prepared_replay {
        let jgrid_specs: Vec<String> = lockfile::read_all_resolved_specs_from_dir(Path::new("."))
            .unwrap_or_else(|| packages.iter().map(|p| (*p).to_string()).collect());
        if !jgrid_specs.is_empty() {
            profiler.mark("before_jgrid_early");
            match try_jgrid_install(&jgrid_specs, options) {
                Ok(true) => {
                    profiler.mark("jgrid_fast_path");
                    let ld = crate::spec_grammar::materialize_special_deps(
                        Path::new("."),
                        Path::new("node_modules"),
                    );
                    finalize_special_deps(Path::new("package-lock.json"), &ld.specials);
                    write_install_markers();
                    return Ok(());
                }
                Ok(false) => {}
                Err(e) => {
                    if !options.quiet {
                        crate::utils::log(&format!("JGrid early path failed: {}", e));
                    }
                }
            }
        }
    }

    if can_use_lockfile_layout && install_lockfile_layout(options)? {
        profiler.mark("layout_complete");
        // Local (`file:`/`link:`/`workspace:`) dependencies are skipped by the
        // lockfile installer (they are `link: true` entries, not downloads), so
        // materialize them here too.
        let ld_project_dir = Path::new(".");
        let ld = crate::spec_grammar::materialize_special_deps(
            ld_project_dir,
            Path::new("node_modules"),
        );
        finalize_special_deps(Path::new("package-lock.json"), &ld.specials);
        sync_hidden_lockfile();
        let _ = crate::capsule::write_capsule_from_project(Path::new("."));
        write_install_markers();
        profiler.mark("layout_capsule_markers");
        #[cfg(feature = "pipeline")]
        post_install_jgrid_ingest(None);
        profiler.mark("layout_l0_publish");
        return Ok(());
    }

    // Phase 5: DSSC-First Resolution — check spectral certs before full pipeline.
    // If all root packages are Stable-certified, skip Knot/SAT/topology entirely.
    // SAFETY: when packages is empty (install from package.json, e.g. `jhol add`
    // just wrote a new dep), the DSSC cert is from a previous install and may
    // be stale. Skip the fast path so the full pipeline re-resolves and picks
    // up the new package.
    if !options.lockfile_only && !packages.is_empty() {
        profiler.mark("after_capsule");
        match dssc_first_install(packages, options, &mut profiler) {
            Ok(true) => {
                remember_installed_packages(Path::new("node_modules"));
                write_install_markers();
                #[cfg(feature = "pipeline")]
                post_install_jgrid_ingest(None);
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                if !options.quiet {
                    crate::utils::log(&format!("DSSC first path failed: {}", e));
                }
            }
        }
    }

    if options.strict_peer_deps {
        let pj = Path::new("package.json");
        if pj.exists() {
            crate::lockfile_write::validate_root_peer_conflicts(pj).map_err(|e| {
                crate::error_handling::utils::config_error("validate_root_peer_conflicts", None, &e)
            })?;
        }
    }

    let mut optional_variants: Vec<OptionalVariant> = Vec::new();
    // Spec grammar: `package.json` may contain `npm:` aliases,
    // `file:`/`link:`/`workspace:` locals or git repos. Materialize locals now
    // and keep their specifiers out of the registry pipeline; aliases add the
    // real package to resolution and are linked to the alias name afterwards.
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let special_result =
        crate::spec_grammar::materialize_special_deps(&project_dir, Path::new("node_modules"));
    let special_deps = &special_result.specials;
    let special_names = special_name_set(special_deps);
    if std::env::var("JHOL_DEBUG_SPECIALS").is_ok() {
        eprintln!(
            "[special] in={packages:?} names={special_names:?} specs={:?} local_registry={:?}",
            special_deps
                .iter()
                .map(|s| (&s.name, &s.spec))
                .collect::<Vec<_>>(),
            special_result.local_registry_deps
        );
    }
    let mut registry_packages: Vec<String> = Vec::new();
    for p in packages {
        let handled = special_deps
            .iter()
            .any(|s| *p == s.name || p.starts_with(&format!("{}@", s.name)));
        if !handled {
            registry_packages.push((*p).to_string());
        }
    }
    // Dependencies declared by local/workspace packages, which the root-only
    // resolver would otherwise never install.
    for spec in &special_result.local_registry_deps {
        let name = base_name(spec);
        if !registry_packages
            .iter()
            .any(|p| p == spec || p.starts_with(&format!("{}@", name)))
        {
            registry_packages.push(spec.clone());
        }
    }
    for s in special_deps {
        if let Some(rspec) = s.registry_spec() {
            registry_packages.push(rspec);
        }
        // A git checkout's own registry dependencies are collected by the
        // spec-grammar pre-pass into `local_registry_deps` (above), which is the
        // single owner of "what a materialized dependency needs". This loop used
        // to walk `SpecialKind::Git { dir }` and add them a second time — two
        // owners of one rule, which is how a fix lands in one and not the other.
    }
    let registry_refs: Vec<&str> = registry_packages.iter().map(|s| s.as_str()).collect();
    let packages: &[&str] = &registry_refs;

    // Set by the flat expansion when its single-version-per-name result violates a
    // declared range; forces the node-keyed layout below.
    let mut expansion_conflict: Option<(String, String, String, String)> = None;
    let requested_packages: Vec<String> = if options.from_lockfile {
        // Deterministic CI mode: read resolved specs directly from lockfile
        match lockfile::read_all_resolved_specs_from_dir(Path::new(".")) {
            Some(specs) if !specs.is_empty() => {
                if !options.quiet && crate::utils::is_verbose() {
                    crate::utils::log(&format!("Lockfile layout ({} packages)", specs.len()));
                }
                specs
            }
            _ => packages.iter().map(|p| (*p).to_string()).collect(),
        }
    } else if let Some(specs) = can_reuse_lockfile_for_requested(packages) {
        if !options.quiet {
            crate::utils::log_file_only(&format!(
                "Reusing lockfile graph ({} packages)",
                specs.len()
            ));
        }
        specs
    } else {
        // When expansion yields nothing we fall back to the direct
        // spec list — but a name listed under `optionalDependencies` must not be
        // forced through: the resolver may have deliberately skipped it
        // (unavailable on the registry, or for this platform), and re-adding it
        // here made the install fail on a package it had already rejected.
        let fallback_specs = |pkgs: &[&str]| -> Vec<String> {
            let optional = optional_dependency_names();
            pkgs.iter()
                .filter(|p| !optional.contains(base_name(p)))
                .map(|p| (*p).to_string())
                .collect()
        };
        // A request *adds* to the project; it does not replace its dependencies. Without
        // the manifest's deps the expansion looks like it dropped every one of them, which
        // then triggered the nesting fallback — and that fallback resolves the manifest
        // alone, so `jhol install is-number@7.0.0` reported success while installing
        // everything except is-number. Union the two root sets instead.
        let mut roots_owned: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
        if !packages.is_empty() {
            if let Some(deps) = lockfile::read_package_json_deps(Path::new("package.json")) {
                // A request ADDS to a project; it does not re-resolve one. Only manifest deps
                // that are missing from node_modules take part, so `jhol install <pkg>` on a
                // large project resolves the delta instead of re-fetching the whole tree.
                // MUST carry the declared range: a bare name means "any version",
                // so the resolver picks the highest published one - a canary or a
                // major the manifest never asked for (measured: `next: "16.2.10"`
                // resolved to 16.4.0-canary.35, `react: "19.2.4"` to 19.3.0).
                let pending: Vec<String> = deps
                    .iter()
                    .filter(|(n, _)| {
                        !Path::new("node_modules")
                            .join(n)
                            .join("package.json")
                            .exists()
                    })
                    .map(|(n, r)| {
                        if r.is_empty() {
                            n.clone()
                        } else {
                            format!("{}@{}", n, r)
                        }
                    })
                    .collect();
                roots_owned =
                    union_registry_roots(&roots_owned, pending.into_iter(), &special_names);
            }
        }
        let roots: Vec<&str> = roots_owned.iter().map(|s| s.as_str()).collect();
        let network_before = profiler.enabled.then(crate::http_client::get_http_metrics);
        let (fresh, fresh_variants, fresh_conflict) = match expand_with_transitive_dependencies(
            &roots,
            options.offline,
            options.overrides.as_ref(),
        ) {
            Ok((specs, vars, conflict)) if !specs.is_empty() => (specs, vars, conflict),
            Ok(_) => (fallback_specs(packages), Vec::new(), None),
            Err(err) => {
                if (options.offline || registry::offline_mode())
                    && err.contains("Offline mode: package(s) not in cache")
                {
                    return Err(crate::error_handling::utils::cache_error(
                        "offline_install",
                        None,
                        &err,
                    ));
                }
                return Err(crate::error_handling::utils::application_error(
                    "expand_dependencies",
                    Some("resolution_failed"),
                    &err,
                ));
            }
        };
        optional_variants = fresh_variants;
        expansion_conflict = fresh_conflict;
        if let Some(before) = network_before {
            let after = crate::http_client::get_http_metrics();
            eprintln!(
                "[jhol-profile] stage=expansion_http requests={} successful={} failed={} body_bytes={} specs={}",
                after.0.saturating_sub(before.0),
                after.1.saturating_sub(before.1),
                after.2.saturating_sub(before.2),
                after.3.saturating_sub(before.3),
                fresh.len(),
            );
        }
        profiler.mark("after_expansion");
        // Final gate before anything is fetched or linked.
        drop_platform_incompatible(fresh)
    };

    // Flat expansion can only represent one version per package. When conflicting
    // ranges make it drop a direct dependency (e.g. `cliui ^8` needs
    // `string-width ^4` while the root wants `^5`), fall back to the node-keyed
    // resolver layout: resolve, write a nested lockfile, and install from it.
    // This is the only path where a *plain* `jhol install` nests versions.
    // The nested resolver exists to repair *conflicts* in small graphs, and its analysis is
    // super-linear in the dependency count: measured on a real 421-dependency project it did not
    // finish within ten minutes, while the flat path installs the same tree in ~12 s. Bound it by
    // manifest size so an install always terminates; `JHOL_JAGR_MAX_DEPS` raises or removes the
    // bound for anyone who wants nested placement on a large graph.
    let max_nested_deps: usize = std::env::var("JHOL_JAGR_MAX_DEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(80);
    let manifest_dep_count = lockfile::read_package_json_deps(Path::new("package.json"))
        .map(|d| d.len())
        .unwrap_or(0);
    let nested_budget_ok = max_nested_deps == 0 || manifest_dep_count <= max_nested_deps;
    if !nested_budget_ok && expansion_conflict.is_some() {
        return Err(crate::error_handling::utils::application_error(
            "resolve_nested_layout",
            Some("resolution_budget"),
            "Required dependency conflict exceeds nested-analysis budget; raise JHOL_JAGR_MAX_DEPS",
        ));
    }
    if !nested_budget_ok && !options.quiet {
        crate::utils::log(&format!(
            "note: using the flat resolver — {} direct dependencies exceeds the nested-analysis \
             budget of {} (set JHOL_JAGR_MAX_DEPS to override)",
            manifest_dep_count, max_nested_deps
        ));
    }
    if nested_budget_ok && !options.from_lockfile && !options.lockfile_only && !packages.is_empty()
    {
        let pj = Path::new("package.json");
        if let Some(deps) = lockfile::read_package_json_deps(pj) {
            let optional = optional_dependency_names();
            let present: HashSet<String> = requested_packages
                .iter()
                .map(|s| base_name(s).to_string())
                .collect();
            let dropped: Vec<String> = deps
                .keys()
                // A special is never in the registry expansion, so it always looked
                // "dropped" and forced this nested fallback — which then handed the
                // raw `github:` spec to the resolver as if it were a version range.
                .filter(|n| {
                    !present.contains(n.as_str())
                        && !special_names.contains(n.as_str())
                        && !optional.contains(n.as_str())
                })
                .cloned()
                .collect();
            if !dropped.is_empty() || expansion_conflict.is_some() {
                if !options.quiet {
                    match &expansion_conflict {
                        Some((parent, dep, range, version)) => crate::utils::log(&format!(
                            "flat resolution conflict ({} requires {} {}, resolved {}); resolving nested layout",
                            parent, dep, range, version
                        )),
                        None => crate::utils::log(&format!(
                            "version conflict in flat expansion ({}); resolving nested layout",
                            dropped.join(", ")
                        )),
                    }
                }
                // Fast path: a *pure* version conflict needs only re-nesting, and
                // everything it needs is in memory (see
                // `nested_layout_from_flat_state`). Only when the flat expansion
                // actually dropped a package — so the metadata cannot describe the
                // tree — do we pay for a second full resolution.
                //
                // DEFAULT ON, and it is the difference between finishing and not on
                // a real app: a fresh-cache install of a 12-dependency Next.js
                // manifest with the second resolution took 273-445 s and did not
                // finish (1.3-4 GB resident), and now completes in ~21 s at 557 MB
                // with zero atlas files written (i.e. the second pass did not run
                // at all). Verified on that manifest: react 19.2.4, next 16.2.10,
                // typescript 5.9.3, eslint 9.39.5, 0 foreign-platform packages, and
                // the installed tree loads under node. `JHOL_SINGLE_PASS=0` restores
                // the old always-re-resolve behaviour.
                let single_pass_enabled = !std::env::var("JHOL_SINGLE_PASS")
                    .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
                    .unwrap_or(false);
                let flat_selected: HashMap<String, String> = requested_packages
                    .iter()
                    .filter_map(|s| {
                        s.rsplit_once('@')
                            .map(|(_, v)| (base_name(s).to_string(), v.to_string()))
                    })
                    .collect();
                let fast_layout = if single_pass_enabled && dropped.is_empty() {
                    lockfile::read_package_json_deps(pj)
                        .and_then(|d| nested_layout_from_flat_state(&d, &flat_selected))
                } else {
                    None
                };
                // Diagnostics for the opt-in path: say *why* it did not run, so a
                // rejection is never a mystery (and so the reason is visible when
                // deciding whether it can become the default).
                if single_pass_enabled && !options.quiet {
                    if !dropped.is_empty() {
                        crate::utils::log(&format!(
                            "single-pass: not attempted — flat expansion dropped {} direct \
                             dependency(ies): {}",
                            dropped.len(),
                            dropped.join(", ")
                        ));
                    } else if fast_layout.is_some() {
                        crate::utils::log(
                            "single-pass: nested layout built from in-memory metadata \
                             (no second resolution)",
                        );
                    } else {
                        crate::utils::log(
                            "single-pass: rejected by the completeness guard; \
                             using the full resolver",
                        );
                    }
                }
                // Primary: per-dependent instances (`jagr_nest`), which can hold
                // several majors of one package — the case a flat domain cannot
                // represent and the reason real trees used to fail here.
                // Fallback: the previous flat-then-nest resolver, so a graph the
                // instance resolver rejects still installs.
                // OPT-IN until the plan and materialisation agree: the instance
                // resolver installs jest end to end (266 packages, nested majors
                // and all), but the resulting tree is not yet stable — a warm run
                // reports a package the lockfile lists and the tree lacks
                // (`function-bind`), i.e. one instance's planned path is not the
                // path that gets materialised. A resolver whose output the
                // installer only partly honours must not be the default.
                let nested_first = matches!(
                    std::env::var("JHOL_NEST_RESOLVER").as_deref(),
                    Ok("1") | Ok("true")
                );
                let resolved_tree: Result<
                    HashMap<String, crate::lockfile_write::ResolvedPackage>,
                    String,
                > = match fast_layout {
                    Some(layout) => Ok(layout.into_iter().collect()),
                    None => {
                        let mut attempt = if nested_first {
                            crate::lockfile_write::resolve_full_tree_nested(pj)
                        } else {
                            Err("disabled".to_string())
                        };
                        if attempt.is_err() {
                            if nested_first && !options.quiet {
                                crate::utils::log(&format!(
                                    "nested resolver: {}; falling back to the flat resolver",
                                    attempt.as_ref().err().map(|e| e.as_str()).unwrap_or("")
                                ));
                            }
                            attempt = crate::lockfile_write::resolve_full_tree(pj);
                        }
                        attempt
                    }
                };
                match resolved_tree {
                    Ok(mut tree) => {
                        // This path resolves the *manifest*, so a package the user asked for
                        // but never declared was silently absent — `jhol install is-number`
                        // reported success having installed everything except is-number. Add
                        // the requested roots (and their transitive tree) to the resolution.
                        if !packages.is_empty() {
                            let store_index = utils::read_store_index();
                            let cache_store =
                                crate::content::store_dir(&PathBuf::from(utils::get_cache_dir()));
                            for spec in packages {
                                let name = base_name(spec).to_string();
                                if tree.contains_key(&format!("node_modules/{}", name)) {
                                    continue;
                                }
                                let expanded = expand_with_transitive_dependencies(
                                    &[*spec],
                                    options.offline,
                                    options.overrides.as_ref(),
                                )
                                .map(|(specs, _, _)| specs)
                                .map_err(|e| {
                                    crate::error_handling::utils::application_error(
                                        "expand_requested_dependencies",
                                        Some("resolution_failed"),
                                        &e,
                                    )
                                })?;
                                for s in expanded {
                                    let Some((n, v)) = s.rsplit_once('@') else {
                                        continue;
                                    };
                                    if n.is_empty() || v.is_empty() {
                                        continue;
                                    }
                                    let key = format!("node_modules/{}", n);
                                    // First writer wins: the manifest's resolution is
                                    // authoritative for anything it already covers.
                                    tree.entry(key).or_insert_with(|| {
                                        let integrity = store_index
                                            .get(&format!("{}@{}", n, v))
                                            .and_then(|hash| {
                                                cached_tarball_sri(
                                                    &cache_store.join(format!("{}.tgz", hash)),
                                                )
                                            });
                                        crate::lockfile_write::ResolvedPackage {
                                            version: v.to_string(),
                                            resolved: crate::lockfile::tarball_url_from_registry(
                                                n, v,
                                            ),
                                            integrity,
                                            dependencies: std::collections::HashMap::new(),
                                            peer_dependencies: std::collections::HashMap::new(),
                                            peer_dependencies_meta: std::collections::HashMap::new(
                                            ),
                                        }
                                    });
                                }
                            }
                        }
                        let lock_path = pj.with_file_name("package-lock.json");
                        crate::lockfile_write::write_package_lock(&lock_path, pj, &tree).map_err(
                            |e| {
                                crate::error_handling::utils::config_error(
                                    "write_package_lock",
                                    None,
                                    &e,
                                )
                            },
                        )?;
                        // Provenance: append one chained entry for this install (IPL).
                        crate::origin::log_install(&lock_path);
                        let mut lock_opts = options.clone();
                        lock_opts.from_lockfile = true;
                        finalize_special_deps(&lock_path, special_deps);
                        if install_lockfile_layout(&lock_opts)? {
                            sync_hidden_lockfile();
                            let _ = crate::capsule::write_capsule_from_project(Path::new("."));
                            remember_installed_packages(Path::new("node_modules"));
                            write_install_markers();
                            #[cfg(feature = "pipeline")]
                            post_install_jgrid_ingest(None);
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        return Err(crate::error_handling::utils::application_error(
                            "resolve_nested_layout",
                            Some("resolution_failed"),
                            &e,
                        ))
                    }
                }
            }
        }
    }

    crate::progress::reset();
    crate::progress::set_total(requested_packages.len().max(1) as u64);

    // JGrid Pipeline: content-addressed, clone-ready materialization.
    // Tries L0 → L1 → L2 fallback with mmap'd zero-copy reads.
    #[cfg(feature = "pipeline")]
    if allow_prepared_replay {
        profiler.mark("before_jgrid");
        match try_jgrid_install(&requested_packages, options) {
            Ok(true) => {
                profiler.mark("jgrid_fast_path");
                let lock_path = Path::new("package-lock.json");
                let pj_path = Path::new("package.json");
                if pj_path.exists() && !lock_path.exists() {
                    if let Ok(packages) = read_all_installed_packages(Path::new("node_modules")) {
                        let _ = crate::lockfile_write::write_package_lock(
                            lock_path, pj_path, &packages,
                        );
                    }
                }
                sync_hidden_lockfile();
                write_install_markers();
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                if !options.quiet {
                    crate::utils::log(&format!("JGrid pipeline failed: {}", e));
                }
            }
        }
    }

    let mut seen_packages = HashSet::new();
    // One gate for every route into the install list: nothing that cannot run on this machine is
    // fetched or linked (the resolver skips such packages, but the flat fallback could re-add one).
    let requested_packages: Vec<String> = drop_platform_incompatible(requested_packages);

    let mut to_install_from_cache = Vec::new();
    let mut to_fetch = Vec::new();
    let mut missing_for_offline = Vec::new();
    let cache_dir_for_lookup = std::path::PathBuf::from(utils::get_cache_dir());
    let store_index = utils::read_store_index();
    profiler.mark("after_store_index");

    // Topological wave optimization: use capsule to skip entire waves
    // of already-installed packages. Only process waves with missing packages.
    let wave_skip = if !options.no_cache {
        let cap_path = crate::capsule::capsule_path(Path::new("."));
        if cap_path.exists() {
            if let Ok(view) = crate::capsule::read_capsule(&cap_path) {
                if view.package_count() > 0 {
                    // Group specs by install_order
                    let mut wave_specs: std::collections::BTreeMap<u16, Vec<String>> =
                        std::collections::BTreeMap::new();
                    for i in 0..view.package_count() {
                        let e = view.entry(i);
                        let spec = format!("{}@{}", view.entry_name(e), view.entry_version(e));
                        wave_specs.entry(e.install_order).or_default().push(spec);
                    }
                    // Check which waves are fully installed
                    let mut skip_packages: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    for specs in wave_specs.values() {
                        let all_installed = specs.iter().all(|s| {
                            let name = base_name(s);
                            let pkg_dir =
                                Path::new("node_modules").join(if name.starts_with('@') {
                                    let parts: Vec<&str> = name.split('/').collect();
                                    let scope = parts[0];
                                    let rest: Vec<&str> = parts[1..].to_vec();
                                    format!("{}/{}", scope, rest.join("/"))
                                } else {
                                    name.to_string()
                                });
                            pkg_dir.join("package.json").exists()
                        });
                        if all_installed {
                            for spec in specs {
                                skip_packages.insert(spec.clone());
                            }
                        } else {
                            // Wave partially installed — process all packages in it
                            // (don't skip any in this wave)
                        }
                    }
                    skip_packages
                } else {
                    std::collections::HashSet::new()
                }
            } else {
                std::collections::HashSet::new()
            }
        } else {
            std::collections::HashSet::new()
        }
    } else {
        std::collections::HashSet::new()
    };
    profiler.mark("after_wave_skip");

    // Fast batch path: when all packages are exact-version specs (already resolved
    // by expansion cache) and the store index is populated, resolve them in a tight
    // HashMap::get + stat check without per-package normalization/fallback overhead.
    // If all are present, skip the full sequential classification loop entirely.
    let batch_ok = !store_index.is_empty()
        && requested_packages.iter().all(|p| {
            p.rfind('@').is_some_and(|idx| {
                idx > 0 && p[idx + 1..].starts_with(|c: char| c.is_ascii_digit())
            })
        });
    if batch_ok {
        let _store_dir = crate::content::store_dir(&cache_dir_for_lookup);
        let mut all_present = true;
        let mut batch = Vec::with_capacity(requested_packages.len());
        for spec in &requested_packages {
            if let Some(hash) = store_index.get(spec) {
                let store_file = crate::content::tarball_path(&cache_dir_for_lookup, hash);
                if store_file.exists() {
                    batch.push((spec.to_string(), store_file));
                    seen_packages.insert(base_name(spec).to_string());
                    continue;
                }
            }
            all_present = false;
            break;
        }
        if all_present {
            to_install_from_cache = batch;
            profiler.mark("classify_cache_vs_fetch");
        } else {
            // The batch fast path needs every requested package already in the
            // store index. A miss is not a failure — the sequential loop below
            // fetches what is missing — so name it for what it is. (The bundled
            // preseed always makes the index non-empty, so a cold install of a
            // non-bundled package lands here by design.)
            seen_packages.clear();
            profiler.mark("batch_fast_path_miss");
        }
    }

    if to_install_from_cache.is_empty() {
        for package in &requested_packages {
            let base = base_name(package);

            // Topological wave skip: if this entire wave is already installed, skip
            if wave_skip.contains(package) || wave_skip.contains(base) {
                if !options.quiet {
                    utils::log(&format!("Skipping {} (wave already installed)", package));
                }
                continue;
            }

            // Soul Path predictive prefetch (once per batch, not O(n²) disk reads)
            if !options.offline && !options.no_cache && seen_packages.len() == 2 {
                let history: Vec<String> = seen_packages.iter().cloned().collect();
                crate::soul_path::predictive_prefetch(&history, 3);
            }

            if seen_packages.contains(base) && !options.quiet {
                crate::utils::log(&format!(
                    "Warning: Multiple versions of {} requested.",
                    base
                ));
            }
            seen_packages.insert(base.to_string());
            if !options.quiet && crate::utils::is_verbose() {
                utils::log(&format!("Installing package: {}", package));
            }

            let start_fast = std::time::Instant::now();
            if let Some(tarball) =
                get_cached_tarball_fast(package, &cache_dir_for_lookup, &store_index)
            {
                if std::env::var("JHOL_DEBUG_JINF").is_ok() {
                    crate::utils::log(&format!(
                        "[JINF] cache_fast {} {:?}",
                        package,
                        start_fast.elapsed()
                    ));
                }
                if !options.quiet && crate::utils::is_verbose() {
                    crate::utils::log(&format!("Installing {} from cache...", package));
                }
                to_install_from_cache.push((package.to_string(), tarball));
                continue;
            }
            if options.offline {
                missing_for_offline.push(package.to_string());
                continue;
            }
            to_fetch.push(package.to_string());
        }

        // Offline mode must be fully cache-backed; never schedule network fetches.
        if options.offline && !missing_for_offline.is_empty() {
            return Err(crate::error_handling::utils::cache_error(
            "offline_install",
            None,
            &format!(
                "Offline mode: package(s) not in the content store: {}. Fetch them once with:\n  jhol prefetch\n(keep package-lock.json committed for reproducible offline installs)",
                missing_for_offline.join(", ")
            ),
        ));
        }

        if !options.offline && !missing_for_offline.is_empty() {
            to_fetch.extend(missing_for_offline.iter().cloned());
        }
        profiler.mark("classify_cache_vs_fetch");
    }

    // Skip npm show when we trust the lockfile or frozen (zero packument).
    // Also skip validation entirely for packages already present in
    // `node_modules` or with an extracted store dir. The store is the source
    // of truth, and a tarball we already have is by definition valid (its
    // integrity was verified on first download). Validating known-good
    // packages against the npm registry is pure overhead AND a failure
    // point when the registry is rate-limited or returns 404 for versioned
    // lookups (`npm show` doesn't accept pinned versions well).
    if !to_fetch.is_empty()
        && !options.from_lockfile
        && !options.strict_lockfile
        && !options.native_only
    {
        let to_validate: Vec<String> = to_fetch
            .iter()
            .filter(|pkg| !package_already_resolved(pkg, &cache_dir_for_lookup))
            .cloned()
            .collect();
        if !to_validate.is_empty() {
            let results = registry::parallel_validate_packages(&to_validate, NPM_SHOW_TIMEOUT_SECS);
            let invalid: Vec<String> = results
                .iter()
                .filter(|(_, ok)| !*ok)
                .map(|(p, _)| p.clone())
                .collect();
            if !invalid.is_empty() {
                return Err(crate::error_handling::utils::registry_error_with_package(
                    "validate_packages",
                    &invalid.join(", "),
                    None,
                    "Package(s) not found or invalid",
                ));
            }
        }
    }

    // Parallel execution: overlap cache materialization with network fetch/resolution
    let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
    let node_modules = Path::new("node_modules");
    std::fs::create_dir_all(node_modules).map_err(|e| {
        crate::error_handling::utils::io_error("create_node_modules", Some("node_modules"), e)
    })?;
    std::fs::create_dir_all(node_modules.join(".bin")).map_err(|e| {
        crate::error_handling::utils::io_error("create_bin_dir", Some("node_modules/.bin"), e)
    })?;
    let purged = prune_platform_incompatible_node_modules(node_modules);
    if purged > 0 && !options.quiet {
        utils::log(&format!(
            "pruned {} platform-incompatible entr{} from node_modules",
            purged,
            if purged == 1 { "y" } else { "ies" }
        ));
    }

    let profile_cache_install = install_profile_enabled();
    let ((fallback_tarballs, cache_profile_totals), (npm_fallback, index_batch)) = rayon::join(
        || {
            if to_install_from_cache.is_empty() {
                return (Vec::new(), [0u128; 5]);
            }
            let mut fallback_tarballs = Vec::new();
            let cache_install_inputs: Vec<(String, std::path::PathBuf)> = to_install_from_cache
                .iter()
                .map(|(pkg, tarball_path)| (pkg.clone(), tarball_path.clone()))
                .collect();
            let install_results: Vec<(String, std::path::PathBuf, bool, [u128; 5])> = {
                let cache_dir = cache_dir.clone();
                let node_modules = node_modules.to_path_buf();
                run_worker_pool(
                    cache_install_inputs,
                    cache_install_conc,
                    worker_pool_seq_threshold,
                    move |(pkg, tarball_path)| {
                        let base = base_name(&pkg).to_string();
                        let mut timing_ns = [0u128; 5];
                        let started = profile_cache_install.then(Instant::now);
                        let bundled = try_install_bundled_from_cache(&pkg, &base, &node_modules);
                        if let Some(started) = started {
                            timing_ns[0] = started.elapsed().as_nanos();
                        }
                        let ok = if bundled {
                            true
                        } else {
                            let started = profile_cache_install.then(Instant::now);
                            let unpacked =
                                registry::ensure_unpacked_in_store(&tarball_path, &cache_dir);
                            if let Some(started) = started {
                                timing_ns[1] = started.elapsed().as_nanos();
                            }
                            match unpacked {
                                Ok(unpacked) => {
                                    let started = profile_cache_install.then(Instant::now);
                                    let linked = utils::link_package_from_store(
                                        &unpacked,
                                        &node_modules,
                                        &base,
                                    )
                                    .is_ok();
                                    if let Some(started) = started {
                                        timing_ns[2] = started.elapsed().as_nanos();
                                    }
                                    if linked {
                                        true
                                    } else {
                                        let started = profile_cache_install.then(Instant::now);
                                        let extracted = selective_extract::extract_selective(
                                            &tarball_path,
                                            &node_modules,
                                            &base,
                                        )
                                        .is_ok();
                                        if let Some(started) = started {
                                            timing_ns[3] = started.elapsed().as_nanos();
                                        }
                                        extracted
                                    }
                                }
                                Err(_) => false,
                            }
                        };
                        let (ok, bins_ok) = if ok {
                            let started = profile_cache_install.then(Instant::now);
                            let bins_ok =
                                bin_links::link_bins_for_package(&node_modules, &base).is_ok();
                            if let Some(started) = started {
                                timing_ns[4] = started.elapsed().as_nanos();
                            }
                            (true, bins_ok)
                        } else {
                            (false, false)
                        };
                        (pkg, tarball_path, ok && bins_ok, timing_ns)
                    },
                )
            };

            let mut totals_ns = [0u128; 5];
            for (pkg, tarball_path, ok, timing_ns) in install_results {
                for (total, sample) in totals_ns.iter_mut().zip(&timing_ns) {
                    *total += sample;
                }
                if ok {
                    crate::progress::mark_from_cache();
                    crate::progress::mark_linked();
                    if !options.quiet {
                        utils::log_file_only(&format!("Installed {} from cache (link/copy).", pkg));
                    }
                } else {
                    fallback_tarballs.push((pkg, tarball_path));
                }
            }
            (fallback_tarballs, totals_ns)
        },
        || {
            if to_fetch.is_empty() {
                return (Vec::new(), std::collections::HashMap::new());
            }
            let mut npm_fallback = Vec::new();
            let mut index_batch: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            if options.from_lockfile {
                let (resolved_urls, resolved_integrity) =
                    match lockfile::read_resolved_urls_and_integrity_from_dir(Path::new(".")) {
                        Some((u, i)) => (u, i),
                        None => (
                            std::collections::HashMap::new(),
                            std::collections::HashMap::new(),
                        ),
                    };
                let mut work: Vec<(String, String, Option<String>)> = Vec::new();
                for pkg in &to_fetch {
                    let url = resolved_urls.get(pkg).cloned().or_else(|| {
                        let base = base_name(pkg);
                        let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("latest");
                        Some(lockfile::tarball_url_from_registry(base, version))
                    });
                    match url {
                        Some(u) => {
                            let integrity = resolved_integrity.get(pkg).cloned();
                            work.push((pkg.clone(), u, integrity));
                        }
                        None => npm_fallback.push(pkg.clone()),
                    }
                }
                let dl_inputs: Vec<(String, String, Option<String>)> = work;
                let dl_concurrency = download_conc;
                let cache_dir_dl = cache_dir.clone();
                let node_modules_dl = node_modules.to_path_buf();
                let download_results: Vec<(String, Result<String, String>)> = run_worker_pool(
                    dl_inputs,
                    dl_concurrency,
                    worker_pool_seq_threshold,
                    move |(pkg, url, integrity)| {
                        let pkg_key = exact_version_from_spec(&pkg)
                            .map(|v| format!("{}@{}", base_name(&pkg), v))
                            .unwrap_or_else(|| pkg.clone());
                        let res = registry::download_tarball_to_store_hash_only(
                            &url,
                            &cache_dir_dl,
                            &pkg_key,
                            integrity.as_deref(),
                        )
                        .and_then(|hash| {
                            let unpacked = crate::content::unpacked_dir(&cache_dir_dl, &hash);
                            let store_path = crate::content::tarball_path(&cache_dir_dl, &hash);
                            materialize_downloaded_package(
                                &store_path,
                                &unpacked,
                                &node_modules_dl,
                                &pkg,
                            )
                            .map_err(|e| format!("package materialization failed: {e}"))?;
                            Ok(hash)
                        });
                        (pkg, res)
                    },
                );
                for (pkg, res) in download_results {
                    match res {
                        Ok(hash) => {
                            index_batch.insert(pkg.clone(), hash.clone());
                            let base = base_name(&pkg);
                            crate::progress::mark_from_network();
                            crate::progress::mark_from_cache();
                            crate::progress::mark_linked();
                            if !options.quiet {
                                let version = pkg.rfind('@').map(|i| &pkg[i + 1..]).unwrap_or("");
                                crate::utils::log_file_only(&format!(
                                    "Installed {}@{} (native)",
                                    base, version
                                ));
                            }
                        }
                        Err(e) => {
                            crate::utils::log(&format!(
                                "Download or unpack failed for {}: {}",
                                pkg, e
                            ));
                            crate::utils::record_fallback_reason(
                                "download_or_unpack_failed",
                                std::slice::from_ref(&pkg),
                            );
                            npm_fallback.push(pkg);
                        }
                    }
                }
            } else {
                let work = ManifestThenPackumentStrategy.resolve(&to_fetch, &mut npm_fallback);

                let dl_inputs: Vec<ResolvedFetch> = work;
                let dl_concurrency = download_conc;
                let node_modules_dl = node_modules.to_path_buf();
                let cache_dir_dl = cache_dir.clone();
                let download_results: Vec<(String, Result<(String, String), String>)> =
                    run_worker_pool(
                        dl_inputs,
                        dl_concurrency,
                        worker_pool_seq_threshold,
                        move |item| {
                            let pkg = item.pkg;
                            let url = item.url;
                            let integrity = item.integrity;
                            let version = item.version;
                            let pkg_key = format!("{}@{}", base_name(&pkg), version);
                            let res = registry::download_tarball_to_store_hash_only(
                                &url,
                                &cache_dir_dl,
                                &pkg_key,
                                integrity.as_deref(),
                            )
                            .and_then(|hash| {
                                let unpacked = crate::content::unpacked_dir(&cache_dir_dl, &hash);
                                let store_path = crate::content::tarball_path(&cache_dir_dl, &hash);
                                materialize_downloaded_package(
                                    &store_path,
                                    &unpacked,
                                    &node_modules_dl,
                                    &pkg,
                                )
                                .map_err(|e| format!("package materialization failed: {e}"))?;
                                Ok((hash, version))
                            });
                            (pkg, res)
                        },
                    );

                for (pkg, res) in download_results {
                    match res {
                        Ok((hash, version)) => {
                            let index_key = format!("{}@{}", base_name(&pkg), version);
                            index_batch.insert(index_key, hash.clone());
                            let base = base_name(&pkg);
                            crate::progress::mark_from_network();
                            crate::progress::mark_from_cache();
                            crate::progress::mark_linked();
                            if !options.quiet {
                                crate::utils::log(&format!(
                                    "Installed {}@{} (native)",
                                    base, version
                                ));
                            }
                        }
                        Err(e) => {
                            crate::utils::log(&format!(
                                "Download or unpack failed for {}: {}",
                                pkg, e
                            ));
                            crate::utils::record_fallback_reason(
                                "download_or_unpack_failed",
                                std::slice::from_ref(&pkg),
                            );
                            npm_fallback.push(pkg);
                        }
                    }
                }
            }

            (npm_fallback, index_batch)
        },
    );

    profiler.mark("materialize_and_fetch_parallel");

    if profile_cache_install && !to_install_from_cache.is_empty() {
        crate::utils::log(&format!(
            "[jhol-profile] cache_materialize packages={} sum_ms bundled_decode={:.3} store_prepare={:.3} package_link={:.3} fallback_extract={:.3} bin_link={:.3} (parallel sums; not wall time)",
            to_install_from_cache.len(),
            cache_profile_totals[0] as f64 / 1_000_000.0,
            cache_profile_totals[1] as f64 / 1_000_000.0,
            cache_profile_totals[2] as f64 / 1_000_000.0,
            cache_profile_totals[3] as f64 / 1_000_000.0,
            cache_profile_totals[4] as f64 / 1_000_000.0,
        ));
    }

    if !index_batch.is_empty() {
        let mut index = utils::read_store_index();
        index.extend(index_batch);
        if let Err(e) = utils::write_store_index(&index) {
            crate::utils::log(&format!(
                "warning: store index update failed ({}); continuing — index will be lazily repaired",
                e
            ));
        }
    }

    if !fallback_tarballs.is_empty() {
        let fallback_pkgs: Vec<String> = fallback_tarballs.iter().map(|(p, _)| p.clone()).collect();
        crate::utils::record_fallback_reason("cache_link_or_extract_failed", &fallback_pkgs);
        if options.native_only {
            return Err(crate::error_handling::utils::cache_error(
                "native_only_install",
                None,
                &format!("Native-only: could not link or extract from cache for: {}. Try JHOL_LINK=0 or run without --native-only.", fallback_pkgs.join(", "))
            ));
        }
        if !options.no_scripts {
            if let Some(allowlist) = &options.script_allowlist {
                check_script_allowlist(&fallback_pkgs, allowlist)?;
            }
        }
        let paths: Vec<std::path::PathBuf> =
            fallback_tarballs.iter().map(|(_, p)| p.clone()).collect();
        match backend::backend_install_tarballs(&paths, options.backend, options.no_scripts) {
            Ok(()) => {
                for (pkg, _) in &fallback_tarballs {
                    if !options.quiet {
                        utils::log_file_only(&format!("Installed {} from cache (backend).", pkg));
                    }
                }
            }
            Err(e) => {
                return Err(crate::error_handling::utils::application_error(
                    "backend_install_tarballs",
                    Some("install_failed"),
                    &e,
                ));
            }
        }
    }

    // Zero-network installs (local harvest / full cache) land on store dirs that
    // already carry in-store dep links from earlier runs on this machine.
    // Relinking is the measured ~30-50ms harvest-cold tail — skip when no bytes
    // left the wire. Any real download still relinks.
    let (http_reqs, _, _, http_bytes, _) = crate::http_client::get_http_metrics();
    if http_reqs == 0 && http_bytes == 0 {
        profiler.mark("tail_relink_skipped_offline_hits");
    } else if expansion_conflict.is_none() {
        profiler.mark("tail_relink_skipped_no_conflicts");
    } else {
        crate::utils::relink_store_dependencies(Path::new("node_modules"));
        profiler.mark("tail_relink");
    }

    if npm_fallback.is_empty() {
        let lock_path = Path::new("package-lock.json");
        let pj_path = Path::new("package.json");
        // Always regenerate the lockfile from the current node_modules state.
        // The lockfile is a derived view of the content-addressed store; if
        // node_modules changed (e.g. `jhol add` just installed a new dep),
        // the lockfile must be rewritten to stay in sync. The previous guard
        // `!lock_path.exists()` skipped the write when a lockfile was already
        // present, which left `jhol add` and any incremental install with a
        // stale lockfile that `jhol ci` would reject.
        let installed_packages = if pj_path.exists() {
            profiler.mark("before_read_installed");
            let pkgs = read_all_installed_packages(node_modules).ok();
            profiler.mark("after_read_installed");
            if let Some(ref packages) = pkgs {
                let _ = crate::lockfile_write::write_package_lock(lock_path, pj_path, packages);
                profiler.mark("after_write_lock");
                // Provenance: append one chained entry for this install (IPL).
                crate::origin::log_install(lock_path);
                finalize_special_deps(lock_path, special_deps);
                append_optional_variants_to_lockfile(lock_path, &optional_variants);
                // Non-critical post-install work in background thread
                let pj_buf = pj_path.to_path_buf();
                let packages_clone = packages.clone();
                std::thread::spawn(move || {
                    let fp = crate::resolution_mirror::fingerprint_deps(
                        &lockfile::read_package_json_deps(&pj_buf).unwrap_or_default(),
                    );
                    crate::resolution_mirror::record_resolution(fp, &packages_clone);
                    crate::soul_path::remember_all_packages(&packages_clone);
                    let names: Vec<String> = packages_clone.keys().cloned().collect();
                    if names.len() >= 3 {
                        crate::second_order_markov::learn_from_install(&names);
                    }
                });
            }
            pkgs
        } else {
            // Always remember installed packages for GRM/lattice even when lockfile exists
            remember_installed_packages(node_modules);
            None
        };
        sync_hidden_lockfile();
        record_install_log(&requested_packages, profiler.total_ms());
        write_install_markers();
        #[cfg(feature = "pipeline")]
        {
            profiler.mark("before_post_jgrid");
            post_install_jgrid_ingest(installed_packages.as_ref());
            profiler.mark("after_post_jgrid");
        }
        return Ok(());
    }

    if options.native_only {
        crate::utils::record_fallback_reason("native_install_failed", &npm_fallback);
        let hint = if std::env::var("JHOL_FETCH_TIMEOUT_SECS").is_ok() {
            " (check network connectivity or increase --fetch-timeout / JHOL_FETCH_TIMEOUT_SECS)"
        } else {
            ""
        };
        return Err(crate::error_handling::utils::application_error(
            "native_only_install",
            Some("install_failed"),
            &format!("Native-only: install failed for: {}{}. Run without --native-only to use Bun/npm fallback.", npm_fallback.join(", "), hint)
        ));
    }

    crate::utils::record_fallback_reason("native_install_failed", &npm_fallback);

    // Save dependency trees to offline cache for future offline installs
    if !options.offline {
        let mut offline_cache = crate::offline_cache::OfflineCache::new(std::path::PathBuf::from(
            utils::get_cache_dir(),
        ));

        // Read installed packages and cache their dependency trees
        if let Ok(packages) = read_all_installed_packages(node_modules) {
            let trees = crate::offline_cache::build_dependency_tree(&packages);
            for tree in trees {
                let _ = offline_cache.save_tree(tree);
            }
        }
    }

    if !options.no_scripts {
        if let Some(allowlist) = &options.script_allowlist {
            check_script_allowlist(&npm_fallback, allowlist)?;
        }
    }

    // Fallback: backend install for any that native failed
    let fetch_refs: Vec<&str> = npm_fallback.iter().map(|s| s.as_str()).collect();
    let mut attempts = 3;
    loop {
        match backend::backend_install(
            &fetch_refs,
            options.backend,
            options.lockfile_only,
            options.no_scripts,
        ) {
            Ok(()) => {
                let cache_dir = std::path::PathBuf::from(utils::get_cache_dir());
                for pkg in &npm_fallback {
                    let base = base_name(pkg);
                    if let Some(version) = read_installed_version(base) {
                        let _ = registry::fill_store_from_registry(base, &version, &cache_dir);
                    }
                    if !options.quiet {
                        utils::log_file_only(&format!("Installed {} via backend.", pkg));
                    }
                }
                let _ = bin_links::rebuild_bin_links(node_modules);
                sync_hidden_lockfile();
                let _ = crate::capsule::write_capsule_from_project(Path::new("."));
                remember_installed_packages(node_modules);
                write_install_markers();
                #[cfg(feature = "pipeline")]
                post_install_jgrid_ingest(None);
                return Ok(());
            }
            Err(e) => {
                if attempts <= 1 {
                    return Err(crate::error_handling::utils::application_error(
                        "backend_install",
                        Some("install_failed"),
                        &e,
                    ));
                }
                if !options.quiet {
                    crate::utils::log("Install failed, retrying in 2s...");
                }
            }
        }
        attempts -= 1;
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod install_materialization_tests {
    use super::{
        can_reuse_prepared_layout, can_use_instant_lock_proof, entry_is_already_installed,
        lockfile_link_root_for_install_path, materialize_downloaded_package,
        publish_extracted_package, publish_extracted_package_with_hook, safe_lockfile_install_path,
        InstallOptions,
    };
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::path::Path;

    #[test]
    fn instant_lock_bypass_requires_a_valid_exact_layout_and_plain_options() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"name":"root","dependencies":{"demo":"1.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{"":{"name":"root"},"node_modules/demo":{"version":"1.0.0"}}}"#,
        )
        .unwrap();
        let demo = temp.path().join("node_modules/demo");
        std::fs::create_dir_all(&demo).unwrap();
        std::fs::write(
            demo.join("package.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .unwrap();
        let proof = crate::instant_lock_proof::project_identity(temp.path()).unwrap();
        std::fs::write(temp.path().join("node_modules/.jhol-ok"), proof).unwrap();

        let options = InstallOptions::default();
        assert!(can_use_instant_lock_proof(&[], &options, temp.path()));
        assert!(!can_use_instant_lock_proof(
            &["demo"],
            &options,
            temp.path()
        ));
        let mut ci = options.clone();
        ci.strict_lockfile = true;
        assert!(!can_use_instant_lock_proof(&[], &ci, temp.path()));

        let mut cli = options.clone();
        cli.from_lockfile = true;
        cli.backend = crate::backend::Backend::Bun;
        cli.telemetry = true;
        assert!(can_reuse_prepared_layout(&["demo@1.0.0"], &cli));
        assert!(!can_use_instant_lock_proof(&[], &cli, temp.path()));
        cli.strict_lockfile = true;
        assert!(!can_reuse_prepared_layout(&["demo@1.0.0"], &cli));
        cli.strict_lockfile = false;
        cli.from_lockfile = false;
        assert!(!can_reuse_prepared_layout(&["demo@1.0.0"], &cli));

        std::fs::write(
            temp.path().join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{"":{"name":"root"},"node_modules/demo":{"version":"2.0.0"}}}"#,
        )
        .unwrap();
        assert!(!can_use_instant_lock_proof(&[], &options, temp.path()));
    }

    #[test]
    fn lockfile_paths_are_relative_and_confined_to_node_modules() {
        for path in [
            "node_modules/demo",
            "node_modules/parent/node_modules/@scope/pkg",
        ] {
            assert!(safe_lockfile_install_path(path));
        }
        for path in [
            "../outside",
            "node_modules/../outside",
            "/tmp/outside",
            "node_modules",
            "node_modules/./demo",
        ] {
            assert!(!safe_lockfile_install_path(path), "accepted {path}");
        }
    }

    #[test]
    fn installed_check_uses_exact_nested_lockfile_path() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("node_modules/parent/node_modules/demo");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("package.json"),
            r#"{"name":"demo","version":"1.2.3"}"#,
        )
        .unwrap();
        assert!(entry_is_already_installed(
            temp.path(),
            "node_modules/parent/node_modules/demo",
            "1.2.3"
        ));
        assert!(!entry_is_already_installed(
            temp.path(),
            "node_modules/demo",
            "1.2.3"
        ));
        assert!(!entry_is_already_installed(
            temp.path(),
            "node_modules/parent/node_modules/demo",
            "1.2.4"
        ));
    }

    #[test]
    fn failed_package_publish_restores_previous_output() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("node_modules/demo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old.txt"), "old").unwrap();
        let missing_candidate = temp.path().join("node_modules/.missing-stage");
        assert!(publish_extracted_package(&missing_candidate, &target).is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("old.txt")).unwrap(),
            "old"
        );
    }

    #[test]
    fn interrupted_package_publish_leaves_previous_output_recoverable() {
        const CHILD: &str = "JHOL_TEST_PACKAGE_PUBLISH_INTERRUPTED_CHILD";
        const ROOT: &str = "JHOL_TEST_PACKAGE_PUBLISH_INTERRUPTED_ROOT";
        if std::env::var_os(CHILD).is_some() {
            let root = std::path::PathBuf::from(std::env::var_os(ROOT).expect("test root"));
            let parent = root.join("node_modules");
            let target = parent.join("demo");
            let candidate = parent.join(".stage-demo");
            publish_extracted_package_with_hook(&candidate, &target, || {
                std::fs::write(root.join("publication-paused"), b"ready").unwrap();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            })
            .expect("the parent test terminates this publication");
            unreachable!("publication should remain paused until terminated");
        }

        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("node_modules");
        let target = parent.join("demo");
        let candidate = parent.join(".stage-demo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old.txt"), "old").unwrap();
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("new.txt"), "new").unwrap();

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "install::install_materialization_tests::interrupted_package_publish_leaves_previous_output_recoverable",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(ROOT, temp.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();

        let ready = temp.path().join("publication-paused");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ready.exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("child exited before publication pause: {status}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child did not reach the publication interruption point"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        assert!(
            !target.exists(),
            "old target was moved before the interruption"
        );
        let backup = std::fs::read_dir(&parent)
            .unwrap()
            .map(Result::unwrap)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".jhol-old-demo-"))
            })
            .expect("previous package remains at its recoverable backup path");
        assert_eq!(
            std::fs::read_to_string(backup.join("old.txt")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(candidate.join("new.txt")).unwrap(),
            "new",
            "verified candidate remains available after interruption"
        );
        super::recover_interrupted_package_publish(&target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("old.txt")).unwrap(),
            "old",
            "interrupted publication restores the prior package on the next install"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn package_publish_recovers_interrupted_backup_before_replacing_output() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("node_modules");
        std::fs::create_dir_all(&parent).unwrap();
        let target = parent.join("demo");
        let backup = parent.join(".jhol-old-demo-abandoned");
        let candidate = parent.join(".stage-demo");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("old.txt"), "old").unwrap();
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("new.txt"), "new").unwrap();

        publish_extracted_package(&candidate, &target).unwrap();

        assert!(!backup.exists(), "recovered backup must not be orphaned");
        assert_eq!(
            std::fs::read_to_string(target.join("new.txt")).unwrap(),
            "new"
        );
    }

    #[test]
    fn package_publish_recovery_preserves_competing_and_ambiguous_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("node_modules");
        std::fs::create_dir_all(&parent).unwrap();
        let target = parent.join("demo");
        let first_backup = parent.join(".jhol-old-demo-first");
        let second_backup = parent.join(".jhol-old-demo-second");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("other.txt"), "competing").unwrap();
        std::fs::create_dir_all(&first_backup).unwrap();
        std::fs::write(first_backup.join("old.txt"), "old one").unwrap();
        std::fs::create_dir_all(&second_backup).unwrap();
        std::fs::write(second_backup.join("old.txt"), "old two").unwrap();

        super::recover_interrupted_package_publish(&target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("other.txt")).unwrap(),
            "competing"
        );
        std::fs::remove_dir_all(&target).unwrap();

        assert!(super::recover_interrupted_package_publish(&target).is_err());
        assert!(!target.exists());
        assert_eq!(
            std::fs::read_to_string(first_backup.join("old.txt")).unwrap(),
            "old one"
        );
        assert_eq!(
            std::fs::read_to_string(second_backup.join("old.txt")).unwrap(),
            "old two"
        );
    }

    #[test]
    fn racing_package_publish_preserves_competing_output_and_previous_backup() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("node_modules");
        let target = parent.join("demo");
        let candidate = parent.join(".stage-demo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old.txt"), "old").unwrap();
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("new.txt"), "new").unwrap();

        let error = publish_extracted_package_with_hook(&candidate, &target, || {
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("other-writer.txt"), "other writer").unwrap();
        })
        .expect_err("publication must report a competing target");

        assert!(error.contains("competing output preserved"), "{error}");
        assert_eq!(
            std::fs::read_to_string(target.join("other-writer.txt")).unwrap(),
            "other writer"
        );
        assert_eq!(
            std::fs::read_to_string(candidate.join("new.txt")).unwrap(),
            "new",
            "failed publication keeps the verified candidate available"
        );
        let backup = std::fs::read_dir(&parent)
            .unwrap()
            .map(Result::unwrap)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".jhol-old-"))
            })
            .expect("previous target is retained at its recovery path");
        assert_eq!(
            std::fs::read_to_string(backup.join("old.txt")).unwrap(),
            "old"
        );
    }

    #[test]
    fn package_publish_replaces_output_only_after_candidate_exists() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("node_modules");
        let target = parent.join("demo");
        let candidate = parent.join(".stage-demo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old.txt"), "old").unwrap();
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("new.txt"), "new").unwrap();
        publish_extracted_package(&candidate, &target).unwrap();
        assert!(!target.join("old.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("new.txt")).unwrap(),
            "new"
        );
    }

    #[test]
    fn lockfile_link_root_handles_scoped_top_level_and_nested_packages() {
        let node_modules = Path::new("node_modules");
        assert_eq!(
            lockfile_link_root_for_install_path(
                Path::new("node_modules/@scope/pkg"),
                "@scope/pkg",
                node_modules,
            ),
            node_modules,
        );
        assert_eq!(
            lockfile_link_root_for_install_path(
                Path::new("node_modules/parent/node_modules/@scope/pkg"),
                "@scope/pkg",
                node_modules,
            ),
            Path::new("node_modules/parent/node_modules"),
        );
        assert_eq!(
            lockfile_link_root_for_install_path(
                Path::new("node_modules/parent/node_modules/child"),
                "child",
                node_modules,
            ),
            Path::new("node_modules/parent/node_modules"),
        );
    }

    #[test]
    fn scoped_store_link_lands_at_lockfile_install_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let node_modules = temp.path().join("node_modules");
        let store = temp.path().join("store");
        std::fs::create_dir_all(&store).expect("store package");
        std::fs::write(store.join("package.json"), r#"{"name":"@scope/pkg"}"#)
            .expect("store manifest");

        for install_path in [
            node_modules.join("@scope/pkg"),
            node_modules.join("parent/node_modules/@scope/pkg"),
        ] {
            let link_root =
                lockfile_link_root_for_install_path(&install_path, "@scope/pkg", &node_modules);
            match crate::utils::link_package_from_store(&store, &link_root, "@scope/pkg") {
                Ok(()) => {
                    assert!(install_path.join("package.json").is_file());
                    let wrong_path = install_path.parent().unwrap().join("@scope/pkg");
                    assert!(!wrong_path.exists());
                }
                Err(error) if error == "JHOL_LINK=0" => return,
                Err(error) => panic!("scoped package link failed: {error}"),
            }
        }
    }

    #[test]
    fn downloaded_package_materialization_reports_bin_link_failure() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = temp.path().join("store");
        let unpacked = temp.path().join("unpacked");
        let node_modules = temp.path().join("node_modules");
        std::fs::create_dir_all(unpacked.join("bin")).expect("unpacked package");
        std::fs::create_dir_all(&node_modules).expect("node_modules");
        std::fs::write(
            unpacked.join("package.json"),
            r#"{"name":"demo-cli","version":"1.0.0","bin":{"demo-cli":"bin/cli.js"}}"#,
        )
        .expect("package manifest");
        std::fs::write(unpacked.join("bin/cli.js"), "#!/usr/bin/env node\n")
            .expect("binary target");
        std::fs::write(node_modules.join(".bin"), "not a directory")
            .expect("block bin directory creation");

        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (path, bytes) in [
            (
                "package/package.json",
                &br#"{"name":"demo-cli","version":"1.0.0","bin":{"demo-cli":"bin/cli.js"}}"#[..],
            ),
            ("package/bin/cli.js", &b"#!/usr/bin/env node\n"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, path, bytes)
                .expect("append archive entry");
        }
        let compressed = archive
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip");
        std::fs::write(&store, compressed).expect("write archive");

        let result = materialize_downloaded_package(&store, &unpacked, &node_modules, "demo-cli");
        assert!(result.is_err(), "install must report failed .bin creation");
    }
}

