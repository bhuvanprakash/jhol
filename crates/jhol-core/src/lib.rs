//! Core library for Jhol: cache, install, doctor, registry, lockfile, backend, audit.
//! Used by the CLI binary; can be reused by other tools (e.g. LSP, server).

pub mod adversarial_install;
pub mod attest;
pub mod env_fork;
pub mod audit;
pub mod axes;
pub mod backend;
pub mod bin_links;
pub mod binary_cache;
pub mod binary_manifest;
pub mod bucket_vsids;
pub mod capsule;
pub mod cas;
pub mod config;
pub mod content;
pub mod daemon_cache;
pub mod doctor;
pub mod engines;
pub mod enterprise;
pub mod error_handling;
pub mod exec;
pub mod fast_stream;
pub mod file_lock;
pub mod git_lifecycle;
pub mod global_cache;
pub mod global_install;
#[cfg(feature = "http-reqwest")]
pub mod http2_download;
pub mod http_client;
pub mod install;
pub mod install_log;
#[path = "../../jhol-trampoline/src/proof.rs"]
pub mod instant_lock_proof;
pub mod jagrs;
#[cfg(feature = "pipeline")]
pub mod jgrid;
pub mod layout;
pub mod lifecycle;
pub mod lockfile;
pub mod lockfile_write;
pub mod materialize;
pub mod mcp;
pub mod model_lock;
pub mod offline_cache;
pub mod optimized_download;
pub mod omit;
pub mod origin;
pub mod pack;
pub mod pm_field;
pub mod patch;
pub mod package_index;
pub mod packument;
pub mod pool;
pub mod prefetch;
pub mod progress;
pub mod project_lock;
pub mod pubgrub;
pub mod publish;
pub mod paper;
pub mod pypi;
pub mod pyinstall;
pub mod pyresolve;
pub mod registry;
pub mod registry_query;
pub mod resolution_mirror;
pub mod run;
pub mod safe_file;
pub mod security_guard;
pub mod selective_extract;
pub mod soul_path;
pub mod spec_grammar;
pub mod task_queue;
pub use jhol_treecheck as tree_state;
pub mod utils;
pub mod why;
pub mod why_not;
pub mod ux;
pub mod workspaces;
pub mod zero_copy_metadata;

// Innovation modules
pub mod binary_lockfile;
pub mod crypto;
pub mod feature_graph;
pub mod grm;
pub mod incremental_hashing;
pub mod incremental_resolution;
pub mod jagr;
pub mod jagr_index;
pub mod jagr_nest;
pub mod jagr_infinite;
pub mod lattice_cache;
pub mod lock_free_grm;
pub mod mpt_store;
pub mod predictive_prefetch;
pub mod sat_resolver;
pub mod second_order_markov;
pub mod spectral_sparsifier;

#[cfg(target_os = "linux")]
pub mod fuse_mount;
#[cfg(target_os = "linux")]
pub mod io_uring_wrapper;

#[cfg(test)]
mod registry_tests;
#[cfg(test)]
// Merged modules kept reachable under original names (no API break):
pub use audit::osv;
pub use grm::distributed as distributed_grm;
pub use http_client::cdn;
pub use incremental_resolution::incremental_resolver;

// ── Re-exports for CLI ──────────────────────────────────────────────────────────

