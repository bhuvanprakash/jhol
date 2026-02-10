//! Core library for Jhol: cache, install, doctor, registry, lockfile, backend, audit.
//! Used by the CLI binary; can be reused by other tools (e.g. LSP, server).

pub mod audit;
pub mod backend;
pub mod config;
pub mod doctor;
pub mod install;
pub mod lockfile;
pub mod registry;
pub mod utils;
pub mod workspaces;

// Re-export main API for CLI
pub use audit::{generate_sbom, run_audit, run_audit_fix, run_audit_raw, SbomFormat};
pub use backend::{bun_available, resolve_backend, Backend};
pub use config::{load_config, Config};
pub use doctor::{check_dependencies, fix_dependencies};
pub use install::{
    install_lockfile_only, install_package, resolve_install_from_package_json, InstallOptions,
};
pub use lockfile::{detect_lockfile, read_resolved_from_dir, LockfileKind};
pub use workspaces::list_workspace_roots;
pub use utils::{
    cache_clean, cache_export, cache_import, cache_prune, cache_size_bytes, get_cache_dir,
    init_cache, list_cached_packages, lockfile_content_hash, log, log_error,
};
