//! Core library for Jhol: cache, install, doctor, registry, lockfile, backend, audit.
//! Used by the CLI binary; can be reused by other tools (e.g. LSP, server).

pub mod audit;
pub mod backend;
pub mod bin_links;
pub mod binary_cache;
pub mod binary_manifest;
pub mod bucket_vsids;
pub mod capsule;
pub mod cas;
pub mod cdn;
pub mod config;
pub mod daemon_cache;
pub mod doctor;
pub mod enterprise;
pub mod error_handling;
pub mod exec;
pub mod global_cache;
pub mod global_cache_2;
pub mod http_client;
pub mod install;
pub mod lockfile;
pub mod lockfile_write;
pub mod offline_cache;
pub mod optimized_download;
pub mod osv;
pub mod package_index;
pub mod prefetch;
pub mod progress;
pub mod pubgrub;
pub mod registry;
pub mod resolution_mirror;
pub mod run;
pub mod safe_file;
pub mod selective_extract;
pub mod task_queue;
pub mod utils;
pub mod ux;
pub mod workspaces;

// Innovation modules
pub mod binary_lockfile;
pub mod crypto;
pub mod distributed_grm;
pub mod grm;
pub mod incremental_hashing;
pub mod incremental_resolver;
pub mod jagr;
pub mod lock_free_grm;
pub mod mpt_store;
pub mod second_order_markov;
pub mod lattice_cache;
pub mod sat_resolver;

#[cfg(target_os = "linux")]
pub mod fuse_mount;
#[cfg(target_os = "linux")]
pub mod io_uring_wrapper;

#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod sat_resolver_tests;

// ── Re-exports for CLI ──────────────────────────────────────────────────────────

pub use audit::{generate_sbom, run_audit, run_audit_fix, run_audit_gate, run_audit_raw, SbomFormat};
pub use backend::{resolve_backend, Backend};
pub use cdn::{esm_sh_url, fetch_esm_to_file};
pub use config::{apply_enterprise_network_env, load_config, load_jagr_config, telemetry_enabled, Config};
pub use doctor::{check_dependencies, explain_project_health, fix_dependencies};
pub use exec::{exec_binary, find_binary_in_node_modules};
pub use install::{
    install_lockfile_only, install_package, resolve_install_from_package_json, InstallOptions,
};
pub use lockfile::{
    detect_lockfile, lockfile_integrity_complete,
    read_resolved_from_dir, read_all_resolved_specs_from_dir, LockfileKind,
};
pub use lockfile_write::{
    resolve_full_tree, write_package_lock, ResolvedPackage,
};
pub use progress::{snapshot as progress_snapshot, ProgressSnapshot};
pub use run::run_script;
pub use utils::{
    cache_clean, cache_export, cache_import, cache_prune, cache_size_bytes, get_cache_dir,
    init_cache, list_cached_packages, lockfile_content_hash, read_fallback_telemetry,
    validate_package_name,
};
pub use ux::{import_lockfile, uninstall, update_packages, why_package};
pub use workspaces::list_workspace_roots;

// JAGR — commented out until jagr module is fully available on main
// pub use jagr::JagrResolver;