pub use audit::{
    generate_sbom, run_audit, run_audit_fix, run_audit_gate, run_audit_raw, SbomFormat,
};
pub use backend::{resolve_backend, Backend};
pub use cdn::{esm_sh_url, fetch_esm_to_file};
pub use config::{
    apply_enterprise_network_env, load_config, load_jagr_config, telemetry_enabled, Config,
    JagrConfig,
};
pub use doctor::{check_dependencies, explain_project_health, fix_dependencies, native_outdated};
pub use exec::{exec_binary, find_binary_in_node_modules};
pub use install::{
    backfill_lockfile_integrity_after_install, count_installed_packages, fast_direct_warm_install,
    install_lockfile_only, install_package, resolve_install_from_package_json,
    resolve_install_from_package_json_omit,
    try_fresh_project_bundle_replay, write_install_markers, InstallOptions,
};
pub use lockfile::{
    detect_lockfile, lockfile_integrity_complete, read_all_resolved_specs_from_dir,
    read_resolved_from_dir, LockfileKind,
};
pub use lockfile_write::{
    resolve_full_tree, resolve_full_tree_jagr_infinite, write_package_lock, ResolvedPackage,
};
pub use progress::{snapshot as progress_snapshot, ProgressSnapshot};
pub use run::run_script;
pub use utils::{
    cache_clean,
    cache_clean_report, cache_export, cache_import, cache_prune, cache_size_bytes, get_cache_dir,
    init_cache, list_cached_packages, lockfile_content_hash, read_fallback_telemetry,
    validate_package_name,
};
pub use ux::{import_lockfile, uninstall, update_packages, why_package};
pub use workspaces::list_workspace_roots;

// JAGR — commented out until jagr module is fully available on main
// pub use jagr::JagrResolver;

// ── External function re-exports (needed by main.rs) ──────────────────────────
pub use prefetch::prefetch_from_lockfile;
pub use utils::{log, set_output_silenced};

/// Check whether the current project is installed and current.
///
/// The proof and the recorded filesystem state are *hints*: they can prove nothing
/// changed around the tree, never that the tree is what the lockfile authorises. A
/// package.json edited inside a package and a directory the lockfile never
/// mentioned both satisfied them, which is how a tampered tree could be reported
/// as up to date. The verdict now comes from the tree itself, so every caller of
/// this function inherits the authority instead of re-deriving it — the property
/// this project keeps needing and keeps having to re-apply per lane.
pub fn lockfile_fingerprint_matches() -> bool {
    let cwd = std::path::Path::new(".");
    crate::instant_lock_proof::prepared_tree_is_valid(cwd)
        && crate::daemon_cache::can_skip_install_for_dir(cwd)
        && crate::tree_state::verify_tree(cwd).is_match()
}

/// Check whether an installable lockfile exists.
///
/// npm-compatible package locks are authoritative too; requiring a private
/// marker wrongly sent them through cache-only capsule replay.
pub fn has_jhol_lockfile(project_dir: &std::path::Path) -> bool {
    // A jhol lockfile is `package-lock.json` carrying either the jhol marker or real
    // installable entries. Deliberately NOT "any file we could read entries from":
    // bun.lock / npm-shrinkwrap must not make this true (they are not jhol lockfiles),
    // which is what this function means to callers.
    crate::lockfile::package_lock_is_usable(project_dir)
}

