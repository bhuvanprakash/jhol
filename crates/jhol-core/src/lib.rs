//! Core library for Jhol: cache, install, doctor, registry, lockfile, backend, audit.
//! Used by the CLI binary; can be reused by other tools (e.g. LSP, server).

pub mod audit;
pub mod backend;
pub mod cdn;
pub mod config;
pub mod doctor;
pub mod exec;
pub mod http_client;
pub mod install;
pub mod lockfile;
pub mod lockfile_write;
pub mod osv;
pub mod prefetch;
pub mod registry;
pub mod run;
pub mod sat_resolver;
pub mod utils;
pub mod ux;
pub mod workspaces;

// Re-export main API for CLI
pub use audit::{generate_sbom, run_audit, run_audit_fix, run_audit_gate, run_audit_raw, SbomFormat};
pub use backend::{bun_available, resolve_backend, Backend};
pub use config::{
    apply_enterprise_network_env,
    effective_registry_url_for_package,
    load_config,
    registry_auth_token_for_url,
    Config,
};
pub use doctor::{check_dependencies, explain_project_health, fix_dependencies};
pub use install::{
    install_lockfile_only, install_package, resolve_install_from_package_json, InstallOptions,
};
pub use prefetch::prefetch_from_lockfile;
pub use lockfile::{detect_lockfile, lockfile_integrity_complete, read_resolved_from_dir, LockfileKind};
pub use run::{get_script_command, run_script};
pub use exec::{exec_binary, find_binary_in_node_modules};
pub use cdn::{esm_sh_url, fetch_esm_to_file};
pub use ux::import_lockfile;
pub use workspaces::list_workspace_roots;
pub use utils::{
    cache_clean, cache_export, cache_import, cache_prune, cache_size_bytes, get_cache_dir,
    init_cache, list_cached_packages, lockfile_content_hash, log, log_error,
    read_fallback_telemetry,
};
pub use ux::{uninstall, update_packages, why_package};
