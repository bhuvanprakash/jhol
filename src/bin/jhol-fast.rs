//! jhol-fast: argument-preserving trampoline to the real `jhol` CLI.
//!
//! It does not parse or reinterpret install commands. All flags and commands
//! reach the CLI's normal validation and dispatch paths.

use std::env;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();
    let bin_name = args.get(0).cloned().unwrap_or_else(|| "jhol".to_string());

    // Always delegate parsed arguments to the CLI. The legacy JSON daemon
    // request dropped lockfile, backend, and mode flags, so it could not safely
    // preserve `ci` or other install semantics.
    let self_path = env::current_exe().ok();
    let real_jhol = self_path
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.join("jhol")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| Path::new("jhol").to_path_buf());

    let _ = Command::new(&real_jhol).args(&args[1..]).exec();

    // If exec fails (shouldn't happen), fall back to PATH
    let err = Command::new("jhol").args(&args[1..]).exec();

    // If we get here, exec failed
    eprintln!("{}: failed to execute jhol: {}", bin_name, err);
    std::process::exit(1);
}