#[cfg(test)]
mod lockfile_presence_tests {
    #[test]
    fn fingerprint_shortcut_rejects_missing_locked_package_manifest() {
        const CHILD: &str = "JHOL_TEST_FINGERPRINT_LAYOUT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let root = std::path::Path::new(".");
            let package_dir = root.join("node_modules/victim");
            std::fs::create_dir_all(&package_dir).unwrap();
            std::fs::write(
                root.join("package.json"),
                r#"{"name":"fixture","version":"1.0.0","dependencies":{"victim":"1.0.0"}}"#,
            )
            .unwrap();
            std::fs::write(
                root.join("package-lock.json"),
                r#"{"lockfileVersion":3,"packages":{"":{"name":"fixture","version":"1.0.0","dependencies":{"victim":"1.0.0"}},"node_modules/victim":{"name":"victim","version":"1.0.0"}}}"#,
            )
            .unwrap();
            std::fs::write(
                package_dir.join("package.json"),
                r#"{"name":"victim","version":"1.0.0"}"#,
            )
            .unwrap();

            let identity = crate::instant_lock_proof::project_identity(root).unwrap();
            std::fs::write(root.join("node_modules/.jhol-ok"), identity).unwrap();
            let hash = crate::capsule::compute_manifest_hash(root).unwrap();
            std::fs::write(root.join("node_modules/.nm_fingerprint"), hash).unwrap();
            crate::daemon_cache::write_status(hash, root);
            assert!(crate::instant_lock_proof::prepared_tree_is_valid(root));

            std::fs::remove_file(package_dir.join("package.json")).unwrap();
            assert!(crate::daemon_cache::can_skip_install_for_dir(root));
            assert!(
                !super::lockfile_fingerprint_matches(),
                "fingerprint shortcut accepted a missing locked package manifest"
            );
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lockfile_presence_tests::fingerprint_shortcut_rejects_missing_locked_package_manifest",
                "--nocapture",
            ])
            .current_dir(project)
            .env(CHILD, "1")
            .env("HOME", home)
            .env_remove("USERPROFILE")
            .env_remove("JHOL_SCRIPT_ALLOWLIST")
            .env("JHOL_STATUS_FILE", temp.path().join("status.bin"))
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "isolated proof probe failed:\n{}",
            String::from_utf8_lossy(&child.stderr)
        );
    }

    #[test]
    fn npm_lockfile_is_authoritative_without_jhol_marker() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("package-lock.json"),
            r#"{
                "lockfileVersion": 3,
                "packages": {
                    "": { "name": "fixture" },
                    "node_modules/a": {
                        "version": "1.0.0",
                        "resolved": "https://registry.npmjs.org/a/-/a-1.0.0.tgz",
                        "integrity": "sha512-test"
                    }
                }
            }"#,
        )
        .unwrap();

        assert!(super::has_jhol_lockfile(temp.path()));
    }
}

// ── Audit re-exports (may be replaced by the full audit module later) ─────────
pub use audit::{
    run_audit_filtered, run_audit_fix_filtered, run_audit_gate_filtered, run_audit_raw_filtered,
};

// ── JAGR stubs (to be replaced by the full jagr module) ───────────────────────
/// A single Betti number history entry (tracks cycle complexity over time).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BettiHistoryEntry {
    pub timestamp: u64,
    pub betti_number: u32,
    pub module_count: usize,
    pub edge_count: usize,
    pub betti_1_before: u32,
    pub betti_1_after: u32,
    pub b0: u32,
    pub b1: u32,
    pub b2: u32,
    pub package_count: u32,
    pub snf_satisfied: bool,
    pub strategy: String,
}

/// Full JAGR analysis report for a project.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JagrAnalysisReport {
    pub betti: Vec<BettiHistoryEntry>,
    pub stability: crate::StabilityClass,
    pub topology_hash: u64,
    pub module_count: usize,
    pub betti_0: u32,
    pub betti_1: u32,
    pub betti_2: u32,
    pub snf_satisfied: bool,
    pub snf_trivial: bool,
    pub snf_difficulty: String,
    pub gluing_has_obstruction: bool,
    pub is_satisfiable: bool,
    pub cost: f64,
    pub strategy_used: String,
    pub layer_times_ms: std::collections::HashMap<String, f64>,
    pub layer1_cost: f64,
    pub layer1_satisfiable: bool,
    pub layer1_variant: String,
    pub layer3_satisfiable: bool,
    pub layer3_kind: String,
    pub derivation_node_count: usize,
    pub derivation_edge_count: usize,
    pub selection: std::collections::HashMap<String, String>,
    pub explanation: String,
    pub manifest_path: String,
    pub root_name: String,
    pub root_version: String,
    pub package_count: usize,
    pub version_pair_count: usize,
    pub edge_count: usize,
    pub package: String,
    pub from_version: String,
    pub to_version: String,
    pub risk: crate::UpgradeRisk,
    pub betti_1_before: u32,
    pub betti_1_after: u32,
    pub snf_satisfied_before: bool,
    pub snf_satisfied_after: bool,
    pub recommendation: String,
    pub differential: Option<Box<JagrAnalysisReport>>,
    pub fiedler_value: f64,
    pub predicted_beta1: f64,
    pub spectral_gap: f64,
}

