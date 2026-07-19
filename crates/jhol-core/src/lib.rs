//! Core library for Jhol: cache, install, doctor, registry, lockfile, backend, audit.
//! Used by the CLI binary; can be reused by other tools (e.g. LSP, server).

pub mod atomic_ring; // ART v3: Atomic Ring Topology
pub mod audit;
pub mod backend;
pub mod bin_links;
pub mod binary_cache; // JHOL Binary Package Cache (future)
pub mod binary_manifest; // JHOL Binary Manifest (Bun-style caching)
pub mod bucket_vsids; // JHOL O(1) Variable Selection (GipSAT 2025)
pub mod cas; // JAGR-2: Content-Addressable Storage
pub mod cdn;
pub mod config;
pub mod content_addressed_store; // Phase 7: Content-Addressed Store (cross-machine)
#[cfg(feature = "daemon")]
pub mod daemon_cache; // JHOL Topological Pre-fetch Daemon
pub mod doctor;
pub mod enterprise;
pub mod error_handling;
pub mod exec;
pub mod global_cache; // JHOL Global Shared Cache (memory-mapped)
pub mod global_cache_2; // JHOL Global Cache 2.0 (INNOVATIONS BEYOND BUN)
pub mod http_client;
pub mod install;
pub mod install_log; // JHOL Install Event Log (used by TUI)
#[cfg(feature = "js-engine")]
pub mod js_engine_backend;
pub mod lifecycle; // Native lifecycle script executor
pub mod lockfile;
pub mod lockfile_write;
pub mod mpt_store;
pub mod offline_cache; // JHOL Offline Mode
pub mod optimized_download; // JHOL Optimized Download (cache-first)
pub mod osv;
pub mod package_index; // JHOL Pre-Resolved Package Index
pub mod prefetch;
pub mod progress; // JHOL Live Install Progress (lock-free atomics)
pub mod pubgrub; // JAGR-2: PubGrub solver
pub mod registry;
pub mod resolution_mirror; // JMR: JAGR Mirror Resolution
pub mod run;
pub mod safe_file;
pub mod sat_resolver; // JAGR-1: SAT solver (kept for fallback)
pub mod selective_extract; // JHOL Selective Extraction (80% faster)
pub mod task_queue; // JAGR-2: Work-stealing task queue
pub mod utils;
pub mod ux;
pub mod workspaces;

#[cfg(test)]
mod registry_tests;

#[cfg(test)]
mod sat_resolver_tests;

// Re-export main API for CLI
pub use audit::{
    generate_sbom, run_audit, run_audit_filtered, run_audit_fix, run_audit_fix_filtered,
    run_audit_gate, run_audit_gate_filtered, run_audit_raw, run_audit_raw_filtered, SbomFormat,
};
pub use backend::{bun_available, resolve_backend, Backend};
pub use bin_links::{link_bins_for_package, rebuild_bin_links, BinLinkReport};
pub use cdn::{esm_sh_url, fetch_esm_to_file};
pub use config::{
    apply_enterprise_network_env, effective_registry_url_for_package, load_config,
    registry_auth_token_for_url, telemetry_enabled, Config,
};
pub use doctor::{check_dependencies, explain_project_health, fix_dependencies};
pub use enterprise::{EnterpriseConfig, LicenseChecker, SsoTokenManager};
pub use error_handling::{ErrorHandler, JholError, RecoveryStrategy};
pub use exec::{exec_binary, find_binary_in_node_modules};
pub use http_client::get_http_metrics;
pub use install::{
    install_lockfile_only, install_package, lockfile_fingerprint_matches,
    resolve_install_from_package_json, set_daemon_mode, set_transitive_depth, InstallOptions,
    OptimizeStrategy,
};
pub use install_log::{
    append_install_log, read_install_log_newest_first, InstallLogEntry, MAX_ENTRY_AGE_HOURS,
};
pub use lifecycle::{run_all_install_scripts, run_install_scripts};
pub use lockfile::{
    detect_lockfile, has_jhol_lockfile, lockfile_exists, lockfile_integrity_complete,
    read_resolved_from_dir, LockfileKind,
};
pub use prefetch::prefetch_from_lockfile;
pub use progress::{snapshot as progress_snapshot, ProgressSnapshot};
pub use resolution_mirror::flush_mirror_pending;
pub use run::{get_script_command, run_script};
pub use utils::{
    cache_clean, cache_export, cache_import, cache_prune, cache_size_bytes, get_cache_dir,
    init_cache, list_cached_packages, lockfile_content_hash, log, log_error,
    read_fallback_telemetry, sanitize_terminal_output, validate_package_name,
};
pub use ux::import_lockfile;
pub use ux::{uninstall, update_packages, why_package};
pub use workspaces::list_workspace_roots;

// JAGR: Novel Three-Layer Resolver (Tropical Geometry + Sheaf Cohomology + Core-Guided MaxSAT)
pub use jagr::resolution_differential::{
    compute_differential as jagr_compute_differential, format_report as jagr_differential_report,
    ResolutionDifferential, StabilityClass,
};
pub use jagr::{
    jagr_resolve, JagrResolution, JagrResolver, ResolveRequest as JagrResolveRequest, JAGR_VERSION,
};

// JAGR: Public analysis helper for the `jhol jagr` CLI subcommand
pub use lockfile_write::{
    jagr_analyze_project_manifest, jagr_analyze_project_manifest_with_resolver,
    jagr_compare_resolvers, jagr_diff_reports, jagr_load_audit_trail, jagr_read_betti_history,
    jagr_record_betti_history, jagr_record_betti_history_for, jagr_save_audit_trail,
    jagr_simulate_upgrade, jagr_upgrade_plan, jagr_why_package_deep, resolve_full_tree,
    write_package_lock, BettiHistoryEntry, JagrAnalysisReport, ResolvedPackage,
    ResolverComparisonRow, UpgradePlanEntry, UpgradeRisk,
};

// JAGR: Per-project configuration (read from `.jholrc` `jagr` section).
pub use config::{load_jagr_config, JagrConfig};

// Phase 1-4 Innovations
pub mod binary_lockfile;
pub mod capsule;
pub mod crypto;
pub mod distributed_grm;
#[cfg(target_os = "linux")]
pub mod fuse_mount;
pub mod incremental_hashing;
pub mod incremental_resolver;
#[cfg(target_os = "linux")]
pub mod io_uring_wrapper;
pub mod lock_free_grm;
pub mod second_order_markov;

// Phase 6-11 Optimizations
// (Safety, Memory, Parallelism, SIMD, Zero-cost, Security)

// Phase 12-19: Advanced Innovations
pub mod decentralized_registry;
pub mod feature_graph;
pub mod hypergraph_resolver;
pub mod incremental_resolution;
pub mod predictive_prefetch;
pub mod quantum_signing;
pub mod sandbox;
pub mod zero_copy_metadata;

// JAGR: Novel Three-Layer Resolver (Tropical Geometry + Sheaf Cohomology + Core-Guided MaxSAT)
// This is jhol's unique resolver architecture using mathematics never before
// applied to package management.
pub mod jagr;

// Lattice Resolution Cache — persistent (package, constraint) → version cache
pub mod lattice_cache;

// Note: Following modules referenced but not yet implemented
pub mod grm;
// pub mod mpt_store;
// pub mod global_resolution_cache;
// pub mod project_state;
// pub mod global_extracted_store;
// pub mod global_store;
// pub mod global_tarball_store;
// pub mod predictive_prefetch;
// pub mod graph_ir;
// pub mod metadata_summary;
// pub mod resolution_graph_cache;
// pub mod jagr3;
