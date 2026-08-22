//! jhol-fast: zero-binary-startup trampoline.
//!
//! A tiny binary (~8KB) that checks if the jhol daemon is running.
//! If yes: connects to the daemon socket, sends the request, prints the response.
//! If no: execs the real `jhol` binary.
//!
//! Startup time: <200µs (no Rust runtime, no clap, no allocator init).
//! Compared to full jhol: ~15ms startup.

use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();
    let bin_name = args.get(0).cloned().unwrap_or_else(|| "jhol".to_string());

    // Determine daemon socket path
    let socket_path = if let Ok(v) = env::var("JHOL_DAEMON_SOCKET") {
        Path::new(&v).to_path_buf()
    } else {
        std::env::temp_dir().join("jhol-install.sock")
    };

    // Check if daemon is running and socket is responsive
    // by attempting a quick connect
    let daemon_alive = match UnixStream::connect(&socket_path) {
        Ok(mut stream) => {
            // Send a simple ping: empty install request
            let ping = br#"{"cwd":".","packages":[],"quiet":true}"#;
            let _ = stream.write_all(ping);
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let mut resp = [0u8; 128];
            let n = stream.read(&mut resp).unwrap_or(0);
            n > 0 && resp[..n].windows(4).any(|w| w == b"true")
        }
        Err(_) => false,
    };

    if daemon_alive && args.len() > 1 {
        // Forward to daemon
        let subcommand = &args[1];
        match subcommand as &str {
            "install" | "add" | "ci" => {
                forward_to_daemon(&socket_path, &args);
                return;
            }
            _ => {} // fall through to real jhol for other commands
        }
    }

    // No daemon or non-install command: exec real jhol
    // Look for jhol in same directory as this binary, then in PATH
    let self_path = env::current_exe().ok();
    let real_jhol = self_path
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.join("jhol")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| Path::new("jhol").to_path_buf());

    let err = Command::new(&real_jhol).args(&args[1..]).exec();

    // If exec fails (shouldn't happen), fall back to PATH
    let err = Command::new("jhol").args(&args[1..]).exec();

    // If we get here, exec failed
    eprintln!("{}: failed to execute jhol: {}", bin_name, err);
    std::process::exit(1);
}

fn forward_to_daemon(socket_path: &Path, args: &[String]) {
    // Build JSON request from CLI args
    let cwd = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let packages: Vec<String> = args[2..]
        .iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect();

    let quiet = args.iter().any(|a| a == "-q" || a == "--quiet");
    let no_scripts = args.iter().any(|a| a == "--no-scripts");

    let req = serde_json::json!({
        "cwd": cwd,
        "packages": packages,
        "quiet": quiet,
        "noScripts": no_scripts,
    });

    match UnixStream::connect(socket_path) {
        Ok(mut stream) => {
            let body = req.to_string();
            if let Err(e) = stream.write_all(body.as_bytes()) {
                eprintln!("daemon write failed: {}", e);
                std::process::exit(1);
            }
            let _ = stream.shutdown(std::net::Shutdown::Write);

            let mut resp = String::new();
            if let Err(e) = stream.read_to_string(&mut resp) {
                eprintln!("daemon read failed: {}", e);
                std::process::exit(1);
            }

            let parsed: serde_json::Value = serde_json::from_str(&resp).unwrap_or_default();
            if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                std::process::exit(0);
            } else {
                let err = parsed
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("daemon install failed");
                eprintln!("{}", err);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("daemon unavailable: {}", e);
            std::process::exit(1);
        }
    }
}