impl std::fmt::Display for JagrAnalysisReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JAGR({}) β₁={} satisfiable={}",
            self.strategy_used, self.betti_1, self.is_satisfiable
        )
    }
}

impl JagrAnalysisReport {
    pub fn derivation_nodes_for_graph_export(&self) -> Vec<String> {
        let mut nodes: Vec<String> = self
            .selection
            .iter()
            .map(|(k, v)| format!("{}@{}", k, v))
            .collect();
        nodes.sort();
        nodes
    }
    pub fn derivation_edge_count_at(&self, _i: usize) -> usize {
        1
    }
    pub fn selection_with_explanations(&self) -> Vec<(String, String)> {
        let mut list: Vec<(String, String)> = self
            .selection
            .iter()
            .map(|(k, v)| (k.clone(), format!("resolved to {}", v)))
            .collect();
        list.sort();
        list
    }
    pub fn high_risk_packages(&self, _threshold: f64) -> Vec<(String, f64)> {
        vec![]
    }
}

/// Build a report from a resolver run.
///
/// Shared by `jagr_analyze_project_manifest` and the resolver comparison, so both
/// describe a run the same way: which strategy ran, whether it produced a tree, the
/// selection it produced, and how long it took.
fn resolver_report(
    path: &std::path::Path,
    strategy: &str,
    result: Result<
        std::collections::HashMap<String, crate::lockfile_write::ResolvedPackage>,
        String,
    >,
    elapsed: std::time::Duration,
) -> JagrAnalysisReport {
    let (satisfiable, selection, explanation, nodes, adj) = match result {
        Ok(tree) => {
            let mut sel = std::collections::HashMap::new();
            let mut node_set = std::collections::HashSet::new();
            let mut adjacency: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();

            for (k, v) in &tree {
                sel.insert(k.clone(), v.version.clone());
                node_set.insert(k.clone());
                for dep in v.dependencies.keys() {
                    if tree.contains_key(dep) {
                        adjacency.entry(k.clone()).or_default().push(dep.clone());
                        node_set.insert(dep.clone());
                    }
                }
                for peer in v.peer_dependencies.keys() {
                    if tree.contains_key(peer) {
                        adjacency.entry(k.clone()).or_default().push(peer.clone());
                        node_set.insert(peer.clone());
                    }
                }
            }
            let empty = sel.is_empty();
            (
                !empty,
                sel,
                if empty {
                    "resolved no packages".to_string()
                } else {
                    String::new()
                },
                node_set,
                adjacency,
            )
        }
        Err(e) => (
            false,
            std::collections::HashMap::new(),
            e,
            std::collections::HashSet::new(),
            std::collections::HashMap::new(),
        ),
    };

    let invariants = crate::jagr_infinite::sheaf_laplacian::compute_sheaf_invariants(&nodes, &adj);
    let ricci = crate::jagr_infinite::ricci_curvature::compute_ricci_curvatures(&adj);
    let rg = crate::jagr_infinite::holographic_rg::compute_wilsonian_rg_decimation(&nodes, &adj);

    let mut times = std::collections::HashMap::new();
    times.insert("total".to_string(), elapsed.as_secs_f64() * 1000.0);
    times.insert("layer2".to_string(), invariants.compute_time_us / 1000.0);
    times.insert("ricci_flow".to_string(), ricci.compute_time_us / 1000.0);
    times.insert("wilsonian_rg".to_string(), rg.decimation_time_us / 1000.0);

    let pkg_count = selection.len();
    let edge_count = ricci.total_edges;
    let snf_trivial = invariants.betti_1 == 0;
    let snf_difficulty = if invariants.betti_1 == 0 {
        "trivial".to_string()
    } else if invariants.betti_2 > 0 {
        "torsion-resolved".to_string()
    } else {
        "cohomological-cycle".to_string()
    };
    let gluing_has_obstruction = invariants.betti_1 > 0 && !invariants.snf_satisfied;

    JagrAnalysisReport {
        betti: vec![],
        stability: crate::StabilityClass::Stable,
        topology_hash: (invariants.betti_0 as u64)
            ^ ((invariants.betti_1 as u64) << 16)
            ^ ((invariants.betti_2 as u64) << 32),
        module_count: pkg_count,
        betti_0: invariants.betti_0,
        betti_1: invariants.betti_1,
        betti_2: invariants.betti_2,
        snf_satisfied: invariants.snf_satisfied && satisfiable,
        snf_trivial,
        snf_difficulty,
        gluing_has_obstruction,
        is_satisfiable: satisfiable,
        cost: 0.0,
        strategy_used: strategy.to_string(),
        layer_times_ms: times,
        layer1_cost: 0.0,
        layer1_satisfiable: satisfiable,
        layer1_variant: "none".to_string(),
        layer3_satisfiable: satisfiable,
        layer3_kind: strategy.to_string(),
        derivation_node_count: pkg_count,
        derivation_edge_count: edge_count,
        selection,
        explanation,
        manifest_path: path.to_string_lossy().to_string(),
        root_name: String::new(),
        root_version: String::new(),
        package_count: pkg_count,
        version_pair_count: edge_count,
        edge_count,
        package: String::new(),
        from_version: String::new(),
        to_version: String::new(),
        risk: crate::UpgradeRisk::SNFStable,
        betti_1_before: 0,
        betti_1_after: invariants.betti_1,
        snf_satisfied_before: satisfiable,
        snf_satisfied_after: satisfiable,
        recommendation: String::new(),
        differential: None,
        fiedler_value: invariants.fiedler_value,
        predicted_beta1: invariants.betti_1 as f64,
        spectral_gap: invariants.spectral_gap,
    }
}

/// Analyze a project manifest and return a JAGR report.
/// Restore the default SIGPIPE disposition.
///
/// Rust ignores SIGPIPE, so writing to a closed pipe (`jhol cache size | head`)
/// surfaced as a `Broken pipe` panic, and release builds use `panic = "abort"`, which
/// aborted the process. Called first thing in `main`.
#[cfg(unix)]
pub fn restore_default_sigpipe() {
    // Safety: resetting a signal disposition to its default is always sound.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub fn restore_default_sigpipe() {}

pub fn jagr_analyze_project_manifest(path: &std::path::Path) -> Result<JagrAnalysisReport, String> {
    // Was a fixed "stub" report (all zeros, strategy_used = "stub"), so `jhol jagr`,
    // `audit-jagr`, `jagr-impact` and the `--jagr` diagnostics reported success with no
    // data. Runs the same resolution the installer uses.
    let t0 = std::time::Instant::now();
    let result = crate::jagr_infinite::resolve_full_tree_jagr_infinite(path);
    let elapsed = t0.elapsed();
    let mut report = resolver_report(path, "jagr-infinite", result, elapsed);
    report.manifest_path = path.to_string_lossy().to_string();
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            report.root_name = v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            report.root_version = v
                .get("version")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            report.package_count = report.selection.len();
            report.module_count = report.selection.len();
            report.derivation_node_count = report.selection.len();
        }
    }
    // Real Sheaf Laplacian invariants (β₀, β₁, β₂, spectral gap, Fiedler value)
    // and Forman-Ricci curvature bottlenecks are computed directly in resolver_report.
    Ok(report)
}

pub fn jagr_analyze_project_manifest_with_resolver(
    _path: &std::path::Path,
    _resolver: &crate::jagr::JagrResolver,
    _cfg: &crate::config::JagrConfig,
) -> Result<JagrAnalysisReport, String> {
    jagr_analyze_project_manifest(_path)
}

pub fn jagr_read_betti_history(_path: &std::path::Path) -> Vec<BettiHistoryEntry> {
    vec![]
}

pub fn jagr_save_audit_trail(
    report: &JagrAnalysisReport,
    path: &std::path::Path,
) -> Result<(), String> {
    // Was a stub returning Ok(()) while the CLI printed "saved to <path>" and exited
    // 0 - a silent false success for the feature whose point is an auditable record.
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let json = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn jagr_load_audit_trail(path: &std::path::Path) -> Result<JagrAnalysisReport, String> {
    // Used to ignore the file and re-analyse the path as a manifest, so `--load
    // report.json` produced a fresh analysis and `--diff` compared runs, not records.
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read report {}: {}", path.display(), e))?;
    serde_json::from_str(&raw).map_err(|e| format!("invalid report {}: {}", path.display(), e))
}

pub fn jagr_compare_resolvers(_path: &std::path::Path) -> Result<Vec<JagrAnalysisReport>, String> {
    let r = jagr_analyze_project_manifest(_path)?;
    Ok(vec![r.clone(), r])
}

pub fn jagr_diff_reports(
    _before: &JagrAnalysisReport,
    _after: &JagrAnalysisReport,
) -> Vec<JagrAnalysisReport> {
    vec![]
}

pub fn jagr_simulate_upgrade(
    path: &std::path::Path,
    pkg: &str,
    version: &str,
) -> Result<JagrAnalysisReport, String> {
    // Ignored its arguments before, so `jagr-impact --target pkg@ver` printed empty
    // Package/From/To fields and exited 0.
    let mut report = jagr_analyze_project_manifest(path)?;
    report.package = pkg.to_string();
    report.from_version = report
        .selection
        .get(&format!("node_modules/{}", pkg))
        .cloned()
        .unwrap_or_default();
    report.to_version = version.to_string();
    let from = semver::Version::parse(&report.from_version).ok();
    let to = semver::Version::parse(version).ok();
    let (risk, recommendation) = match (from, to) {
        (Some(f), Some(t)) if t < f => (
            UpgradeRisk::TopologyChanged,
            format!("downgrade {} -> {}", f, t),
        ),
        (Some(f), Some(t)) if t.major != f.major => (
            UpgradeRisk::SNFBroken,
            format!("major upgrade {} -> {}: expect breaking changes", f, t),
        ),
        (Some(f), Some(t)) if t.minor != f.minor => (
            UpgradeRisk::TopologyChanged,
            format!("minor upgrade {} -> {}", f, t),
        ),
        (Some(f), Some(t)) => (
            UpgradeRisk::SNFStable,
            format!("patch upgrade {} -> {}", f, t),
        ),
        _ => (
            UpgradeRisk::SNFStable,
            if report.from_version.is_empty() {
                format!(
                    "{} is not part of the current resolution; target {} not compared",
                    pkg, version
                )
            } else {
                format!("cannot compare {} with {}", report.from_version, version)
            },
        ),
    };
    report.risk = risk;
    report.recommendation = recommendation;
    report.betti_1_before = report.betti_1;
    report.betti_1_after = report.betti_1;
    report.snf_satisfied_before = report.is_satisfiable;
    report.snf_satisfied_after = report.is_satisfiable;
    Ok(report)
}

pub fn jagr_upgrade_plan(path: &std::path::Path) -> Result<Vec<JagrAnalysisReport>, String> {
    // Returned an empty list before, which reads as "nothing to upgrade". Still does
    // not query the registry per package (that is `jhol outdated`'s job).
    let deps = crate::lockfile::read_package_json_deps(path).unwrap_or_default();
    let mut out = Vec::new();
    for (name, spec) in deps {
        let mut r = jagr_analyze_project_manifest(path)?;
        r.package = name.clone();
        r.from_version = r
            .selection
            .get(&format!("node_modules/{}", name))
            .cloned()
            .unwrap_or_default();
        r.to_version = r.from_version.clone();
        r.recommendation = format!(
            "resolved {} at {}{}",
            name,
            if r.from_version.is_empty() {
                "?"
            } else {
                &r.from_version
            },
            if r.from_version.is_empty() {
                String::new()
            } else {
                format!(" (declared {})", spec)
            }
        );
        out.push(r);
    }
    Ok(out)
}

pub fn jagr_why_package_deep(path: &std::path::Path, pkg: &str) -> Result<String, String> {
    // Returned the placeholder "JAGR analysis not available in this build." before, so
    // `jhol why --jagr <pkg>` told the user nothing.
    let tree = crate::jagr_infinite::resolve_full_tree_jagr_infinite(path)?;
    let key = format!("node_modules/{}", pkg);
    let mut dependents: Vec<String> = tree
        .iter()
        .filter(|(_, v)| v.dependencies.contains_key(pkg))
        .map(|(k, v)| {
            let spec = v
                .dependencies
                .get(pkg)
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            format!("{}{}", k, spec)
        })
        .collect();
    dependents.sort();
    let mut out = String::new();
    match tree.get(&key) {
        Some(v) => out.push_str(&format!(
            "{}@{} is installed; package.json declares {}\n",
            pkg,
            v.version,
            crate::lockfile::read_package_json_deps(path)
                .and_then(|d| d.get(pkg).cloned())
                .unwrap_or_else(|| "-".to_string())
        )),
        None => out.push_str(&format!("{} is not part of the resolved tree\n", pkg)),
    }
    if dependents.is_empty() {
        out.push_str("no resolved package depends on it (it is required directly or unused)\n");
    } else {
        out.push_str(&format!(
            "depended on by {} package(s):\n",
            dependents.len()
        ));
        for d in dependents.iter().take(20) {
            out.push_str(&format!("  - {}\n", d));
        }
    }
    Ok(out)
}

// ── Missing stubs for main.rs integration ──────────────────────────────────────
// These are referenced by main.rs but weren't ported during the worktree merge.
// Each stub preserves the interface so the build works without losing code.

/// Enable daemon mode for this process (CLI sets this before install).
static DAEMON_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn set_daemon_mode(enabled: bool) {
    DAEMON_MODE.store(enabled, std::sync::atomic::Ordering::Release);
}
pub fn daemon_mode() -> bool {
    DAEMON_MODE.load(std::sync::atomic::Ordering::Acquire)
}

/// Set transitive resolution depth (used by daemon path).
static TRANSITIVE_DEPTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(64);
pub fn set_transitive_depth(d: u32) {
    TRANSITIVE_DEPTH.store(d, std::sync::atomic::Ordering::Release);
}

/// Upgrade risk classification (from JAGR analysis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UpgradeRisk {
    SNFStable,
    TopologyChanged,
    SNFBroken,
    Unsatisfiable,
    Skipped,
}

impl UpgradeRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            UpgradeRisk::SNFStable => "snf_stable",
            UpgradeRisk::TopologyChanged => "topology_changed",
            UpgradeRisk::SNFBroken => "snf_broken",
            UpgradeRisk::Unsatisfiable => "unsatisfiable",
            UpgradeRisk::Skipped => "skipped",
        }
    }
    pub fn emoji(&self) -> &'static str {
        match self {
            UpgradeRisk::SNFStable => "ok",
            UpgradeRisk::TopologyChanged => "!",
            UpgradeRisk::SNFBroken => "x",
            UpgradeRisk::Unsatisfiable => "blocked",
            UpgradeRisk::Skipped => "⏭",
        }
    }
}

/// Stability class for a dependency (from JAGR Betti analysis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StabilityClass {
    Stable,
    Marginal,
    Fragile,
    Degenerate,
}
