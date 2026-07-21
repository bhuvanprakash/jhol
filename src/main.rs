//! Thin CLI layer: parse args, styled output, and call into jhol-core.
//! Crash-proof: panic caught and reported; all errors return Result.

mod tui;

use clap::{Arg, ArgAction, Command};
use clap_complete;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::IsTerminal;
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

// ── colour helpers (no-op when stdout isn't a TTY) ────────────────────────────

fn use_color() -> bool {
    std::io::stdout().is_terminal() && env::var("NO_COLOR").unwrap_or_default().is_empty()
}

fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

fn success(msg: &str) {
    if use_color() {
        println!("{}", msg.green());
    } else {
        println!("{}", msg);
    }
}

fn error(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.red());
    } else {
        eprintln!("{}", msg);
    }
}

#[allow(dead_code)]
fn warning(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.yellow());
    } else {
        eprintln!("{}", msg);
    }
}

fn info(msg: &str) {
    if use_color() {
        println!("{}", msg.cyan());
    } else {
        println!("{}", msg);
    }
}

fn dim(msg: &str) {
    if use_color() {
        println!("{}", msg.dimmed());
    } else {
        println!("{}", msg);
    }
}

#[allow(dead_code)]
fn dim_err(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.dimmed());
    } else {
        eprintln!("{}", msg);
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let t: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}

// ── Progress & install display ─────────────────────────────────────────────────

/// Animated spinner styles used across all commands.
fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .tick_strings(&["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"])
        .template("{spinner:.magenta.bold} {msg}")
        .unwrap()
}

fn bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("  {bar:38.cyan/dim}  {pos:>3}/{len:3}  {msg:.dim}")
        .unwrap()
        .progress_chars("▰▰▱")
}

/// Live install progress driven by jhol-core's lock-free atomic counters.
///
/// The install runs on a background thread; this renders a real-time view
/// (a progress bar + per-stage counts + throughput) from `progress_snapshot()`
/// on the main thread until the install completes. Captivating, zero-cost to
/// the install itself (it only reads atomics).
struct InstallProgress {
    mp: MultiProgress,
    bar: ProgressBar,
    detail: ProgressBar,
    enabled: bool,
}

impl InstallProgress {
    fn new(pkg_count: usize, quiet: bool, json_out: bool) -> Self {
        let enabled = !quiet && !json_out && use_color() && is_tty();
        let mp = MultiProgress::new();
        let (bar, detail) = if enabled {
            let bar = mp.add(ProgressBar::new(pkg_count.max(1) as u64));
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(
                        "  {spinner:.magenta.bold} {bar:34.magenta/dim} {pos:>3}/{len:<3} {msg}",
                    )
                    .unwrap()
                    .tick_strings(&["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"])
                    .progress_chars("█▓░"),
            );
            bar.enable_steady_tick(Duration::from_millis(80));
            let detail = mp.add(ProgressBar::new_spinner());
            detail.set_style(
                ProgressStyle::default_spinner()
                    .template("  {msg}")
                    .unwrap(),
            );
            (bar, detail)
        } else {
            (ProgressBar::hidden(), ProgressBar::hidden())
        };
        Self {
            mp,
            bar,
            detail,
            enabled,
        }
    }

    /// Refresh the bar + detail line from a live progress snapshot.
    fn render(&self, snap: &jhol_core::ProgressSnapshot) {
        if !self.enabled {
            return;
        }
        let total = snap.total.max(1);
        self.bar.set_length(total);
        self.bar.set_position(snap.linked.min(total));

        // Stage message: show the busiest active stage.
        let phase = if snap.linked >= total {
            "finishing"
        } else if snap.downloaded > snap.extracted {
            "extracting"
        } else if snap.from_network + snap.from_cache > 0 {
            "linking"
        } else {
            "resolving"
        };
        self.bar.set_message(format!(
            "{}{}",
            phase.to_string().bright_magenta(),
            if snap.bytes_downloaded > 0 {
                format!("  {}", snap.downloaded_label())
                    .dimmed()
                    .to_string()
            } else {
                String::new()
            }
        ));

        // Detail line: cache vs network split.
        let detail = format!(
            "{} from cache  ·  {} from network{}",
            snap.from_cache.to_string().green(),
            snap.from_network.to_string().cyan(),
            if snap.failed > 0 {
                format!("  ·  {} failed", snap.failed).red().to_string()
            } else {
                String::new()
            },
        );
        self.detail.set_message(detail);
    }

    fn finish(self) {
        if self.enabled {
            self.bar.finish_and_clear();
            self.detail.finish_and_clear();
        }
    }
}

/// Print the final install summary line using accurate per-install metrics.
fn print_install_summary(pkg_count: usize, elapsed: Duration, quiet: bool, json_out: bool) {
    if quiet || json_out {
        return;
    }
    let ms = elapsed.as_millis();
    let speed = match ms {
        0..=9 => format!("{}⚡ instant{}", "\x1b[1;36m", "\x1b[0m"),
        10..=99 => format!("{}⚡ {}ms{}", "\x1b[1;36m", ms, "\x1b[0m"),
        100..=999 => format!("{}{}ms{}", "\x1b[36m", ms, "\x1b[0m"),
        _ => format!("{}{:.2}s{}", "\x1b[33m", elapsed.as_secs_f64(), "\x1b[0m"),
    };

    if use_color() {
        let pkg_word = if pkg_count == 1 {
            "package"
        } else {
            "packages"
        };
        println!();
        println!(
            "  {}  {} {}  {}",
            "✓".green().bold(),
            format!("{}", pkg_count).white().bold(),
            pkg_word.dimmed(),
            speed,
        );

        // Accurate cache hit rate from per-install progress counters.
        let snap = jhol_core::progress_snapshot();
        let done = snap.from_cache + snap.from_network;
        if done > 0 {
            let rate = snap.cache_rate();
            let rate_str = if rate >= 99.9 {
                format!("{:.0}%", rate).green().bold().to_string()
            } else if rate >= 50.0 {
                format!("{:.0}%", rate).yellow().to_string()
            } else {
                format!("{:.0}%", rate).cyan().to_string()
            };
            let mut detail = format!(
                "  {}  cache {} hit  •  {} cached  •  {} downloaded",
                "│".dimmed(),
                rate_str,
                snap.from_cache.to_string().green(),
                snap.from_network.to_string().cyan(),
            );
            if snap.bytes_downloaded > 0 {
                detail.push_str(&format!("  •  {}", snap.downloaded_label().dimmed()));
            }
            println!("{}", detail);
        }
        println!();
    } else {
        let ms_or_s = if ms < 1000 {
            format!("{}ms", ms)
        } else {
            format!("{:.2}s", elapsed.as_secs_f64())
        };
        println!("Added {} packages in {}", pkg_count, ms_or_s);
    }
}

/// Print a friendly, actionable error with consistent formatting.
fn print_error(cmd: &str, msg: &str) {
    if use_color() {
        eprintln!();
        eprintln!("  {} {}", "✗".red().bold(), msg.red());
        if !cmd.is_empty() {
            eprintln!("  {} command: {}", "│".dimmed(), cmd.dimmed());
        }
        eprintln!(
            "  {} Run {} for details",
            "╰".dimmed(),
            "jhol doctor --explain".cyan()
        );
        eprintln!();
    } else {
        eprintln!("Error: {}", msg);
        if !cmd.is_empty() {
            eprintln!("Command: {}", cmd);
        }
    }
}

/// Run a long-running task; in quiet mode show a spinner until done.
#[allow(dead_code)]
fn run_with_spinner<F>(message: &str, quiet: bool, f: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    if !quiet {
        return f();
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = f();
        let _ = tx.send(result);
    });
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(spinner_style());
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(Duration::from_millis(80));
    let mut elapsed = Duration::ZERO;
    let timeout = Duration::from_secs(600);
    let tick = Duration::from_millis(80);
    loop {
        match rx.try_recv() {
            Ok(res) => {
                spinner.finish_and_clear();
                return res;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                spinner.finish_and_clear();
                return Err("Operation failed.".to_string());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if elapsed >= timeout {
            spinner.finish_and_clear();
            return Err("Operation timed out.".to_string());
        }
        thread::sleep(tick);
        elapsed += tick;
    }
}

#[cfg(unix)]
fn daemon_socket_path() -> std::path::PathBuf {
    if let Ok(v) = env::var("JHOL_DAEMON_SOCKET") {
        return std::path::PathBuf::from(v);
    }
    std::env::temp_dir().join("jhol-install.sock")
}

#[cfg(unix)]
fn daemon_serve() -> Result<(), String> {
    let socket_path = daemon_socket_path();
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path).map_err(|e| {
        format!(
            "failed to bind daemon socket {}: {}",
            socket_path.display(),
            e
        )
    })?;

    // Set socket permissions to 0o600 (owner only) to prevent unauthorized access
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600));
    }

    info(&format!(
        "JHOL daemon listening on {}",
        socket_path.display()
    ));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                error(&format!("daemon accept failed: {}", e));
                continue;
            }
        };
        // Hand off to the shared handler
        let _ = daemon_handle_one(stream);
    }

    Ok(())
}
#[cfg(unix)]
/// Try to install via daemon. On non-Unix, always returns error.
fn daemon_try_install(
    _cwd: &Path,
    _packages: &[String],
    _opts: &jhol_core::InstallOptions,
    _auto_attach: bool,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        install_via_daemon(_cwd, _packages, _opts, _auto_attach)
    }
    #[cfg(not(unix))]
    {
        Err("--daemon install is only supported on Unix".to_string())
    }
}

/// Direct install (non-daemon path) with progress reporting.
fn direct_install(
    _cwd: &Path,
    specs: &[String],
    opts: &jhol_core::InstallOptions,
    pkg_count: usize,
    quiet: bool,
    json_out: bool,
) -> Result<(), String> {
    let progress = InstallProgress::new(pkg_count, quiet, json_out);
    let specs_owned: Vec<String> = specs.to_vec();
    let opts_owned = opts.clone();
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let worker = thread::spawn(move || {
        let refs2: Vec<&str> = specs_owned.iter().map(|s| s.as_str()).collect();
        let r = jhol_core::install_package(&refs2, &opts_owned)
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    let install_result = loop {
        match rx.recv_timeout(Duration::from_millis(70)) {
            Ok(r) => break r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let snap = jhol_core::progress_snapshot();
                progress.render(&snap);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("install worker terminated unexpectedly".to_string());
            }
        }
    };
    let _ = worker.join();
    progress.finish();
    // Write install markers for zero-binary-startup fast path
    if install_result.is_ok() {
        if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
            jhol_core::daemon_cache::write_nm_fingerprint(Path::new("."), hash);
            jhol_core::daemon_cache::store_last_hash(hash);
            jhol_core::daemon_cache::write_status(hash, Path::new("."));
        }
    }
    install_result
}

fn install_via_daemon(
    cwd: &Path,
    packages: &[String],
    opts: &jhol_core::InstallOptions,
    auto_attach: bool,
) -> Result<(), String> {
    let socket_path = daemon_socket_path();

    // Try to connect
    let stream_result = UnixStream::connect(&socket_path);
    if stream_result.is_err() {
        if auto_attach {
            // Safety gate: auto-attach only connects to existing socket.
            // Don't start daemon (avoids capsule write race). Fall back.
            return Err("no daemon socket found; falling back to direct install".to_string());
        }
        if !opts.quiet {
            info("Daemon not running — auto-starting...");
        }
        // Spawn daemon in background
        let socket_path_clone = socket_path.clone();
        let _ = std::thread::spawn(move || {
            // Remove stale socket if any
            let _ = fs::remove_file(&socket_path_clone);
            // Run daemon in a new thread (this blocks on incoming connections)
            let listener = std::os::unix::net::UnixListener::bind(&socket_path_clone);
            if let Ok(listener) = listener {
                // Set 0600 permissions
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&socket_path_clone, fs::Permissions::from_mode(0o600));
                }
                for stream in listener.incoming() {
                    if let Ok(s) = stream {
                        let _ = handle_daemon_request(s);
                    }
                }
            }
        });
        // Give daemon a moment to start
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Connect (retry once)
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("daemon unavailable at {}: {}", socket_path.display(), e))?;

    let req = serde_json::json!({
        "cwd": cwd.display().to_string(),
        "packages": packages,
        "quiet": opts.quiet,
        "noCache": opts.no_cache,
        "offline": opts.offline,
        "strictLockfile": opts.strict_lockfile,
        "strictPeerDeps": opts.strict_peer_deps,
        "fromLockfile": opts.from_lockfile,
        "lockfileOnly": opts.lockfile_only,
        "nativeOnly": opts.native_only,
        "noScripts": opts.no_scripts,
    });
    stream
        .write_all(req.to_string().as_bytes())
        .map_err(|e| format!("daemon write failed: {}", e))?;
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut resp = String::new();
    stream
        .read_to_string(&mut resp)
        .map_err(|e| format!("daemon read failed: {}", e))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("invalid daemon response: {}", e))?;
    if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        Ok(())
    } else {
        Err(parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("daemon install failed")
            .to_string())
    }
}

/// Handle a single daemon request (extracted for reuse by auto-start).
#[cfg(unix)]
fn handle_daemon_request(stream: std::os::unix::net::UnixStream) -> Result<(), String> {
    daemon_handle_one(stream)
}

/// Handle one daemon connection (the actual logic, extracted from daemon_serve).
#[cfg(unix)]
fn daemon_handle_one(mut stream: std::os::unix::net::UnixStream) -> Result<(), String> {
    use std::io::Read;
    use std::io::Write;
    const MAX_DAEMON_REQUEST_BYTES: usize = 64 * 1024;

    let req_buf: String = {
        let mut buf = Vec::with_capacity(MAX_DAEMON_REQUEST_BYTES);
        let mut tmp = [0u8; 4096];
        let mut too_large = false;
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len() + n > MAX_DAEMON_REQUEST_BYTES {
                        too_large = true;
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                Err(_) => {
                    too_large = true;
                    break;
                }
            }
        }
        if too_large {
            let _ = stream.write_all(br#"{"ok":false,"error":"request too large or invalid"}"#);
            return Ok(());
        }
        match String::from_utf8(buf) {
            Ok(s) => s,
            Err(_) => {
                let _ = stream.write_all(br#"{"ok":false,"error":"invalid utf8"}"#);
                return Ok(());
            }
        }
    };

    let req: serde_json::Value = match serde_json::from_str(&req_buf) {
        Ok(v) => v,
        Err(e) => {
            let payload = serde_json::json!({"ok": false, "error": format!("invalid json: {}", e)});
            let _ = stream.write_all(payload.to_string().as_bytes());
            return Ok(());
        }
    };

    // SNR query: read-only lookup in JMR for predicted resolution.
    if let Some(action) = req.get("action").and_then(|v| v.as_str()) {
        if action == "snr_query" || action == "snr" {
            let fp = req.get("fingerprint").and_then(|v| v.as_u64()).unwrap_or(0);
            let edit = req.get("edit").and_then(|v| v.as_str()).unwrap_or("");
            if fp == 0 || edit.is_empty() {
                let payload = serde_json::json!({"ok": false, "error": "snr_query requires fingerprint and edit"});
                let _ = stream.write_all(payload.to_string().as_bytes());
                return Ok(());
            }
            let result = jhol_core::resolution_mirror::snr_get(fp, edit);
            let payload = match result {
                Some(entries) => serde_json::json!({"ok": true, "hit": true, "entries": entries.len()}),
                None => serde_json::json!({"ok": true, "hit": false}),
            };
            let _ = stream.write_all(payload.to_string().as_bytes());
            return Ok(());
        }
    }

    let cwd = req.get("cwd").and_then(|v| v.as_str()).unwrap_or(".").to_string();
    if cwd.contains('\0') || cwd.len() > 4096 {
        let payload = serde_json::json!({"ok": false, "error": "invalid cwd"});
        let _ = stream.write_all(payload.to_string().as_bytes());
        return Ok(());
    }

    let packages: Vec<String> = req.get("packages")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if packages.len() > 1000 {
        let payload = serde_json::json!({"ok": false, "error": "too many packages"});
        let _ = stream.write_all(payload.to_string().as_bytes());
        return Ok(());
    }

    let quiet = req.get("quiet").and_then(|v| v.as_bool()).unwrap_or(false);
    let no_cache = req.get("noCache").and_then(|v| v.as_bool()).unwrap_or(false);
    let offline = req.get("offline").and_then(|v| v.as_bool()).unwrap_or(false);
    let strict_lockfile = req.get("strictLockfile").and_then(|v| v.as_bool()).unwrap_or(false);
    let strict_peer_deps = req.get("strictPeerDeps").and_then(|v| v.as_bool()).unwrap_or(false);
    let from_lockfile = req.get("fromLockfile").and_then(|v| v.as_bool()).unwrap_or(false);
    let lockfile_only = req.get("lockfileOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let native_only = req.get("nativeOnly").and_then(|v| v.as_bool()).unwrap_or(true);
    let no_scripts = req.get("noScripts").and_then(|v| v.as_bool()).unwrap_or(true);

    let prev_cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        std::env::set_current_dir(&cwd).map_err(|e| format!("chdir {}: {}", cwd, e))?;

        // Helper: write fingerprint + status after any successful install.
        let write_install_markers = || {
            if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
                jhol_core::daemon_cache::write_nm_fingerprint(Path::new("."), hash);
                jhol_core::daemon_cache::store_last_hash(hash);
                jhol_core::daemon_cache::write_status(hash, Path::new("."));
            }
        };

        // Daemon fast-path: if node_modules/.nm_fingerprint matches last hash, skip everything.
        if jhol_core::daemon_cache::fast_nm_check(Path::new(".")) {
            return Ok(());
        }
        // Fallback: compute hash from disk and check fingerprint
        if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
            if jhol_core::daemon_cache::nm_is_fresh(Path::new("."), hash) {
                jhol_core::daemon_cache::store_last_hash(hash);
                jhol_core::daemon_cache::write_status(hash, Path::new("."));
                return Ok(());
            }
        }

        // Daemon fast-path: if resolved graph cached and fingerprint matches,
        // skip resolution entirely — use live capsule view from memory.
        let cap_path = jhol_core::capsule::capsule_path(Path::new("."));
        if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
            // Check live in-memory capsule first (avoids mmap + verify)
            if let Some(view) = jhol_core::daemon_cache::get_cached_capsule(hash) {
                let r = jhol_core::capsule::install_from_capsule(&view, Path::new("."))
                    .map_err(|e| e.to_string());
                if r.is_ok() { write_install_markers(); }
                return r;
            }
            // Fallback: check disk capsule
            if let Some(_cached) = jhol_core::daemon_cache::get_resolved_graph(hash) {
                if cap_path.exists() {
                    if let Ok(view) = jhol_core::capsule::read_capsule(&cap_path) {
                        if view.manifest_hash() == hash {
                            let result = jhol_core::capsule::install_from_capsule(&view, Path::new("."))
                                .map_err(|e| e.to_string());
                            if result.is_ok() {
                                jhol_core::daemon_cache::store_capsule_view(hash, view);
                                write_install_markers();
                            }
                            return result;
                        }
                    }
                }
            }
        }

        let refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
        let opts = jhol_core::InstallOptions {
            no_cache, quiet,
            backend: jhol_core::resolve_backend(None),
            lockfile_only, offline, strict_lockfile, strict_peer_deps,
            from_lockfile, native_only, no_scripts,
            script_allowlist: None,
            resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
            overrides: None,
            telemetry: false,
        };
        jhol_core::set_daemon_mode(true);
        jhol_core::set_transitive_depth(2);
        let install_result = jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string());
        // SNR speculation: after successful install, pre-compute neighbor resolutions
        if install_result.is_ok() && !packages.is_empty() {
            if let Some(deps) = jhol_core::lockfile::read_package_json_deps(Path::new("package.json")) {
                let fp = jhol_core::resolution_mirror::fingerprint_deps(&deps);
                jhol_core::daemon_cache::speculate_neighborhoods(Path::new("."), &deps, fp);
            }
        }
        // CGA capsule write after successful install
        if install_result.is_ok() {
            let cga_gate = std::env::var("JHOL_CGA")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(true);
            if cga_gate {
                let _ = jhol_core::capsule::write_cga_capsule_from_project(Path::new("."));
            }
            // Write .nm_fingerprint so next request returns instantly
            // (also written by write_capsule_from_project in capsule.rs)
            if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
                jhol_core::daemon_cache::write_nm_fingerprint(Path::new("."), hash);
                jhol_core::daemon_cache::store_last_hash(hash);
                jhol_core::daemon_cache::write_status(hash, Path::new("."));
            }
        }
        install_result
    })();
    let _ = std::env::set_current_dir(prev_cwd);

    let payload = match result {
        Ok(_) => {
            // Store resolved graph
            let cap_path = jhol_core::capsule::capsule_path(Path::new("."));
            if cap_path.exists() {
                if let Ok(view) = jhol_core::capsule::read_capsule(&cap_path) {
                    let hash = view.manifest_hash();
                    // Store live capsule view for next request (avoids mmap)
                    jhol_core::daemon_cache::store_capsule_view(hash, view);
                    // Pin capsule + store pages in kernel page cache for next CLI
                    jhol_core::daemon_cache::repin_install_artifacts(Path::new("."));
                    if let Some(tree) = jhol_core::lockfile::read_resolved_from_dir(Path::new(".")) {
                        let mut resolved = std::collections::HashMap::new();
                        for (spec, _version) in &tree {
                            if let Some(name) = spec.split('@').next() {
                                let entry = jhol_core::lockfile_write::ResolvedPackage {
                                    version: _version.clone(),
                                    resolved: String::new(),
                                    integrity: None,
                                    dependencies: std::collections::HashMap::new(),
                                    peer_dependencies: std::collections::HashMap::new(),
                                    peer_dependencies_meta: std::collections::HashMap::new(),
                                };
                                resolved.insert(spec.clone(), entry);
                            }
                        }
                        jhol_core::daemon_cache::store_resolved_graph(hash, resolved);
                    }
                }
            }
            // Pre-warm CAS + predictive prefetch in background
            let packages = packages.clone();
            let cwd = cwd.clone();
            std::thread::spawn(move || {
                let lock_path = std::path::Path::new(&cwd).join("package-lock.json");
                if let Ok(bytes) = std::fs::read(&lock_path) {
                    if let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        if let Some(pkgs) = lock.get("packages").and_then(|p| p.as_object()) {
                            // Soul Path predictive prefetch from installed graph
                            let all_pkgs: Vec<String> = pkgs
                                .keys()
                                .filter(|k| !k.is_empty())
                                .map(|k| k.strip_prefix("node_modules/").unwrap_or(k).to_string())
                                .collect();
                            if all_pkgs.len() >= 2 {
                                jhol_core::soul_path::predictive_prefetch(&all_pkgs, 3);
                            }
                            // Pre-warm CAS in background
                            for (key, pkg) in pkgs {
                                if let Some(resolved) = pkg.get("resolved").and_then(|r| r.as_str()) {
                                    let name = key.strip_prefix("node_modules/").unwrap_or(key);
                                    let version = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("");
                                    jhol_core::registry::download_tarball_to_store(
                                        resolved,
                                        &std::path::PathBuf::from(jhol_core::utils::get_cache_dir()),
                                        &format!("{}@{}", name, version),
                                        None,
                                        None,
                                    ).ok();
                                }
                            }
                        }
                    }
                }
            });
            serde_json::json!({"ok": true})
        }
        Err(e) => serde_json::json!({"ok": false, "error": e}),
    };
    let _ = stream.write_all(payload.to_string().as_bytes());
    Ok(())
}

fn install_globally() -> Result<(), String> {
    let install_path = if cfg!(target_os = "windows") {
        format!(
            "{}\\jhol.exe",
            env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Program Files".to_string())
        )
    } else {
        "/usr/local/bin/jhol".to_string()
    };

    if Path::new(&install_path).exists() {
        info(&format!("Jhol is already installed at {}", install_path));
        return Ok(());
    }

    let exe_path = env::current_exe().map_err(|e| e.to_string())?;
    info(&format!("Installing Jhol at {} …", install_path));
    fs::copy(&exe_path, &install_path).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&install_path)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&install_path, perms).map_err(|e| e.to_string())?;
    }

    success("Jhol installed. You can run `jhol` from anywhere.");
    Ok(())
}

/// Build the CLI command tree (used for completion generation).
fn build_completions_cli() -> Command {
    Command::new("jhol")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Bhuvan Prakash <bhuvanstark6@gmail.com>")
        .about("Fast, offline-friendly package manager — cache first, Bun/npm backend")
        .subcommand(Command::new("install").about("Install packages"))
        .subcommand(Command::new("ci").about("Clean, deterministic install"))
        .subcommand(Command::new("add").about("Add packages to dependencies"))
        .subcommand(Command::new("remove").about("Remove packages"))
        .subcommand(Command::new("uninstall").about("Remove a package"))
        .subcommand(Command::new("update").about("Update packages"))
        .subcommand(Command::new("doctor").about("Check and fix outdated dependencies"))
        .subcommand(Command::new("audit").about("Check for known vulnerabilities"))
        .subcommand(Command::new("run").about("Run a script from package.json or a JS/TS file"))
        .subcommand(Command::new("eval").about("Evaluate JavaScript/TypeScript code inline"))
        .subcommand(Command::new("exec").about("Run a binary from node_modules/.bin"))
        .subcommand(Command::new("build").about("Bundle JS/TS/CSS for browser, Node.js, or Jhol"))
        .subcommand(Command::new("why").about("Explain why a package is installed"))
        .subcommand(Command::new("outdated").about("Show outdated dependencies"))
        .subcommand(Command::new("init").about("Initialize a new package.json"))
        .subcommand(Command::new("link").about("Link a local package directory"))
        .subcommand(Command::new("unlink").about("Remove a linked package"))
        .subcommand(Command::new("prefetch").about("Download lockfile dependencies into cache"))
        .subcommand(Command::new("cache").about("Manage local package cache"))
        .subcommand(Command::new("import-lock").about("Import/convert lockfile formats"))
        .subcommand(Command::new("sbom").about("Generate Software Bill of Materials"))
        .subcommand(Command::new("jagr").about("Run the JAGR three-layer resolver analysis"))
        .subcommand(Command::new("audit-jagr").about("Save / load / diff JAGR analysis reports"))
        .subcommand(
            Command::new("jagr-impact").about("Simulate upgrading a package and report impact"),
        )
        .subcommand(Command::new("cdn").about("Print esm.sh URL for a package"))
        .subcommand(Command::new("gui").about("Open the jhol Terminal GUI"))
        .subcommand(Command::new("daemon").about("Run persistent install daemon"))
        .subcommand(Command::new("global-install").about("Install jhol binary to PATH"))
        .subcommand(
            Command::new("completions")
                .about("Generate shell completion scripts")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(["bash", "zsh", "fish", "powershell", "elvish"])
                        .help("Shell to generate completions for"),
                ),
        )
}

fn run() -> Result<(), String> {
    let show_splash = env::var("JHOL_NO_SPLASH").unwrap_or_default().is_empty()
        && std::io::stdout().is_terminal();
    if show_splash {
        let version = env!("CARGO_PKG_VERSION");
        if use_color() {
            // Gradient-style banner: top rows brighter, bottom rows dimmer
            println!(
                "{}",
                "  ┌─────────────────────────────────────┐".truecolor(120, 80, 200)
            );
            println!(
                "  {} {}  {}  {}",
                "│".truecolor(120, 80, 200),
                " jhol "
                    .truecolor(255, 255, 255)
                    .on_truecolor(100, 60, 180)
                    .bold(),
                format!("v{}", version).truecolor(180, 140, 255),
                "fast · offline · cache-first".truecolor(150, 150, 150),
            );
            println!(
                "  {} {}",
                "│".truecolor(120, 80, 200),
                "Powered by JAGR · Tropical Geometry · Sheaf · MaxSAT".truecolor(110, 110, 150),
            );
            println!(
                "{}",
                "  └─────────────────────────────────────┘".truecolor(120, 80, 200)
            );
            println!();
        } else {
            println!("  jhol v{} — fast, offline, cache-first", version);
            println!("  Powered by JAGR (Tropical + Sheaf + MaxSAT)");
            println!();
        }
    }

    fn read_trusted_dependencies(path: &Path) -> HashSet<String> {
        // Read trustedDependencies from the given path (package.json or .jholrc)
        let Ok(raw) = fs::read_to_string(path) else {
            return HashSet::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return HashSet::new();
        };
        v.get("trustedDependencies")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default()
    }

    let backend_arg = Arg::new("backend")
        .long("backend")
        .value_parser(["bun", "npm"])
        .help("Package manager backend (default: bun if available, else npm)");

    let matches = Command::new("jhol")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Bhuvan Prakash <bhuvanstark6@gmail.com>")
        .about("Fast, offline-friendly package manager — cache first, Bun/npm backend")
        .after_help(
            "Examples:\n  jhol install lodash\n  jhol install react react-dom\n  jhol install\n  jhol doctor --fix\n  jhol cache list",
        )
        .subcommand(
            Command::new("install")
                .about("Install packages (from args or package.json)")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .num_args(0..)
                        .help("Package(s) to install; omit to install from package.json"),
                )
                .arg(
                    Arg::new("no-cache")
                        .long("no-cache")
                        .action(ArgAction::SetTrue)
                        .help("Ignore cache and fetch from registry"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output; show spinner when busy"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("lockfile-only")
                        .long("lockfile-only")
                        .action(ArgAction::SetTrue)
                        .help("Only update lockfile, do not install to node_modules"),
                )
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Only use cache; fail if any package is missing (or set JHOL_OFFLINE=1)"),
                )
                .arg(
                    Arg::new("frozen")
                        .long("frozen-lockfile")
                        .visible_alias("frozen")
                        .action(ArgAction::SetTrue)
                        .help("Require lockfile and fail if out of sync with package.json"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages (from package.json workspaces)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output machine-readable JSON result"),
                )
                .arg(
                    Arg::new("native-only")
                        .long("native-only")
                        .action(ArgAction::SetTrue)
                        .help("Never use Bun/npm; fail if native install fails (default)"),
                )
                .arg(
                    Arg::new("fallback-backend")
                        .long("fallback-backend")
                        .action(ArgAction::SetTrue)
                        .help("On failure, fall back to Bun/npm for install"),
                )
                .arg(
                    Arg::new("no-scripts")
                        .long("no-scripts")
                        .action(ArgAction::SetTrue)
                        .help("Do not run lifecycle scripts in fallback backend installs (default true)"),
                )
                .arg(
                    Arg::new("scripts")
                        .long("scripts")
                        .action(ArgAction::SetTrue)
                        .help("Allow lifecycle scripts in fallback backend installs"),
                )
                .arg(
                    Arg::new("resolver")
                        .long("resolver")
                        .value_parser(["pubgrub-v2", "pubgrub", "jagr", "legacy"])
                        .help("Resolver strategy for lockfile/dependency resolution"),
                )
                .arg(
                    Arg::new("strict-peer-deps")
                        .long("strict-peer-deps")
                        .action(ArgAction::SetTrue)
                        .help("Fail install when root peer dependency constraints conflict"),
                )
                .arg(
                    Arg::new("daemon")
                        .long("daemon")
                        .action(ArgAction::SetTrue)
                        .help("Send install request to running jhol daemon (Unix)"),
                )
                .arg(
                    Arg::new("fetch-timeout")
                        .long("fetch-timeout")
                        .value_parser(clap::value_parser!(u64))
                        .help("Timeout in seconds for individual package fetch operations"),
                )
                .arg(
                    Arg::new("resolve-timeout")
                        .long("resolve-timeout")
                        .value_parser(clap::value_parser!(u64))
                        .help("Timeout in seconds for dependency resolution"),
                )
                .arg(
                    Arg::new("install-timeout")
                        .long("install-timeout")
                        .value_parser(clap::value_parser!(u64))
                        .help("Timeout in seconds for the entire install operation"),
                )
                .arg(
                    Arg::new("prefer")
                        .long("prefer")
                        .value_parser(["minimal", "optimize", "latest"])
                        .help("Prefer minimal versions (minimal), optimal deduplication (optimize), or latest (default)"),
                ),
        )
        .subcommand(
            Command::new("ci")
                .about("Clean, deterministic install (npm ci-style): requires lockfile")
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue)
                        .help("Only use cache; fail if any package is missing"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output machine-readable JSON result"),
                )
                .arg(
                    Arg::new("jagr-check")
                        .long("jagr-check")
                        .action(ArgAction::SetTrue)
                        .help("After ci succeeds, run JAGR three-layer analysis on the manifest to confirm satisfiability"),
                )
                .arg(backend_arg.clone()),
        )
        .subcommand(
            Command::new("doctor")
                .about("Check and fix outdated dependencies")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .action(ArgAction::SetTrue)
                        .help("Update outdated packages"),
                )
                .arg(
                    Arg::new("explain")
                        .long("explain")
                        .action(ArgAction::SetTrue)
                        .help("Explain lockfile/health/fallback diagnostics for this project"),
                )
                .arg(
                    Arg::new("jagr")
                        .long("jagr")
                        .action(ArgAction::SetTrue)
                        .help("Run JAGR three-layer resolver health check on this project"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output machine-readable JSON"),
                ),
        )
        .subcommand(
            Command::new("import-lock")
                .about("Import/convert lockfile formats (bun.lock, yarn.lock, pnpm-lock.yaml -> package-lock.json)")
                .arg(
                    Arg::new("from")
                        .long("from")
                        .value_parser(["auto", "bun", "npm", "yarn", "pnpm"])
                        .default_value("auto")
                        .help("Source lockfile format (auto, bun, npm, yarn, pnpm)"),
                ),
        )
        .subcommand(
            Command::new("global-install")
                .about("Install jhol binary to PATH (e.g. /usr/local/bin)"),
        )
        .subcommand(
            Command::new("daemon")
                .about("Run persistent install daemon (Unix)")
                .arg(
                    Arg::new("serve")
                        .long("serve")
                        .action(ArgAction::SetTrue)
                        .help("Run daemon server loop"),
                ),
        )
        .subcommand(
            Command::new("cache")
                .about("Manage local package cache")
                .subcommand(Command::new("list").about("List cached packages"))
                .subcommand(Command::new("size").about("Show cache size and tarball count"))
                .subcommand(
                    Command::new("prune")
                        .about("Remove unreferenced tarballs; optionally keep only N most recent")
                        .arg(
                            Arg::new("keep")
                                .long("keep")
                                .value_parser(clap::value_parser!(usize))
                                .help("Keep only N most recently used tarballs"),
                        ),
                )
                .subcommand(
                    Command::new("export")
                        .about("Export project deps from cache to directory (for offline)")
                        .arg(Arg::new("dir").required(true).help("Output directory")),
                )
                .subcommand(
                    Command::new("import")
                        .about("Import tarballs from directory into cache")
                        .arg(Arg::new("dir").required(true).help("Directory from jhol cache export")),
                )
                .subcommand(Command::new("clean").about("Remove all cached tarballs"))
                .subcommand(
                    Command::new("telemetry").about("Show native fallback telemetry summary"),
                )
                .subcommand(
                    Command::new("key")
                        .about("Print lockfile hash for CI cache key (same lockfile => same key)"),
                )
                .subcommand(
                    Command::new("mirror")
                        .about("Show JMR resolution mirror stats (entries, coverage, hits)")
                        .subcommand(Command::new("stats").about("Show mirror statistics"))
                        .subcommand(Command::new("prune").about("Remove expired entries from mirror"))
                        .subcommand(Command::new("clear").about("Clear all mirror entries"))
                        .subcommand(Command::new("lookup")
                            .about("Look up a package in the mirror")
                            .arg(Arg::new("package").required(true).help("Package name to look up (@scope/pkg@^range)"))
                        ),
                ),
        )
        .subcommand(
            Command::new("audit")
                .about("Check for known vulnerabilities")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .action(ArgAction::SetTrue)
                        .help("Apply fixes where possible"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                )
                .arg(backend_arg.clone())
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output raw audit JSON"),
                )
                .arg(
                    Arg::new("gate")
                        .long("gate")
                        .action(ArgAction::SetTrue)
                        .help("Fail with non-zero exit if vulnerabilities are found (CI gate)"),
                ),
        )
        .subcommand(
            Command::new("prefetch")
                .about("Download lockfile dependencies into cache (no node_modules). Use before install --offline.")
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                ),
        )
        .subcommand(
            Command::new("run")
                .about("Run a script from package.json or a JS/TS file with Jhol runtime")
                .arg(
                    Arg::new("script")
                        .required(true)
                        .help("Script name from package.json, or JS/TS file path"),
                )
                .arg(
                    Arg::new("file")
                        .long("file")
                        .short('f')
                        .action(ArgAction::SetTrue)
                        .help("Treat argument as a JS/TS file path (run with Jhol runtime)"),
                )
                .arg(
                    Arg::new("watch")
                        .long("watch")
                        .short('w')
                        .action(ArgAction::SetTrue)
                        .help("Watch mode: restart on file changes"),
                )
                .arg(
                    Arg::new("engine")
                        .long("engine")
                        .default_value("auto")
                        .value_parser(["auto", "quickjs", "js-engine"])
                        .help("Select script engine"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                ),
        )
        .subcommand(
            Command::new("eval")
                .about("Evaluate JavaScript/TypeScript code inline")
                .arg(
                    Arg::new("code")
                        .required(true)
                        .help("JavaScript/TypeScript code to evaluate"),
                )
                .arg(
                    Arg::new("type")
                        .long("type")
                        .value_parser(["js", "ts", "jsx", "tsx"])
                        .default_value("js")
                        .help("File type for inline evaluation"),
                ),
        )
        .subcommand(
            Command::new("build")
                .about("Bundle JavaScript/TypeScript/CSS for browser, Node.js, or Jhol")
                .arg(
                    Arg::new("entry")
                        .required(true)
                        .num_args(1..)
                        .help("Entry file(s) to bundle"),
                )
                .arg(
                    Arg::new("outdir")
                        .long("outdir")
                        .short('o')
                        .default_value("dist")
                        .help("Output directory"),
                )
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["esm", "cjs", "iife"])
                        .default_value("esm")
                        .help("Output format"),
                )
                .arg(
                    Arg::new("target")
                        .long("target")
                        .value_parser(["browser", "node", "jhol"])
                        .default_value("browser")
                        .help("Target platform"),
                )
                .arg(
                    Arg::new("minify")
                        .long("minify")
                        .short('m')
                        .action(ArgAction::SetTrue)
                        .help("Minify output"),
                )
                .arg(
                    Arg::new("watch")
                        .long("watch")
                        .short('w')
                        .action(ArgAction::SetTrue)
                        .help("Watch mode: rebuild on file changes"),
                )
                .arg(
                    Arg::new("sourcemap")
                        .long("sourcemap")
                        .value_parser(["none", "inline", "external", "both"])
                        .default_value("none")
                        .help("Source map generation"),
                )
                .arg(
                    Arg::new("external")
                        .long("external")
                        .num_args(0..)
                        .help("Modules to mark as external"),
                )
                .arg(
                    Arg::new("tree-shake")
                        .long("tree-shake")
                        .action(ArgAction::SetTrue)
                        .help("Enable tree shaking"),
                )
                .arg(
                    Arg::new("code-splitting")
                        .long("code-splitting")
                        .action(ArgAction::SetTrue)
                        .help("Enable code splitting"),
                ),
        )
        .subcommand(
            Command::new("exec")
                .visible_alias("x")
                .about("Run a binary from node_modules/.bin (no npx)")
                .arg(
                    Arg::new("binary")
                        .required(true)
                        .help("Binary name (e.g. eslint, tsc)"),
                )
                .arg(
                    Arg::new("args")
                        .num_args(0..)
                        .help("Arguments to pass to the binary"),
                ),
        )
        .subcommand(
            Command::new("cdn")
                .about("Print esm.sh URL for a package (optional: fetch to file)")
                .arg(
                    Arg::new("package")
                        .required(true)
                        .help("Package spec (e.g. lodash@4 or lodash)"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Fetch ESM bundle to this file"),
                ),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove a package (optionally from package.json)")
                .arg(Arg::new("package").required(true).help("Package name to remove"))
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .help("Remove from package.json dependencies"),
                ),
        )
        .subcommand(
            Command::new("update")
                .about("Update packages to latest and refresh lockfile")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .num_args(0..)
                        .help("Package(s) to update; omit for full lockfile refresh"),
                )
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .help("Write updated versions back to package.json"),
                ),
        )
        .subcommand(
            Command::new("why")
                .about("Explain why a package is installed (dependency path)")
                .arg(Arg::new("package").required(true).help("Package name"))
                .arg(
                    Arg::new("jagr")
                        .long("jagr")
                        .action(ArgAction::SetTrue)
                        .help("Use JAGR three-layer analysis to produce a richer explanation (Tropical + Sheaf Cohomology + Max-SMT)"),
                )
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .short('m')
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json to analyze (used with --jagr)"),
                ),
        )
        .subcommand(
            Command::new("outdated")
                .about("Show outdated dependencies with current, wanted, and latest versions")
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
                )
                .arg(
                    Arg::new("jagr")
                        .long("jagr")
                        .action(ArgAction::SetTrue)
                        .help("After listing outdated packages, also report JAGR topological impact (which upgrades would break SNF or raise β₁)"),
                ),
        )
        .subcommand(
            Command::new("init")
                .about("Initialize a new package.json in the current directory")
                .arg(
                    Arg::new("yes")
                        .short('y')
                        .long("yes")
                        .action(ArgAction::SetTrue)
                        .help("Accept defaults without prompting"),
                )
                .arg(
                    Arg::new("private")
                        .long("private")
                        .action(ArgAction::SetTrue)
                        .help("Mark package as private (not for publishing)"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Alias for install: add packages to dependencies")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .num_args(0..)
                        .help("Package(s) to add"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output"),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about("Alias for uninstall: remove packages from dependencies")
                .arg(Arg::new("package").required(true).help("Package name(s) to remove"))
                .arg(
                    Arg::new("save")
                        .long("save")
                        .action(ArgAction::SetTrue)
                        .help("Remove from package.json dependencies"),
                ),
        )
        .subcommand(
            Command::new("link")
                .about("Link a local package directory into node_modules")
                .arg(
                    Arg::new("package")
                        .required(false)
                        .help("Package directory to link (default: current directory)"),
                ),
        )
        .subcommand(
            Command::new("unlink")
                .about("Remove a linked package")
                .arg(Arg::new("package").required(true).help("Package name to unlink")),
        )
        .subcommand(
            Command::new("sbom")
                .about("Generate Software Bill of Materials")
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["cyclonedx", "simple"])
                        .default_value("cyclonedx")
                        .help("SBOM format"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Write to file (default: stdout)"),
                ),
        )
        .subcommand(
            Command::new("jagr")
                .about("Run the JAGR three-layer resolver analysis on the current project's package.json (Tropical + Sheaf Cohomology + Core-Guided MaxSAT)")
                .arg(
                    Arg::new("show-config")
                        .long("show-config")
                        .action(ArgAction::SetTrue)
                        .help("Print the merged JAGR config (.jholrc + env vars) and exit"),
                )
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .short('m')
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json manifest to analyze"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output the full JagrAnalysisReport as JSON"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Minimal output (strategy + selection only)"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .short('w')
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run the JAGR analysis in all workspace packages"),
                )
                .arg(
                    Arg::new("skip-layer2")
                        .long("skip-layer2")
                        .action(ArgAction::SetTrue)
                        .help("Skip Layer 2 (sheaf cohomology) and go directly to Layer 1"),
                )
                .arg(
                    Arg::new("skip-layer3")
                        .long("skip-layer3")
                        .action(ArgAction::SetTrue)
                        .help("Skip Layer 3 (tropical PTIME filter)"),
                )
                .arg(
                    Arg::new("run-all-layers")
                        .long("run-all-layers")
                        .action(ArgAction::SetTrue)
                        .help("Run all three layers even if an earlier layer already produced a definite answer (useful for benchmarking)"),
                )
                .arg(
                    Arg::new("graph")
                        .long("graph")
                        .value_parser(["dot", "mermaid"])
                        .help("Output the derivation/conflict graph in the given format (dot or mermaid)"),
                )
                .arg(
                    Arg::new("lockfile-only")
                        .long("lockfile-only")
                        .action(ArgAction::SetTrue)
                        .help("Resolve and write the lockfile, but don't install any packages (mirrors `jhol install --lockfile-only`)"),
                )
                .arg(
                    Arg::new("explain-each")
                        .long("explain-each")
                        .action(ArgAction::SetTrue)
                        .help("Show a per-package explanation chain (which layer picked each version and why)"),
                )
                .arg(
                    Arg::new("compare-resolvers")
                        .long("compare-resolvers")
                        .action(ArgAction::SetTrue)
                        .help("Run JAGR and the legacy resolvers (pubgrub, sat) on the same manifest and print a side-by-side time/selection comparison"),
                )
                .arg(
                    Arg::new("upgrade-plan")
                        .long("upgrade-plan")
                        .action(ArgAction::SetTrue)
                        .help("For each declared dep, run JAGR before/after a candidate version change and report topological impact (β₁, SNF, risk class)"),
                )
                .arg(
                    Arg::new("dry-run-upgrade")
                        .long("dry-run-upgrade")
                        .value_name("PKG@VERSION")
                        .action(ArgAction::Set)
                        .help("Simulate upgrading PKG to VERSION and report JAGR topological impact (no manifest is modified)"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .help("Write the analysis output to a file (default: stdout)"),
                ),
        )
        .subcommand(
            Command::new("audit-jagr")
                .about("Save / load / diff JAGR analysis reports for reproducibility and audit trails")
                .arg(
                    Arg::new("save")
                        .long("save")
                        .value_name("PATH")
                        .help("Run JAGR on the current manifest and save the report to PATH as JSON"),
                )
                .arg(
                    Arg::new("load")
                        .long("load")
                        .value_name("PATH")
                        .help("Load a JAGR report from PATH and print a summary"),
                )
                .arg(
                    Arg::new("diff")
                        .long("diff")
                        .value_name("OLD,NEW")
                        .help("Compare two JAGR audit JSON files (comma-separated paths) and print the diff"),
                )
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .short('m')
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json manifest to analyze (used with --save)"),
                ),
        )
        .subcommand(
            Command::new("betti-history")
                .about("Show the persistent Betti-number history of JAGR analyses for this project (set JHOL_BETTI_HISTORY=1 to record)")
                .arg(
                    Arg::new("file")
                        .long("file")
                        .value_name("PATH")
                        .default_value(".jagr_betti_history.jsonl")
                        .help("Path to the Betti history JSONL file"),
                )
                .arg(
                    Arg::new("last")
                        .long("last")
                        .value_name("N")
                        .help("Show only the last N entries (e.g. --last 5)"),
                ),
        )
        .subcommand(
            Command::new("jagr-impact")
                .about("Simulate upgrading a single package and report the JAGR topological impact (β₁, SNF, risk class). No manifest is modified.")
                .arg(
                    Arg::new("manifest")
                        .short('m')
                        .long("manifest")
                        .value_name("PATH")
                        .default_value("package.json")
                        .help("Path to the package.json manifest"),
                )
                .arg(
                    Arg::new("package")
                        .long("package")
                        .short('p')
                        .value_name("PKG")
                        .required_unless_present("target")
                        .help("Package name to simulate upgrading"),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .short('v')
                        .value_name("VER")
                        .required_unless_present("target")
                        .help("Target version to simulate"),
                )
                .arg(
                    Arg::new("target")
                        .long("target")
                        .value_name("PKG@VERSION")
                        .help("Single-value form: PKG@VERSION (e.g. left-pad@1.3.0)"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Output as JSON"),
                ),
        )
        .subcommand(
            Command::new("gui")
                .about("Open the jhol Terminal GUI — browse packages, view recent installs, navigate with arrow keys")
                .long_about(
                    "Launches the jhol Terminal GUI (TUI). \
                     The classic CLI is still available — this is purely additive.\n\n\
                     Keyboard shortcuts inside the GUI:\n  \
                     ↑↓ / j k   navigate\n  \
                     Tab / 1 2 3  switch panels\n  \
                     r / F5      refresh\n  \
                     q / Esc     quit\n  \
                     ?           help"
                ),
        )
        .subcommand(
            Command::new("completions")
                .about("Generate shell completion scripts")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(["bash", "zsh", "fish", "powershell", "elvish"])
                        .help("Shell to generate completions for (bash, zsh, fish, powershell, elvish)"),
                ),
        )
        .get_matches();

    // global-install (no cache needed)
    if let Some(("global-install", _)) = matches.subcommand() {
        install_globally().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // ── jhol gui ─ Terminal GUI ──────────────────────────────────────────────
    if let Some(("gui", _)) = matches.subcommand() {
        return tui::run_tui();
    }

    // ── jhol completions ─ Shell completion generation ───────────────────────
    if let Some(("completions", sub_m)) = matches.subcommand() {
        use clap_complete::{
            generate,
            shells::{Bash, Elvish, Fish, PowerShell, Zsh},
        };
        let shell = sub_m
            .get_one::<String>("shell")
            .map(|s| s.as_str())
            .unwrap_or("bash");
        // Build a fresh copy of the CLI for completions generation
        let mut cmd = build_completions_cli();
        let mut stdout = std::io::stdout();
        match shell {
            "bash" => {
                generate(Bash, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.bashrc:\n# eval \"$(jhol completions bash)\"");
            }
            "zsh" => {
                generate(Zsh, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.zshrc:\n# eval \"$(jhol completions zsh)\"");
            }
            "fish" => {
                generate(Fish, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.config/fish/config.fish:\n# jhol completions fish | source");
            }
            "powershell" => {
                generate(PowerShell, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to your PowerShell profile:\n# jhol completions powershell | Out-String | Invoke-Expression");
            }
            "elvish" => {
                generate(Elvish, &mut cmd, "jhol", &mut stdout);
                eprintln!("# Add to ~/.elvish/rc.elv:\n# eval (jhol completions elvish)");
            }
            _ => return Err(format!("Unknown shell: {}", shell)),
        }
        return Ok(());
    }

    if let Some(("daemon", sub_m)) = matches.subcommand() {
        if sub_m.get_flag("serve") {
            #[cfg(unix)]
            {
                daemon_serve()?;
                return Ok(());
            }
            #[cfg(not(unix))]
            {
                return Err("daemon mode is only supported on Unix".to_string());
            }
        }
        return Err("use `jhol daemon --serve` to run daemon".to_string());
    }

    // cache list | size | prune | export | import | clean
    if let Some(("cache", sub)) = matches.subcommand() {
        jhol_core::init_cache().map_err(|e| format!("Failed to initialize cache: {}", e))?;
        match sub.subcommand() {
            Some(("list", _)) => {
                let list = jhol_core::list_cached_packages()
                    .map_err(|e| format!("Failed to list cache: {}", e))?;
                let (bytes, _) = jhol_core::cache_size_bytes()
                    .map_err(|e| format!("Failed to get cache size: {}", e))?;
                if list.is_empty() {
                    dim("No cached packages.");
                } else {
                    info(&format!("Cached packages ({})", list.len()));
                    for name in list {
                        println!("  {}", name);
                    }
                    dim(&format!("Total size: {} MB", bytes / 1024 / 1024));
                }
            }
            Some(("size", _)) => {
                let (bytes, count) = jhol_core::cache_size_bytes()
                    .map_err(|e| format!("Failed to get cache size: {}", e))?;
                info(&format!(
                    "Cache: {} tarball(s), {} MB",
                    count,
                    bytes / 1024 / 1024
                ));
            }
            Some(("prune", sub_prune)) => {
                let keep = sub_prune.get_one::<usize>("keep").copied();
                let n = jhol_core::cache_prune(keep)
                    .map_err(|e| format!("Failed to prune cache: {}", e))?;
                success(&format!("Pruned {} tarball(s).", n));
            }
            Some(("export", sub_exp)) => {
                let dir = sub_exp
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap();
                let n = jhol_core::cache_export(std::path::Path::new(dir))
                    .map_err(|e| format!("Export failed: {}", e))?;
                success(&format!("Exported {} package(s) to {}.", n, dir));
            }
            Some(("import", sub_imp)) => {
                let dir = sub_imp
                    .get_one::<String>("dir")
                    .map(|s| s.as_str())
                    .unwrap();
                let n = jhol_core::cache_import(std::path::Path::new(dir))
                    .map_err(|e| format!("Import failed: {}", e))?;
                success(&format!("Imported {} package(s) from {}.", n, dir));
            }
            Some(("clean", _)) => {
                let n = jhol_core::cache_clean()
                    .map_err(|e| format!("Failed to clean cache: {}", e))?;
                success(&format!("Removed {} cached package(s).", n));
            }
            Some(("telemetry", _)) => {
                let v = jhol_core::read_fallback_telemetry();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
                );
            }
            Some(("key", _)) => {
                let hash = jhol_core::lockfile_content_hash(std::path::Path::new("."))
                    .unwrap_or_else(|| "none".to_string());
                println!("{}", hash);
            }
            Some(("mirror", sub)) => match sub.subcommand() {
                Some(("stats", _)) => {
                    let stats = jhol_core::resolution_mirror::global_mirror().stats();
                    info(&format!("JMR Resolution Mirror:"));
                    println!("  Total entries:    {}", stats.total_entries);
                    println!("  Fresh entries:    {}", stats.fresh_entries);
                    println!("  Expired entries:  {}", stats.expired_entries);
                    if stats.total_entries > 0 {
                        let pct = (stats.fresh_entries as f64 / stats.total_entries as f64) * 100.0;
                        println!("  Fresh ratio:      {:.1}%", pct);
                    }
                }
                Some(("prune", _)) => {
                    let n = jhol_core::resolution_mirror::global_mirror()
                        .prune()
                        .map_err(|e| format!("Failed to prune mirror: {}", e))?;
                    success(&format!("Pruned {} expired mirror entries.", n));
                }
                Some(("clear", _)) => {
                    jhol_core::resolution_mirror::global_mirror()
                        .clear()
                        .map_err(|e| format!("Failed to clear mirror: {}", e))?;
                    success("Cleared all mirror entries.");
                }
                Some(("lookup", sub_l)) => {
                    let pkg = sub_l
                        .get_one::<String>("package")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let (name, range) = if pkg.contains('@') && !pkg.starts_with('@') {
                        let parts: Vec<&str> = pkg.splitn(2, '@').collect();
                        (parts[0], if parts.len() > 1 { parts[1] } else { "*" })
                    } else if pkg.starts_with('@') {
                        let at_idx = pkg.rfind('@').unwrap_or(0);
                        if at_idx > 0 {
                            (&pkg[..at_idx], &pkg[at_idx + 1..])
                        } else {
                            (pkg, "*")
                        }
                    } else {
                        (pkg, "*")
                    };
                    match jhol_core::resolution_mirror::global_mirror().lookup(name, range) {
                        Some(entry) => {
                            info(&format!("Mirror entry for {}@{}", name, range));
                            println!("  Version:          {}", entry.resolved_version);
                            println!("  Dep signature:    {}", entry.dep_signature);
                            println!("  Dependencies:     {}", entry.dependencies.len());
                            println!(
                                "  Tarball URL:      {}",
                                entry.tarball_url.as_deref().unwrap_or("(none)")
                            );
                            println!(
                                "  Integrity:        {}",
                                entry.integrity.as_deref().unwrap_or("(none)")
                            );
                        }
                        None => dim(&format!("No mirror entry for {}@{}", name, range)),
                    }
                }
                _ => {
                    dim("Use `jhol cache mirror stats`, `jhol cache mirror prune`, `jhol cache mirror clear`, or `jhol cache mirror lookup <package>`.");
                }
            },
            _ => {
                dim("Use `jhol cache list`, `jhol cache size`, `jhol cache prune`, `jhol cache export <dir>`, `jhol cache import <dir>`, `jhol cache clean`, `jhol cache telemetry`, or `jhol cache key`.");
            }
        }
        return Ok(());
    }

    if let Some(("import-lock", sub_m)) = matches.subcommand() {
        let from = sub_m
            .get_one::<String>("from")
            .map(|s| s.as_str())
            .unwrap_or("auto");
        let msg = jhol_core::import_lockfile(from).map_err(|e| e.to_string())?;
        success(&msg);
        return Ok(());
    }

    if let Some(("run", sub_m)) = matches.subcommand() {
        let script_or_file = sub_m
            .get_one::<String>("script")
            .unwrap()
            .as_str()
            .to_string();
        let is_file = sub_m.get_flag("file");
        let watch = sub_m.get_flag("watch");
        let engine = sub_m
            .get_one::<String>("engine")
            .map(|s| s.as_str())
            .unwrap_or("auto");

        // If --file flag is set, run with Jhol runtime
        if is_file
            || std::path::Path::new(&script_or_file).exists()
                && (script_or_file.ends_with(".js")
                    || script_or_file.ends_with(".ts")
                    || script_or_file.ends_with(".jsx")
                    || script_or_file.ends_with(".tsx")
                    || script_or_file.ends_with(".mjs")
                    || script_or_file.ends_with(".cjs"))
        {
            if engine == "js-engine" {
                #[cfg(feature = "js-engine")]
                {
                    let mut backend =
                        jhol_core::js_engine_backend::js_engine_backend::JSEngineBackend::new();
                    match backend.run_file(&script_or_file) {
                        Ok(result) => {
                            println!("{}", result);
                            std::process::exit(0);
                        }
                        Err(e) => {
                            error(&format!("Runtime error: {}", e));
                            std::process::exit(1);
                        }
                    }
                }
                #[cfg(not(feature = "js-engine"))]
                {
                    return Err(
                        "JSEngine backend not enabled. Rebuild with --features js-engine"
                            .to_string(),
                    );
                }
            }
            #[cfg(feature = "js-engine")]
            {
                let engine = js_engine::JSEngine::new();
                let source = std::fs::read_to_string(&script_or_file)
                    .map_err(|e| format!("Failed to read {}: {}", script_or_file, e))?;
                match engine.run_script(&source, &script_or_file) {
                    Ok(_) => std::process::exit(0),
                    Err(e) => {
                        error(&format!("Runtime error: {}", e));
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(feature = "runtime")]
            {
                let path = std::path::Path::new(&script_or_file);
                if watch {
                    jhol_runtime::run_file_watch(path).map_err(|e| e.to_string())?;
                } else {
                    let code = jhol_runtime::run_file(path).map_err(|e| e.to_string())?;
                    std::process::exit(code);
                }
                return Ok(());
            }
            #[cfg(not(feature = "runtime"))]
            {
                return Err("Jhol runtime not enabled. Rebuild with --features runtime".to_string());
            }
        }

        // Existing behavior: run a package.json script
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let roots: Vec<std::path::PathBuf> = if sub_m.get_flag("all-workspaces") {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() {
                vec![cwd]
            } else {
                r.into_iter().map(|p| cwd.join(p)).collect()
            }
        } else {
            vec![cwd]
        };
        let mut last_code = 0i32;
        for root in &roots {
            let status =
                jhol_core::run_script(&script_or_file, root, None).map_err(|e| e.to_string())?;
            last_code = status.code().unwrap_or(1);
        }
        std::process::exit(last_code);
    }

    // jhol eval — evaluate inline JavaScript/TypeScript
    if let Some(("eval", sub_m)) = matches.subcommand() {
        let code = sub_m.get_one::<String>("code").unwrap();
        let file_type = sub_m
            .get_one::<String>("type")
            .map(|s| s.as_str())
            .unwrap_or("js");
        let filename = format!("<eval>.{}", file_type);

        #[cfg(feature = "js-engine")]
        {
            let engine = js_engine::JSEngine::new();
            match engine.run_script(code, &filename) {
                Ok(result) => {
                    println!("{}", result);
                    std::process::exit(0);
                }
                Err(e) => {
                    error(&format!("Runtime error: {}", e));
                    std::process::exit(1);
                }
            }
        }

        #[cfg(feature = "runtime")]
        {
            let exit_code =
                jhol_runtime::eval_code(code, Some(&filename)).map_err(|e| e.to_string())?;
            std::process::exit(exit_code);
        }
        #[cfg(not(feature = "runtime"))]
        {
            return Err("Jhol runtime not enabled. Rebuild with --features runtime".to_string());
        }
    }

    if let Some(("cdn", sub_m)) = matches.subcommand() {
        let spec = sub_m.get_one::<String>("package").unwrap().as_str();
        let (name, version) = if let Some(at) = spec.rfind('@') {
            if at > 0 {
                (&spec[..at], Some(&spec[at + 1..]))
            } else {
                (spec, None)
            }
        } else {
            (spec, None)
        };
        let url = jhol_core::esm_sh_url(name, version);
        println!("{}", url);
        if let Some(out_path) = sub_m.get_one::<String>("output") {
            if let Err(e) = jhol_core::fetch_esm_to_file(&url, std::path::Path::new(out_path)) {
                eprintln!("Fetch failed: {}", e);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if let Some(("exec", sub_m)) = matches.subcommand() {
        let binary = sub_m.get_one::<String>("binary").unwrap().as_str();
        let args: Vec<String> = sub_m
            .get_many::<String>("args")
            .map(|it| it.map(String::clone).collect())
            .unwrap_or_default();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let bin_path = jhol_core::find_binary_in_node_modules(binary, &cwd).ok_or_else(|| {
            format!(
                "Binary \"{}\" not found in node_modules/.bin. Run jhol install first.",
                binary
            )
        })?;
        let status = jhol_core::exec_binary(&bin_path, &args, &cwd).map_err(|e| e.to_string())?;
        std::process::exit(status.code().unwrap_or(1));
    }

    if let Some(("uninstall", sub_m)) = matches.subcommand() {
        let package = sub_m.get_one::<String>("package").unwrap();
        let save = sub_m.get_flag("save");
        jhol_core::uninstall(package, save).map_err(|e| e.to_string())?;
        success(&format!("Removed {}", package));
        return Ok(());
    }

    if let Some(("update", sub_m)) = matches.subcommand() {
        let save = sub_m.get_flag("save");
        let packages: Vec<String> = sub_m
            .get_many::<String>("package")
            .map(|it| it.map(|s| s.clone()).collect())
            .unwrap_or_default();
        if save {
            jhol_core::update_packages(&packages).map_err(|e| e.to_string())?;
        } else if packages.is_empty() {
            jhol_core::update_packages(&packages).map_err(|e| e.to_string())?;
        } else {
            return Err(
                "Use --save to update package.json when updating specific packages".to_string(),
            );
        }
        success("Update complete.");
        return Ok(());
    }

    if let Some(("why", sub_m)) = matches.subcommand() {
        let package = sub_m.get_one::<String>("package").unwrap();
        let use_jagr = sub_m.get_flag("jagr");
        if use_jagr {
            let manifest = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("manifest")
                    .cloned()
                    .unwrap_or_else(|| "package.json".to_string()),
            );
            let paths =
                jhol_core::jagr_why_package_deep(&manifest, package).map_err(|e| e.to_string())?;
            println!("{}", paths);
        } else {
            let paths = jhol_core::why_package(package).map_err(|e| e.to_string())?;
            for path in paths {
                println!("{}", path);
            }
        }
        return Ok(());
    }

    if let Some(("outdated", sub_m)) = matches.subcommand() {
        let quiet = sub_m.get_flag("quiet");
        let all_workspaces = sub_m.get_flag("all-workspaces");
        let with_jagr = sub_m.get_flag("jagr");
        if quiet {
            env::set_var("JHOL_QUIET", "1");
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let roots: Vec<std::path::PathBuf> = if all_workspaces {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() {
                vec![cwd]
            } else {
                r.into_iter().map(|p| cwd.join(p)).collect()
            }
        } else {
            vec![cwd]
        };
        for root in &roots {
            if roots.len() > 1 {
                info(&format!("Workspace: {}", root.display()));
            }
            jhol_core::check_dependencies(quiet, jhol_core::Backend::Npm)
                .map_err(|e| e.to_string())?;
        }

        // --jagr: After listing outdated packages, also report JAGR
        // topological impact (β₁/β₂/SNF status from JAGR's three-layer pipeline).
        if with_jagr {
            if use_color() {
                println!(
                    "\n{}",
                    "JAGR Topological Impact on Upgrades".bright_cyan().bold()
                );
            } else {
                println!("\nJAGR Topological Impact on Upgrades");
            }
            for root in &roots {
                let manifest = root.join("package.json");
                if !manifest.exists() {
                    info(&format!("No package.json in {}", root.display()));
                    continue;
                }
                match jhol_core::jagr_analyze_project_manifest(&manifest) {
                    Ok(report) => {
                        println!(
                            "  {:<14} β₀={}, β₁={}, β₂={}",
                            root.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                            report.betti_0,
                            report.betti_1,
                            report.betti_2
                        );
                        println!(
                            "  {:<14} SNF satisfied = {}, trivial = {}",
                            "", report.snf_satisfied, report.snf_trivial
                        );
                        println!(
                            "  {:<14} Layer 3: {}  Layer 1: {}",
                            "",
                            report.layer3_kind,
                            if report.layer1_satisfiable { "satisfiable" } else { "not invoked" }
                        );
                        println!(
                            "  {:<14} Strategy: {}  Selection: {} packages",
                            "",
                            report.strategy_used,
                            report.selection.len()
                        );
                    }
                    Err(e) => {
                        info(&format!(
                            "  JAGR analysis failed for {}: {}",
                            root.display(),
                            e
                        ));
                    }
                }
            }
        }
        return Ok(());
    }

    if let Some(("init", sub_m)) = matches.subcommand() {
        let _accept_defaults = sub_m.get_flag("yes");
        let private = sub_m.get_flag("private");
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let package_json_path = cwd.join("package.json");
        if package_json_path.exists() {
            return Err("package.json already exists".to_string());
        }
        let name = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-package")
            .to_lowercase()
            .replace(" ", "-");
        let json_value = serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "description": "",
            "main": "index.js",
            "scripts": {
                "test": "echo \"Error: no test specified\" && exit 1"
            },
            "keywords": [],
            "author": "",
            "license": "MIT",
            "private": private
        });
        let json_str = serde_json::to_string_pretty(&json_value).map_err(|e| e.to_string())?;
        fs::write(&package_json_path, &json_str).map_err(|e| e.to_string())?;
        success(&format!(
            "Created {} in {}",
            package_json_path.display(),
            cwd.display()
        ));
        info("Run `jhol install` to get started.");
        return Ok(());
    }

    if let Some(("add", sub_m)) = matches.subcommand() {
        let packages: Vec<&str> = sub_m
            .get_many::<String>("package")
            .map(|v| v.map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let quiet = sub_m.get_flag("quiet");
        if packages.is_empty() {
            return Err("At least one package must be specified".to_string());
        }

        // Soul Path: Jacobian conflict risk prediction before fetch.
        // Compute ∂R/∂X against existing deps to warn if adding is high-risk.
        let mut frontier_risk = -1.0f64;
        let mut has_deps = false;
        if let Ok(pj) = std::fs::read_to_string("package.json") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&pj) {
                let mut deps_map = std::collections::HashMap::new();
                if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
                    for (k, v) in deps {
                        deps_map.insert(k.clone(), v.as_str().unwrap_or("*").to_string());
                    }
                }
                if let Some(deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
                    for (k, v) in deps {
                        deps_map.entry(k.clone()).or_insert_with(|| v.as_str().unwrap_or("*").to_string());
                    }
                }
                if !deps_map.is_empty() {
                    has_deps = true;
                    let add_strs: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
                    let risk = jhol_core::soul_path::add_conflict_risk(&deps_map, &add_strs);
                    frontier_risk = risk;
                    if risk > 0.5 && !quiet {
                        println!(
                            "{} Jacobian risk: {:.0}% — adding {} may cause conflicts",
                            "⚠".yellow(),
                            risk * 100.0,
                            packages.join(", ")
                        );
                    } else if !quiet {
                        println!(
                            "{} Jacobian risk: {:.0}% — adding {} is low-risk",
                            "✓".green(),
                            risk * 100.0,
                            packages.join(", ")
                        );
                    }
                }
            }
        }

        let config = jhol_core::load_config(
            &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        );

        // Soul Path: low-risk add uses frontier-only append (homological delta).
        // Skips full re-resolve; downloads frontier + projects into node_modules.
        let use_frontier = has_deps && frontier_risk >= 0.0 && frontier_risk < 0.4
            && jhol_core::soul_path::auto_daemon_enabled(); // gate: daemon keeps zero-copy hot
        if use_frontier {
            let add_strs: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
            match jhol_core::soul_path::frontier_add(&add_strs, quiet) {
                Ok(()) => {
                    success("Package(s) added (frontier delta).");
                    return Ok(());
                }
                Err(e) => {
                    if !quiet {
                        println!("Frontier add failed ({}), falling back to full install", e);
                    }
                }
            }
        }

        // Soul Path: high-risk add prefetches predicted conflict cluster first.
        // Uses spectral conflict_risk to warm the cache for likely problem packages.
        if has_deps && frontier_risk >= 0.5 && !quiet {
            let add_strs: Vec<String> = packages.iter().map(|s| s.to_string()).collect();
            // Prefetch using context (existing deps + adding packages)
            jhol_core::soul_path::predictive_prefetch(&add_strs, 5);
        }

        let opts = jhol_core::InstallOptions {
            no_cache: false,
            quiet,
            backend: config.backend.unwrap_or(jhol_core::Backend::Bun),
            lockfile_only: false,
            offline: false,
            strict_lockfile: false,
            strict_peer_deps: false,
            from_lockfile: false,
            native_only: false,
            no_scripts: true,
            script_allowlist: None,
            resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
            overrides: None,
            telemetry: jhol_core::telemetry_enabled(&config),
        };
        let refs: Vec<&str> = packages;
        jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string())?;
        // Write install markers for zero-binary-startup fast path
        if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
            jhol_core::daemon_cache::write_nm_fingerprint(Path::new("."), hash);
            jhol_core::daemon_cache::store_last_hash(hash);
            jhol_core::daemon_cache::write_status(hash, Path::new("."));
        }
        success("Package(s) added.");
        return Ok(());
    }

    if let Some(("remove", sub_m)) = matches.subcommand() {
        let packages: Vec<&str> = sub_m
            .get_many::<String>("package")
            .map(|v| v.map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let save = sub_m.get_flag("save");
        if packages.is_empty() {
            return Err("At least one package must be specified".to_string());
        }
        for pkg in packages {
            jhol_core::uninstall(pkg, save).map_err(|e| e.to_string())?;
        }
        success("Package(s) removed.");
        return Ok(());
    }

    if let Some(("link", sub_m)) = matches.subcommand() {
        let package_dir = sub_m
            .get_one::<String>("package")
            .map(|s| std::path::PathBuf::from(s))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if !package_dir.exists() {
            return Err(format!(
                "Package directory does not exist: {}",
                package_dir.display()
            ));
        }
        let package_json = package_dir.join("package.json");
        if !package_json.exists() {
            return Err(format!(
                "No package.json found in {}",
                package_dir.display()
            ));
        }
        let pkg_data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&package_json).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let name = pkg_data
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or("Package name not found in package.json")?;
        // Validate package name to prevent path traversal
        jhol_core::validate_package_name(name)
            .ok_or_else(|| format!("invalid package name: {}", name))?;
        let node_modules = cwd.join("node_modules");
        fs::create_dir_all(&node_modules).map_err(|e| e.to_string())?;
        let link_path = if name.starts_with('@') {
            let parts: Vec<&str> = name.split('/').collect();
            let scope_dir = node_modules.join(parts[0]);
            fs::create_dir_all(&scope_dir).map_err(|e| e.to_string())?;
            scope_dir.join(parts[1])
        } else {
            node_modules.join(name)
        };
        if link_path.exists() {
            if link_path.is_symlink() {
                fs::remove_file(&link_path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_dir_all(&link_path).map_err(|e| e.to_string())?;
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&package_dir, &link_path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&package_dir, &link_path).map_err(|e| e.to_string())?;
        success(&format!("Linked {} -> {}", name, link_path.display()));
        return Ok(());
    }

    if let Some(("unlink", sub_m)) = matches.subcommand() {
        let package = sub_m
            .get_one::<String>("package")
            .map(|s| s.as_str())
            .unwrap();
        // Validate package name to prevent path traversal
        jhol_core::validate_package_name(package)
            .ok_or_else(|| format!("invalid package name: {}", package))?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let node_modules = cwd.join("node_modules");
        let link_path = if package.starts_with('@') {
            let parts: Vec<&str> = package.split('/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid scoped package name: {}", package));
            }
            node_modules.join(parts[0]).join(parts[1])
        } else {
            node_modules.join(package)
        };
        if !link_path.exists() {
            return Err(format!("Package {} is not linked", package));
        }
        if !link_path.is_symlink() {
            return Err(format!("Package {} is not a symlink", package));
        }
        fs::remove_file(&link_path).map_err(|e| e.to_string())?;
        success(&format!("Unlinked {}", package));
        return Ok(());
    }

    if let Some(("ci", sub_m)) = matches.subcommand() {
        jhol_core::init_cache().map_err(|e| format!("Failed to initialize cache: {}", e))?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let config = jhol_core::load_config(&cwd);
        if let Some(ref d) = config.cache_dir {
            env::set_var("JHOL_CACHE_DIR", d);
        }
        let quiet = false;
        let offline = sub_m.get_flag("offline")
            || env::var("JHOL_OFFLINE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let all_workspaces = sub_m.get_flag("all-workspaces");
        let jagr_check = sub_m.get_flag("jagr-check");
        let backend = match sub_m.get_one::<String>("backend") {
            Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
            Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
            _ => config.backend,
        };
        let backend = jhol_core::resolve_backend(backend);
        let json_out = sub_m.get_flag("json");
        let roots: Vec<std::path::PathBuf> = if all_workspaces {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() {
                vec![cwd.clone()]
            } else {
                r.into_iter().map(|p| cwd.join(p)).collect()
            }
        } else {
            vec![cwd.clone()]
        };
        let mut jagr_all_ok = true;
        for root in &roots {
            std::env::set_current_dir(root)
                .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
            let specs =
                jhol_core::resolve_install_from_package_json(true).map_err(|e| e.to_string())?;
            let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
            let opts = jhol_core::InstallOptions {
                no_cache: false,
                quiet,
                backend,
                lockfile_only: false,
                offline,
                strict_lockfile: true,
                strict_peer_deps: false,
                from_lockfile: true,
                native_only: true,
                no_scripts: true,
                script_allowlist: None,
                resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
                overrides: None,
                telemetry: jhol_core::telemetry_enabled(&config),
            };
            jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string())?;
            // Write install markers for zero-binary-startup fast path
            if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
                jhol_core::daemon_cache::write_nm_fingerprint(Path::new("."), hash);
                jhol_core::daemon_cache::store_last_hash(hash);
                jhol_core::daemon_cache::write_status(hash, Path::new("."));
            }

            if jagr_check {
                // After install, run JAGR three-layer analysis on the manifest
                // to confirm the lockfile's resolutions still satisfy it.
                let manifest = root.join("package.json");
                match jhol_core::jagr_analyze_project_manifest(&manifest) {
                    Ok(r) => {
                        if r.is_satisfiable {
                            if use_color() {
                                println!(
                                    "{} {}: JAGR OK ({}: β₁={}, β₂={}, SNF={})",
                                    "[JAGR]".bright_green(),
                                    root.display(),
                                    r.strategy_used,
                                    r.betti_1,
                                    r.betti_2,
                                    r.snf_satisfied
                                );
                            } else {
                                println!(
                                    "[JAGR] {}: JAGR OK ({}: β₁={}, β₂={}, SNF={})",
                                    root.display(),
                                    r.strategy_used,
                                    r.betti_1,
                                    r.betti_2,
                                    r.snf_satisfied
                                );
                            }
                        } else {
                            jagr_all_ok = false;
                            error(&format!(
                                "[JAGR] {}: UNSAT — {}",
                                root.display(),
                                r.explanation
                            ));
                        }
                    }
                    Err(e) => {
                        jagr_all_ok = false;
                        error(&format!(
                            "[JAGR] {}: analysis failed: {}",
                            root.display(),
                            e
                        ));
                    }
                }
            }
        }
        std::env::set_current_dir(&cwd).ok();
        if json_out {
            let jagr_field = if jagr_check {
                format!(
                    ", \"jagr_check\":\"{}\"",
                    if jagr_all_ok { "ok" } else { "fail" }
                )
            } else {
                String::new()
            };
            println!("{{\"schemaVersion\":\"1\",\"command\":\"ci\",\"status\":\"ok\",\"workspaces\":{}{}}}",
                roots.len(), jagr_field);
        }
        if jagr_check && !jagr_all_ok {
            std::process::exit(2);
        }
        return Ok(());
    }

    jhol_core::init_cache().map_err(|e| format!("Failed to initialize cache: {}", e))?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    jhol_core::apply_enterprise_network_env(&cwd);
    let config = jhol_core::load_config(&cwd);
    if let Some(ref d) = config.cache_dir {
        env::set_var("JHOL_CACHE_DIR", d);
    }

    // ── Feature 10: output config ──────────────────────────────────────────────
    if config.output_quiet == Some(true) && env::var("JHOL_QUIET").is_err() {
        env::set_var("JHOL_QUIET", "1");
    }
    if config.output_no_color == Some(true) && env::var("NO_COLOR").is_err() {
        env::set_var("NO_COLOR", "1");
    }
    if config.output_json_log == Some(true) {
        if env::var("JHOL_LOG_FILE").is_err() {
            env::set_var("JHOL_LOG_FILE", "1");
        }
    }

    match matches.subcommand() {
        Some(("install", sub_m)) => {
            let no_cache = sub_m.get_flag("no-cache");
            let quiet = sub_m.get_flag("quiet");
            let lockfile_only = sub_m.get_flag("lockfile-only");
            let offline = sub_m.get_flag("offline")
                || env::var("JHOL_OFFLINE")
                    .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
            let strict_lockfile = sub_m.get_flag("frozen") || config.frozen.unwrap_or(false);
            let strict_peer_deps = sub_m.get_flag("strict-peer-deps")
                || env::var("JHOL_STRICT_PEER_DEPS")
                    .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

            // Feature 1: native_only / fallback_backend from config (CLI takes precedence)
            let cli_fallback = sub_m.get_flag("fallback-backend");
            let cli_native_only = sub_m.get_flag("native-only");
            let native_only = if cli_native_only {
                true
            } else if cli_fallback {
                false
            } else {
                // Neither CLI flag set — use config fallback_backend if present
                config.fallback_backend.map(|fb| !fb).unwrap_or(true)
            };

            let all_workspaces = sub_m.get_flag("all-workspaces");

            // Feature 1: no_scripts — CLI flags take precedence, then config
            let cli_scripts_flag = sub_m.get_flag("scripts");
            let cli_no_scripts_flag = sub_m.get_flag("no-scripts");
            let no_scripts = if cli_scripts_flag {
                false
            } else if cli_no_scripts_flag {
                true
            } else {
                config.no_scripts.unwrap_or(true)
            };

            let mut use_daemon = sub_m.get_flag("daemon");
            let mut daemon_auto_attach = false;
            if !use_daemon && jhol_core::soul_path::auto_daemon_enabled() {
                #[cfg(unix)]
                {
                    // Safety gate: only connect to existing socket, don't auto-start
                    let sock = daemon_socket_path();
                    if sock.exists() {
                        use_daemon = true;
                        daemon_auto_attach = true;
                        if !quiet {
                            info("Soul Path: auto-daemon enabled");
                        }
                    }
                }
                #[cfg(not(unix))]
                { /* daemon is Unix-only */ }
            }
            if let Some(resolver) = sub_m.get_one::<String>("resolver") {
                env::set_var("JHOL_RESOLVER", resolver);
            }

            // Feature 1: env vars from config (only set if not already set)
            if let Some(ft) = config.fetch_timeout {
                if env::var("JHOL_FETCH_TIMEOUT_SECS").is_err() {
                    env::set_var("JHOL_FETCH_TIMEOUT_SECS", ft.to_string());
                }
            }
            if let Some(it) = config.install_timeout {
                if env::var("JHOL_INSTALL_TIMEOUT_SECS").is_err() {
                    env::set_var("JHOL_INSTALL_TIMEOUT_SECS", it.to_string());
                }
            }
            if let Some(con) = config.concurrency {
                if env::var("JHOL_DOWNLOAD_CONCURRENCY").is_err() {
                    env::set_var("JHOL_DOWNLOAD_CONCURRENCY", con.to_string());
                }
            }

            let script_allowlist: Option<HashSet<String>> = env::var("JHOL_SCRIPT_ALLOWLIST")
                .ok()
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<HashSet<_>>()
                })
                .filter(|s| !s.is_empty());
            let backend = match sub_m.get_one::<String>("backend") {
                Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
                Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
                _ => config.backend,
            };
            let backend = jhol_core::resolve_backend(backend);
            let json_out = sub_m.get_flag("json");
            if quiet || json_out {
                env::set_var("JHOL_QUIET", "1");
            }
            let packages: Vec<String> = sub_m
                .get_many::<String>("package")
                .map(|it| it.map(|s| s.clone()).collect())
                .unwrap_or_default();

            // Feature 5: workspaces parallel / bail
            let ws_parallel = config.workspaces_parallel.unwrap_or(false);
            let ws_bail = config.workspaces_bail.unwrap_or(true);

            let roots: Vec<std::path::PathBuf> = if all_workspaces {
                let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                if r.is_empty() {
                    vec![cwd.clone()]
                } else {
                    r.into_iter().map(|p| cwd.join(p)).collect()
                }
            } else {
                vec![cwd.clone()]
            };

            // Helper: run install for a single root (used by both sequential and parallel paths)
            let run_install_for_root = |root: &std::path::Path,
                                        packages: &Vec<String>,
                                        config: &jhol_core::Config,
                                        script_allowlist: &Option<HashSet<String>>,
                                        quiet: bool,
                                        json_out: bool,
                                        no_cache: bool,
                                        lockfile_only: bool,
                                        offline: bool,
                                        strict_lockfile: bool,
                                        strict_peer_deps: bool,
                                        from_lockfile_override: Option<bool>,
                                        native_only: bool,
                                        no_scripts: bool,
                                        backend: jhol_core::Backend,
                                        use_daemon: bool|
             -> Result<(), String> {
                let specs: Vec<String> = if packages.is_empty() {
                    jhol_core::resolve_install_from_package_json(strict_lockfile)
                        .map_err(|e| e.to_string())?
                } else {
                    packages.iter().map(|s| s.clone()).collect()
                };
                if specs.is_empty() {
                    return Ok(());
                }
                let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
                let from_lockfile = from_lockfile_override.unwrap_or_else(|| {
                    packages.is_empty()
                        && specs.iter().all(|s| {
                            s.contains('@') && s.rfind('@').map(|i| i > 0).unwrap_or(false)
                        })
                        && jhol_core::has_jhol_lockfile(std::path::Path::new("."))
                });
                let opts = jhol_core::InstallOptions {
                    no_cache,
                    quiet,
                    backend,
                    lockfile_only,
                    offline,
                    strict_lockfile,
                    strict_peer_deps,
                    from_lockfile,
                    native_only,
                    no_scripts,
                    script_allowlist: {
                        let mut merged = script_allowlist.clone().unwrap_or_default();
                        if !no_scripts {
                            merged.extend(read_trusted_dependencies(std::path::Path::new(
                                "package.json",
                            )));
                        }
                        if let Some(ref td) = config.trusted_dependencies {
                            merged.extend(td.iter().cloned());
                        }
                        if merged.is_empty() {
                            None
                        } else {
                            Some(merged)
                        }
                    },
                    resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
                    overrides: {
                        let mut ov = config.overrides.clone().unwrap_or_default();
                        ov.extend(config.resolutions.clone().unwrap_or_default());
                        if ov.is_empty() {
                            None
                        } else {
                            Some(ov)
                        }
                    },
                    telemetry: jhol_core::telemetry_enabled(&config),
                };
                // Feature 6: pre-install hook
                if let Some(ref hook) = config.hook_pre_install {
                    if let Err(e) = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(hook)
                        .current_dir(root)
                        .status()
                    {
                        eprintln!("Warning: pre-install hook failed: {}", e);
                    }
                }
                jhol_core::log(&format!("Installing: {:?}", specs));
                let install_start = std::time::Instant::now();
                let pkg_count = specs.len();
                let install_result = if use_daemon {
                    let dr = daemon_try_install(root, &specs, &opts, daemon_auto_attach);
                    // Safety gate fallback: auto-attach with no socket → direct install
                    if daemon_auto_attach && dr.is_err() {
                        if !quiet {
                            info("Daemon unavailable — falling back to direct install");
                        }
                        direct_install(root, &specs, &opts, pkg_count, quiet, json_out)
                    } else {
                        dr
                    }
                } else {
                    direct_install(root, &specs, &opts, pkg_count, quiet, json_out)
                };
                install_result?;
                let elapsed = install_start.elapsed();
                print_install_summary(pkg_count, elapsed, quiet, json_out);
                // Feature 6: post-install hook
                if let Some(ref hook) = config.hook_post_install {
                    if let Err(e) = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(hook)
                        .current_dir(root)
                        .status()
                    {
                        eprintln!("Warning: post-install hook failed: {}", e);
                    }
                }
                // Feature 4: cache auto-management
                if let Some(max_gb) = config.cache_max_size_gb {
                    if let Ok((bytes, _)) = jhol_core::cache_size_bytes() {
                        let max_bytes = (max_gb * 1024.0 * 1024.0 * 1024.0) as u64;
                        if bytes > max_bytes {
                            let _ = jhol_core::cache_prune(config.cache_keep_recent);
                        }
                    }
                }
                if config.cache_auto_prewarm == Some(true) {
                    let _ = thread::spawn(|| {
                        let _ = jhol_core::prefetch_from_lockfile(true);
                    });
                }
                Ok(())
            };

            if all_workspaces && ws_parallel && roots.len() > 1 {
                // Feature 5: parallel workspace install
                let mut handles = Vec::new();
                let telemetry_flag = jhol_core::telemetry_enabled(&config);
                for root in &roots {
                    let root = root.clone();
                    let packages = packages.clone();
                    let config_overrides_clone = config.overrides.clone();
                    let config_resolutions_clone = config.resolutions.clone();
                    let config_trusted_clone = config.trusted_dependencies.clone();
                    let config_hook_pre = config.hook_pre_install.clone();
                    let config_hook_post = config.hook_post_install.clone();
                    let config_cache_max = config.cache_max_size_gb;
                    let config_cache_keep = config.cache_keep_recent;
                    let config_prewarm = config.cache_auto_prewarm;
                    let script_allowlist_clone = script_allowlist.clone();
                    handles.push(thread::spawn(move || -> Result<(), String> {
                        std::env::set_current_dir(&root)
                            .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                        let specs: Vec<String> = if packages.is_empty() {
                            jhol_core::resolve_install_from_package_json(strict_lockfile)
                                .map_err(|e| e.to_string())?
                        } else {
                            packages.clone()
                        };
                        if specs.is_empty() {
                            return Ok(());
                        }
                        let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
                        let from_lockfile = packages.is_empty()
                            && specs.iter().all(|s| {
                                s.contains('@') && s.rfind('@').map(|i| i > 0).unwrap_or(false)
                            })
                            && jhol_core::has_jhol_lockfile(std::path::Path::new("."));
                        let opts = jhol_core::InstallOptions {
                            no_cache,
                            quiet,
                            backend,
                            lockfile_only,
                            offline,
                            strict_lockfile,
                            strict_peer_deps,
                            from_lockfile,
                            native_only,
                            no_scripts,
                            script_allowlist: {
                                let mut merged = script_allowlist_clone.unwrap_or_default();
                                if !no_scripts {
                                    merged.extend(read_trusted_dependencies(std::path::Path::new(
                                        "package.json",
                                    )));
                                }
                                if let Some(ref td) = config_trusted_clone {
                                    merged.extend(td.iter().cloned());
                                }
                                if merged.is_empty() {
                                    None
                                } else {
                                    Some(merged)
                                }
                            },
                            resolver: std::env::var("JHOL_RESOLVER").unwrap_or_default(),
                            overrides: {
                                let mut ov = config_overrides_clone.unwrap_or_default();
                                ov.extend(config_resolutions_clone.unwrap_or_default());
                                if ov.is_empty() {
                                    None
                                } else {
                                    Some(ov)
                                }
                            },
                            telemetry: telemetry_flag,
                        };
                        if let Some(ref hook) = config_hook_pre {
                            if let Err(e) = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(hook)
                                .current_dir(&root)
                                .status()
                            {
                                eprintln!("Warning: pre-install hook failed: {}", e);
                            }
                        }
                        jhol_core::install_package(&refs, &opts).map_err(|e| e.to_string())?;
                        // Write install markers for zero-binary-startup fast path
                        if let Ok(hash) = jhol_core::capsule::compute_manifest_hash(Path::new(".")) {
                            jhol_core::daemon_cache::write_nm_fingerprint(Path::new("."), hash);
                            jhol_core::daemon_cache::store_last_hash(hash);
                            jhol_core::daemon_cache::write_status(hash, Path::new("."));
                        }
                        if let Some(ref hook) = config_hook_post {
                            if let Err(e) = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(hook)
                                .current_dir(&root)
                                .status()
                            {
                                eprintln!("Warning: post-install hook failed: {}", e);
                            }
                        }
                        if let Some(max_gb) = config_cache_max {
                            if let Ok((bytes, _)) = jhol_core::cache_size_bytes() {
                                let max_bytes = (max_gb * 1024.0 * 1024.0 * 1024.0) as u64;
                                if bytes > max_bytes {
                                    let _ = jhol_core::cache_prune(config_cache_keep);
                                }
                            }
                        }
                        if config_prewarm == Some(true) {
                            let _ = thread::spawn(|| {
                                let _ = jhol_core::prefetch_from_lockfile(true);
                            });
                        }
                        Ok(())
                    }));
                }
                let mut errors: Vec<String> = Vec::new();
                for h in handles {
                    match h.join() {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            if ws_bail {
                                return Err(e);
                            }
                            errors.push(e);
                        }
                        Err(_) => {
                            let e = "workspace install thread panicked".to_string();
                            if ws_bail {
                                return Err(e);
                            }
                            errors.push(e);
                        }
                    }
                }
                std::env::set_current_dir(&cwd).ok();
                if !errors.is_empty() {
                    return Err(format!("Workspace install errors:\n{}", errors.join("\n")));
                }
            } else {
                // Sequential workspace install (original behavior)
                for root in &roots {
                    std::env::set_current_dir(root)
                        .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                    if !quiet && roots.len() > 1 {
                        info(&format!("Workspace: {}", root.display()));
                    }
                    if packages.is_empty() && lockfile_only {
                        jhol_core::install_lockfile_only(jhol_core::backend::resolve_backend(None))
                            .map_err(|e| e.to_string())?;
                        continue;
                    }
                    if packages.is_empty()
                        && !no_cache
                        && !offline
                        && jhol_core::lockfile_fingerprint_matches()
                    {
                        if !quiet {
                            jhol_core::log("Already up-to-date.");
                        }
                        continue;
                    }
                    run_install_for_root(
                        root,
                        &packages,
                        &config,
                        &script_allowlist,
                        quiet,
                        json_out,
                        no_cache,
                        lockfile_only,
                        offline,
                        strict_lockfile,
                        strict_peer_deps,
                        None,
                        native_only,
                        no_scripts,
                        backend,
                        use_daemon,
                    )?;
                }
                std::env::set_current_dir(&cwd).ok();
            }
            if json_out {
                println!("{{\"schemaVersion\":\"1\",\"command\":\"install\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
            } else if quiet {
                success("Done.");
            }
        }
        Some(("doctor", sub_m)) => {
            let quiet = sub_m.get_flag("quiet");
            let do_fix = sub_m.get_flag("fix");
            let explain = sub_m.get_flag("explain");
            let use_jagr = sub_m.get_flag("jagr");
            let all_workspaces = sub_m.get_flag("all-workspaces");
            let json_out = sub_m.get_flag("json");
            let backend = match sub_m.get_one::<String>("backend") {
                Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
                Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
                _ => config.backend,
            };
            let backend = jhol_core::resolve_backend(backend);
            if quiet || json_out {
                env::set_var("JHOL_QUIET", "1");
            }
            if explain {
                let report = jhol_core::explain_project_health()?;
                println!("{}", report);
                return Ok(());
            }
            if use_jagr {
                // JAGR-driven doctor: run the three-layer analysis on the
                // project's manifest, then print a health summary that
                // combines layer outputs with project-level recommendations.
                let manifest = cwd.join("package.json");
                match jhol_core::jagr_analyze_project_manifest(&manifest) {
                    Ok(report) => {
                        if use_color() {
                            println!(
                                "{}",
                                "JAGR Doctor — Three-Layer Resolver Health Check"
                                    .bright_cyan()
                                    .bold()
                            );
                        } else {
                            println!("JAGR Doctor — Three-Layer Resolver Health Check");
                        }
                        println!("Manifest:       {}", report.manifest_path);
                        println!("Satisfiable:    {}", report.is_satisfiable);
                        println!("Strategy:       {}", report.strategy_used);
                        println!("β₁:             {}", report.betti_1);
                        println!("β₂:             {}", report.betti_2);
                        println!("SNF satisfied:  {}", report.snf_satisfied);
                        println!("Packages:       {}", report.package_count);
                        let total = report.layer_times_ms.get("total").copied().unwrap_or(0.0);
                        let layer0 = report.layer_times_ms.get("layer0").copied().unwrap_or(0.0);
                        println!("Total time:     {:.2} ms", total);

                        // ── Resolution Differential (Layer 0) output ──────────
                        if let Some(ref diff) = report.differential {
                            let stability_label = match diff.stability {
                                jhol_core::StabilityClass::Stable => "STABLE ✓",
                                jhol_core::StabilityClass::Marginal => "MARGINAL ⚠",
                                jhol_core::StabilityClass::Fragile => "FRAGILE ✗",
                                jhol_core::StabilityClass::Degenerate => "–",
                            };
                            if use_color() {
                                println!();
                                println!("{}", "  ── Resolution Differential (Layer 0) ──".cyan());
                                println!(
                                    "  Spectral gap δ:    {:.4}  [{:.2} ms]",
                                    diff.spectral_gap, layer0
                                );
                                println!("  Fiedler value λ₂:  {:.4}", diff.fiedler_value);
                                println!("  Predicted β₁:      {}", diff.predicted_beta1);
                                println!("  Stability:         {}", stability_label);
                                let risky = diff.high_risk_packages(0.5);
                                if !risky.is_empty() {
                                    println!(
                                        "  High-risk pkgs:    {}",
                                        risky
                                            .iter()
                                            .take(3)
                                            .map(|(k, v)| format!("{} ({:.0}%)", k, v * 100.0))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    );
                                }
                            } else {
                                println!(
                                    "Spectral gap:   {:.4}  [{:.2}ms]",
                                    diff.spectral_gap, layer0
                                );
                                println!("Stability:      {}", stability_label);
                            }
                        }
                        println!();
                        // Health verdict
                        if !report.is_satisfiable {
                            error("JAGR verdict: UNSAT — dependency conflict detected");
                            if !report.explanation.is_empty() {
                                println!("Explanation:");
                                for line in report.explanation.lines() {
                                    println!("  {}", line);
                                }
                            }
                            std::process::exit(2);
                        } else if !report.snf_satisfied {
                            warning("JAGR verdict: Non-SNF — project has topological cycles in its dep graph");
                            println!("  Resolution used Layer 1 (Max-SAT) instead of the faster greedy path.");
                            println!("  Consider reviewing peer/optional dependencies to reduce the Betti numbers.");
                        } else if report.gluing_has_obstruction {
                            warning(
                                "JAGR verdict: SNF, but Mayer-Vietoris gluing obstruction present",
                            );
                        } else {
                            success("JAGR verdict: Healthy — SNF satisfied, no gluing obstruction, satisfiable.");
                        }
                        if json_out {
                            let j = serde_json::json!({
                                "manifest": report.manifest_path,
                                "satisfiable": report.is_satisfiable,
                                "strategy_used": report.strategy_used,
                                "b1": report.betti_1,
                                "b2": report.betti_2,
                                "snf_satisfied": report.snf_satisfied,
                                "package_count": report.package_count,
                                "total_time_ms": report.layer_times_ms.get("total").copied().unwrap_or(0.0),
                                "explanation": report.explanation,
                            });
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&j)
                                    .unwrap_or_else(|_| "{}".to_string())
                            );
                        }
                    }
                    Err(e) => {
                        error(&format!("JAGR doctor failed: {}", e));
                        std::process::exit(2);
                    }
                }
                return Ok(());
            }
            let roots: Vec<std::path::PathBuf> = if all_workspaces {
                let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                if r.is_empty() {
                    vec![cwd.clone()]
                } else {
                    r.into_iter().map(|p| cwd.join(p)).collect()
                }
            } else {
                vec![cwd.clone()]
            };
            for root in &roots {
                std::env::set_current_dir(root)
                    .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                if !quiet && !json_out && roots.len() > 1 {
                    info(&format!("Workspace: {}", root.display()));
                }
                if do_fix {
                    jhol_core::log("Running doctor --fix");
                    jhol_core::fix_dependencies(quiet, backend).map_err(|e| e.to_string())?;
                } else {
                    jhol_core::log("Running doctor (check only)");
                    jhol_core::check_dependencies(quiet, backend).map_err(|e| e.to_string())?;
                }
            }
            std::env::set_current_dir(&cwd).ok();
            if json_out {
                println!("{{\"schemaVersion\":\"1\",\"command\":\"doctor\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
            } else if quiet {
                success("Done.");
            }
        }
        Some(("audit", sub_m)) => {
            let quiet = sub_m.get_flag("quiet");
            let do_fix = sub_m.get_flag("fix");
            let gate = sub_m.get_flag("gate") || config.audit_gate == Some(true);
            let all_workspaces = sub_m.get_flag("all-workspaces");
            let json_out = sub_m.get_flag("json");
            let backend = match sub_m.get_one::<String>("backend") {
                Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
                Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
                _ => config.backend,
            };
            let backend = jhol_core::resolve_backend(backend);
            if quiet || json_out {
                env::set_var("JHOL_QUIET", "1");
            }
            // Feature 3: audit filtering
            let ignore_advisories: Vec<String> =
                config.audit_ignore_advisories.clone().unwrap_or_default();
            let min_severity: Option<String> = config.audit_min_severity.clone();

            // Feature 6: pre-audit hook
            if let Some(ref hook) = config.hook_pre_audit {
                if let Err(e) = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(hook)
                    .current_dir(&cwd)
                    .status()
                {
                    eprintln!("Warning: pre-audit hook failed: {}", e);
                }
            }

            let audit_result: Result<(), String> = (|| {
                if gate {
                    jhol_core::run_audit_gate_filtered(
                        backend,
                        &ignore_advisories,
                        min_severity.as_deref(),
                    )?;
                    if json_out {
                        println!("{{\"schemaVersion\":\"1\",\"command\":\"audit\",\"status\":\"ok\",\"gate\":true}}");
                    }
                } else if json_out && !do_fix {
                    let json_bytes = jhol_core::run_audit_raw_filtered(
                        backend,
                        &ignore_advisories,
                        min_severity.as_deref(),
                    )
                    .map_err(|e| e.to_string())?;
                    println!("{}", String::from_utf8_lossy(&json_bytes));
                } else {
                    let roots: Vec<std::path::PathBuf> = if all_workspaces {
                        let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                        if r.is_empty() {
                            vec![cwd.clone()]
                        } else {
                            r.into_iter().map(|p| cwd.join(p)).collect()
                        }
                    } else {
                        vec![cwd.clone()]
                    };
                    for root in &roots {
                        std::env::set_current_dir(root)
                            .map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                        if !quiet && !json_out && roots.len() > 1 {
                            info(&format!("Workspace: {}", root.display()));
                        }
                        if do_fix {
                            jhol_core::run_audit_fix_filtered(
                                backend,
                                quiet,
                                &ignore_advisories,
                                min_severity.as_deref(),
                            )?;
                        } else {
                            jhol_core::run_audit_filtered(
                                backend,
                                quiet,
                                &ignore_advisories,
                                min_severity.as_deref(),
                            )?;
                        }
                    }
                    std::env::set_current_dir(&cwd).ok();
                    if json_out {
                        println!("{{\"schemaVersion\":\"1\",\"command\":\"audit\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
                    } else if quiet {
                        success("Done.");
                    }
                }
                Ok(())
            })();

            // Feature 6: post-audit hook (best-effort, even if audit found vulns)
            if let Some(ref hook) = config.hook_post_audit {
                if let Err(e) = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(hook)
                    .current_dir(&cwd)
                    .status()
                {
                    eprintln!("Warning: post-audit hook failed: {}", e);
                }
            }
            audit_result?;
        }
        Some(("build", sub_m)) => {
            #[cfg(feature = "bundle")]
            {
                let entries: Vec<std::path::PathBuf> = sub_m
                    .get_many::<String>("entry")
                    .map(|v| v.map(|s| std::path::PathBuf::from(s)).collect())
                    .unwrap_or_default();
                let outdir = std::path::PathBuf::from(
                    sub_m
                        .get_one::<String>("outdir")
                        .map(|s| s.as_str())
                        .unwrap_or("dist"),
                );
                let format = sub_m
                    .get_one::<String>("format")
                    .map(|s| s.as_str())
                    .unwrap_or("esm");
                let target = sub_m
                    .get_one::<String>("target")
                    .map(|s| s.as_str())
                    .unwrap_or("browser");
                let minify = sub_m.get_flag("minify");
                let watch = sub_m.get_flag("watch");
                let tree_shake = sub_m.get_flag("tree-shake");
                let code_splitting = sub_m.get_flag("code-splitting");

                let mut config = jhol_bundle::config::BundleConfig::default();
                config.entries = entries;
                config.out_dir = outdir;

                if let Some(f) = jhol_bundle::config::OutputFormat::from_str(format) {
                    config.format = f;
                }
                if let Some(t) = jhol_bundle::config::Target::from_str(target) {
                    config.target = t;
                }
                config.minify = minify;
                config.watch = watch;
                config.tree_shake = tree_shake;
                config.code_splitting = code_splitting;

                if watch {
                    jhol_bundle::bundle_watch(config).map_err(|e| e.to_string())?;
                } else {
                    let result = jhol_bundle::bundle(&config).map_err(|e| e.to_string())?;
                    success(&format!(
                        "✓ Built {} modules to {} ({} files, {:?})",
                        result.module_count,
                        config.out_dir.display(),
                        result.files.len(),
                        std::time::Duration::from_millis(result.total_time_ms),
                    ));
                }
            }
            #[cfg(not(feature = "bundle"))]
            {
                return Err("Jhol bundler not enabled. Rebuild with --features bundle".to_string());
            }
        }
        Some(("prefetch", sub_m)) => {
            let quiet = sub_m.get_flag("quiet");
            jhol_core::prefetch_from_lockfile(quiet).map_err(|e| e.to_string())?;
            if !quiet {
                success("Prefetch done. Run `jhol install --offline` to install from cache.");
            }
        }
        Some(("sbom", sub_m)) => {
            let format = match sub_m.get_one::<String>("format").map(|s| s.as_str()) {
                Some("simple") => jhol_core::SbomFormat::Simple,
                _ => jhol_core::SbomFormat::CycloneDx,
            };
            let json = jhol_core::generate_sbom(format).map_err(|e| e.to_string())?;
            if let Some(out_path) = sub_m.get_one::<String>("output") {
                fs::write(out_path, &json).map_err(|e| format!("Write failed: {}", e))?;
                success(&format!("SBOM written to {}.", out_path));
            } else {
                println!("{}", json);
            }
        }
        Some(("audit-jagr", sub_m)) => {
            if let Some(diff) = sub_m.get_one::<String>("diff") {
                // --diff OLD,NEW
                let mut parts = diff.splitn(2, ',');
                let old = parts.next().unwrap_or("").to_string();
                let new = parts.next().unwrap_or("").to_string();
                if old.is_empty() || new.is_empty() {
                    return Err(
                        "audit-jagr --diff requires two paths separated by a comma".to_string()
                    );
                }
                let old_path = std::path::PathBuf::from(&old);
                let new_path = std::path::PathBuf::from(&new);
                let old_report = jhol_core::jagr_load_audit_trail(&old_path)
                    .map_err(|e| format!("Load {}: {}", old, e))?;
                let new_report = jhol_core::jagr_load_audit_trail(&new_path)
                    .map_err(|e| format!("Load {}: {}", new, e))?;
                let diffs = jhol_core::jagr_diff_reports(&old_report, &new_report);
                if diffs.is_empty() {
                    success("Reports are equivalent (no semantic diff).");
                } else {
                    println!("JAGR audit diff ({} → {}):", old, new);
                    for d in diffs {
                        println!("  • {}", d);
                    }
                }
                return Ok(());
            }
            if let Some(load_path) = sub_m.get_one::<String>("load") {
                let p = std::path::PathBuf::from(load_path);
                let r = jhol_core::jagr_load_audit_trail(&p)
                    .map_err(|e| format!("Load {}: {}", load_path, e))?;
                println!("JAGR audit trail: {}", load_path);
                println!("  Manifest:        {}", r.manifest_path);
                println!("  Satisfiable:     {}", r.is_satisfiable);
                println!("  Strategy:        {}", r.strategy_used);
                println!("  β₁/β₂:           {}/{}", r.betti_1, r.betti_2);
                println!("  SNF satisfied:   {}", r.snf_satisfied);
                println!("  Packages:        {}", r.package_count);
                println!("  Selection size:  {}", r.selection.len());
                return Ok(());
            }
            if let Some(save_path) = sub_m.get_one::<String>("save") {
                let manifest = cwd.join(
                    sub_m
                        .get_one::<String>("manifest")
                        .cloned()
                        .unwrap_or_else(|| "package.json".to_string()),
                );
                let report = jhol_core::jagr_analyze_project_manifest(&manifest)
                    .map_err(|e| e.to_string())?;
                let p = std::path::PathBuf::from(save_path);
                jhol_core::jagr_save_audit_trail(&report, &p)
                    .map_err(|e| format!("Save failed: {}", e))?;
                success(&format!("JAGR audit trail saved to {}.", save_path));
                return Ok(());
            }
            return Err(
                "audit-jagr requires one of: --save <path>, --load <path>, --diff <old,new>"
                    .to_string(),
            );
        }
        Some(("betti-history", sub_m)) => {
            let path = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("file")
                    .cloned()
                    .unwrap_or_else(|| ".jagr_betti_history.jsonl".to_string()),
            );
            let entries = jhol_core::jagr_read_betti_history(&path);
            let last: Option<usize> = sub_m.get_one::<String>("last").and_then(|s| s.parse().ok());
            let to_show: &[jhol_core::BettiHistoryEntry] = match last {
                Some(n) => {
                    let start = entries.len().saturating_sub(n);
                    &entries[start..]
                }
                None => &entries[..],
            };
            if entries.is_empty() {
                info(&format!("No JAGR Betti history at {}", path.display()));
                info("Set JHOL_BETTI_HISTORY=1 to start recording.");
            } else {
                println!(
                    "JAGR Betti history ({} entries, file: {}):",
                    entries.len(),
                    path.display()
                );
                println!(
                    "  {:<28} {:<8} {:<8} {:<8} {:<8} {:<10} {:<10}",
                    "timestamp", "β₀", "β₁", "β₂", "snf", "strategy", "pkgs"
                );
                for e in to_show {
                    let ts_str = e.timestamp.to_string();
                    let ts = if ts_str.len() > 26 {
                        format!("{}…", &ts_str[..25])
                    } else {
                        ts_str
                    };
                    println!(
                        "  {:<28} {:<8} {:<8} {:<8} {:<8} {:<10} {:<10}",
                        ts,
                        e.b0,
                        e.b1,
                        e.b2,
                        e.snf_satisfied,
                        truncate(&e.strategy, 10),
                        e.package_count
                    );
                }
            }
        }
        Some(("jagr-impact", sub_m)) => {
            let manifest_path = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("manifest")
                    .cloned()
                    .unwrap_or_else(|| "package.json".to_string()),
            );
            let json_out = sub_m.get_flag("json");

            // Accept either --target PKG@VERSION or --package PKG --version VER.
            let (pkg, ver): (String, String) =
                if let Some(target) = sub_m.get_one::<String>("target") {
                    let (a, b) = target.split_once('@').ok_or_else(|| {
                        format!("--target requires PKG@VERSION, got '{}'", target)
                    })?;
                    (a.to_string(), b.to_string())
                } else {
                    let pkg = sub_m
                        .get_one::<String>("package")
                        .cloned()
                        .ok_or("--package PKG (or --target PKG@VERSION) is required")?;
                    let ver = sub_m
                        .get_one::<String>("version")
                        .cloned()
                        .ok_or("--version VER (or --target PKG@VERSION) is required")?;
                    (pkg, ver)
                };

            let entry = jhol_core::jagr_simulate_upgrade(&manifest_path, &pkg, &ver)
                .map_err(|e| e.to_string())?;

            if json_out {
                let j = serde_json::json!({
                    "package": entry.package,
                    "from_version": entry.from_version,
                    "to_version": entry.to_version,
                    "risk": entry.risk.as_str(),
                    "betti_1_before": entry.betti_1_before,
                    "betti_1_after": entry.betti_1_after,
                    "snf_satisfied_before": entry.snf_satisfied_before,
                    "snf_satisfied_after": entry.snf_satisfied_after,
                    "recommendation": entry.recommendation,
                });
                println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
            } else {
                if use_color() {
                    println!("{}", "JAGR Upgrade Impact".bright_cyan().bold());
                } else {
                    println!("JAGR Upgrade Impact");
                }
                println!("  Package:        {}", entry.package);
                println!("  From:           {}", entry.from_version);
                println!("  To:             {}", entry.to_version);
                println!(
                    "  Risk:           {} {}",
                    entry.risk.emoji(),
                    entry.risk.as_str()
                );
                println!(
                    "  β₁:             {} → {}",
                    entry.betti_1_before, entry.betti_1_after
                );
                println!(
                    "  SNF satisfied:  {} → {}",
                    entry.snf_satisfied_before, entry.snf_satisfied_after
                );
                println!("  Recommendation: {}", entry.recommendation);
            }
        }
        Some(("jagr", sub_m)) => {
            let manifest_path = std::path::PathBuf::from(
                sub_m
                    .get_one::<String>("manifest")
                    .cloned()
                    .unwrap_or_else(|| "package.json".to_string()),
            );
            let json_out = sub_m.get_flag("json");
            let quiet = sub_m.get_flag("quiet");
            let all_workspaces = sub_m.get_flag("all-workspaces");
            let graph_format = sub_m.get_one::<String>("graph").cloned();
            let output_path = sub_m.get_one::<String>("output").cloned();
            let show_config = sub_m.get_flag("show-config");

            if show_config {
                let cfg = jhol_core::load_jagr_config(&cwd);
                println!("JAGR config (from .jholrc + env, merged):");
                println!("  use_lp:           {:?}", cfg.use_lp);
                println!("  timeout_ms:       {:?}", cfg.timeout_ms);
                println!("  skip_layer3:      {:?}", cfg.skip_layer3);
                println!("  skip_layer2:      {:?}", cfg.skip_layer2);
                println!("  run_all_layers:   {:?}", cfg.run_all_layers);
                println!("  betti_history:    {:?}", cfg.betti_history);
                println!("  betti_history_dir:{:?}", cfg.betti_history_dir);
                println!("  max_versions:     {:?}", cfg.max_versions);
                println!("  strict:           {:?}", cfg.strict);
                return Ok(());
            }

            let lockfile_only = sub_m.get_flag("lockfile-only");

            // Read CLI-flag overrides for the per-project JAGR config.
            // These override the .jholrc values when set on the command line.
            let cli_skip_layer3 = sub_m.get_flag("skip-layer3");
            let cli_skip_layer2 = sub_m.get_flag("skip-layer2");
            let cli_run_all_layers = sub_m.get_flag("run-all-layers");

            // Collect the list of (cwd_relative_root, manifest_path) pairs to analyze.
            // Preserve the manifest's directory component when the user pointed
            // at a subdir manifest, not just the basename.
            let manifest_basename = manifest_path
                .file_name()
                .map(|s| s.to_os_string())
                .unwrap_or_else(|| std::ffi::OsString::from("package.json"));
            let manifest_dir = manifest_path
                .parent()
                .map(|p| p.to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let roots: Vec<(std::path::PathBuf, std::path::PathBuf)> = if all_workspaces {
                let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
                if r.is_empty() {
                    vec![(cwd.clone(), manifest_path.clone())]
                } else {
                    r.into_iter()
                        .map(|p| {
                            let workspace_root = cwd.join(&p);
                            let candidate =
                                workspace_root.join(&manifest_dir).join(&manifest_basename);
                            (workspace_root, candidate)
                        })
                        .collect()
                }
            } else {
                vec![(cwd.clone(), manifest_path.clone())]
            };

            // Read all reports first (so we can also do graph output across all).
            let mut reports: Vec<(std::path::PathBuf, jhol_core::JagrAnalysisReport)> = Vec::new();
            for (root, manifest) in &roots {
                if !quiet && roots.len() > 1 {
                    info(&format!("Workspace: {}", root.display()));
                }
                // For each workspace, load the config from that workspace's root
                // directory so .jholrc settings are per-workspace. CLI flag
                // overrides (skip-layer2/3, run-all-layers) still apply globally.
                let jagr_cfg = jhol_core::load_jagr_config(root);
                let mut effective_cfg = jagr_cfg.clone();
                if cli_skip_layer3 {
                    effective_cfg.skip_layer3 = Some(true);
                }
                if cli_skip_layer2 {
                    effective_cfg.skip_layer2 = Some(true);
                }
                if cli_run_all_layers {
                    effective_cfg.run_all_layers = Some(true);
                }
                let resolver = effective_cfg.apply_to_resolver();
                let r = jhol_core::jagr_analyze_project_manifest_with_resolver(
                    manifest,
                    &resolver,
                    &effective_cfg,
                )
                .map_err(|e| e.to_string())?;
                reports.push((root.clone(), r));
            }

            // Graph export path: write the derivation graph of the first report
            // (or all of them concatenated if --all-workspaces).
            if let Some(fmt) = graph_format {
                let mut out = String::new();
                for (i, (root, r)) in reports.iter().enumerate() {
                    if i > 0 {
                        out.push_str(&format!("\n# === Workspace: {} ===\n", root.display()));
                    }
                    match fmt.as_str() {
                        "dot" => {
                            out.push_str(&format!(
                                "# JAGR derivation graph for {}\n",
                                r.manifest_path
                            ));
                            out.push_str("digraph jagr_derivation {\n");
                            out.push_str("  rankdir=LR;\n");
                            out.push_str("  node [shape=box, style=\"rounded,filled\", fillcolor=\"#eef\"];\n");
                            for (idx, node) in
                                r.derivation_nodes_for_graph_export().iter().enumerate()
                            {
                                let label = node.replace('"', "\\\"");
                                let color = match idx {
                                    _ if idx == 0 => "fillcolor=\"#cfe\"", // root decision: green
                                    _ => "fillcolor=\"#eef\"",
                                };
                                out.push_str(&format!(
                                    "  n{} [label=\"{}\", {}];\n",
                                    idx, label, color
                                ));
                            }
                            for (i, _node) in
                                r.derivation_nodes_for_graph_export().iter().enumerate()
                            {
                                for j in 0..r.derivation_edge_count_at(i) {
                                    let _ = j; // count only; actual edges not in this struct
                                }
                            }
                            // Use the count to emit a synthetic label.
                            out.push_str(&format!(
                                "  edge_count_label [label=\"{} edges (see JSON --graph json for full)\", shape=plaintext];\n",
                                r.derivation_edge_count
                            ));
                            out.push_str("}\n");
                        }
                        "mermaid" => {
                            out.push_str(&format!(
                                "%% JAGR derivation graph for {}\n",
                                r.manifest_path
                            ));
                            out.push_str("graph LR\n");
                            for (idx, node) in
                                r.derivation_nodes_for_graph_export().iter().enumerate()
                            {
                                let label =
                                    node.replace('"', "'").replace('[', "(").replace(']', ")");
                                out.push_str(&format!("  n{}[\"{}\"]\n", idx, label));
                            }
                            out.push_str(&format!(
                                "  edges[\"{} edges\"]\n",
                                r.derivation_edge_count
                            ));
                        }
                        _ => {}
                    }
                }
                if let Some(p) = output_path {
                    fs::write(&p, &out).map_err(|e| format!("Write failed: {}", e))?;
                    success(&format!("Graph written to {}.", p));
                } else {
                    println!("{}", out);
                }
                return Ok(());
            }

            // For multi-workspace, prefer the summary view.
            if roots.len() > 1 {
                // Summary table
                if use_color() {
                    println!(
                        "{}",
                        "JAGR — Multi-Workspace Three-Layer Analysis"
                            .bright_cyan()
                            .bold()
                    );
                } else {
                    println!("JAGR — Multi-Workspace Three-Layer Analysis");
                }
                println!(
                    "{:<40} {:<10} {:<8} {:<8} {:<8} {:<12} {:<10}",
                    "workspace", "ok", "pkgs", "β₁", "β₂", "strategy", "time_ms"
                );
                for (root, r) in &reports {
                    let manifest_short = root
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| r.manifest_path.clone());
                    let t = r.layer_times_ms.get("total").copied().unwrap_or(0.0);
                    println!(
                        "{:<40} {:<10} {:<8} {:<8} {:<8} {:<12} {:<10.2}",
                        manifest_short,
                        if r.is_satisfiable { "yes" } else { "no" },
                        r.package_count,
                        r.betti_1,
                        r.betti_2,
                        truncate(&r.strategy_used, 12),
                        t,
                    );
                }
                if json_out {
                    // JSON array of all reports
                    let arr: Vec<serde_json::Value> = reports.iter().map(|(_root, r)| {
                        serde_json::json!({
                            "manifest": r.manifest_path,
                            "root": { "name": r.root_name, "version": r.root_version },
                            "package_count": r.package_count,
                            "is_satisfiable": r.is_satisfiable,
                            "cost": r.cost,
                            "strategy_used": r.strategy_used,
                            "layer_times_ms": r.layer_times_ms,
                            "layer2": { "b1": r.betti_1, "b2": r.betti_2, "snf_satisfied": r.snf_satisfied },
                            "selection_size": r.selection.len(),
                        })
                    }).collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
                    );
                }
                if let Some(p) = output_path {
                    let mut buf = String::new();
                    for (root, r) in &reports {
                        buf.push_str(&format!("# {}\n", root.display()));
                        buf.push_str(&format!("strategy: {}\n", r.strategy_used));
                        buf.push_str(&format!("satisfiable: {}\n", r.is_satisfiable));
                        buf.push_str(&format!("β₁={}, β₂={}\n", r.betti_1, r.betti_2));
                        buf.push_str(&format!(
                            "time_ms: {}\n",
                            r.layer_times_ms.get("total").copied().unwrap_or(0.0)
                        ));
                        buf.push('\n');
                    }
                    fs::write(&p, &buf).map_err(|e| format!("Write failed: {}", e))?;
                    success(&format!("Summary written to {}.", p));
                }
                return Ok(());
            }

            // Single-workspace path (original behavior).
            let (_root, report) = reports.into_iter().next().unwrap();
            let _ = sub_m; // keep for potential future use
            if json_out {
                // Serialize the JagrAnalysisReport to JSON for tooling.
                let j: serde_json::Value = serde_json::json!({
                    "manifest": report.manifest_path,
                    "root": { "name": report.root_name, "version": report.root_version },
                    "package_count": report.package_count,
                    "version_pair_count": report.version_pair_count,
                    "edge_count": report.edge_count,
                    "is_satisfiable": report.is_satisfiable,
                    "cost": report.cost,
                    "strategy_used": report.strategy_used,
                    "layer_times_ms": report.layer_times_ms,
                    "layer3": {
                        "satisfiable": report.layer3_satisfiable,
                        "kind": report.layer3_kind,
                    },
                    "layer2": {
                        "betti": { "b0": report.betti_0, "b1": report.betti_1, "b2": report.betti_2 },
                        "snf_satisfied": report.snf_satisfied,
                        "snf_trivial": report.snf_trivial,
                        "snf_difficulty": report.snf_difficulty,
                        "gluing_has_obstruction": report.gluing_has_obstruction,
                    },
                    "layer1": {
                        "cost": report.layer1_cost,
                        "satisfiable": report.layer1_satisfiable,
                        "variant": report.layer1_variant,
                    },
                    "derivation": {
                        "node_count": report.derivation_node_count,
                        "edge_count": report.derivation_edge_count,
                    },
                    "selection": report.selection,
                    "explanation": report.explanation,
                });
                let s = serde_json::to_string_pretty(&j).unwrap_or_else(|_| "{}".to_string());
                if let Some(p) = output_path {
                    fs::write(&p, &s).map_err(|e| format!("Write failed: {}", e))?;
                    success(&format!("JSON written to {}.", p));
                } else {
                    println!("{}", s);
                }
            } else if quiet {
                // Minimal output
                println!("JAGR strategy: {}", report.strategy_used);
                println!("Satisfiable:   {}", report.is_satisfiable);
                let mut names: Vec<&String> = report.selection.keys().collect();
                names.sort();
                for name in names {
                    let v = report.selection.get(name).cloned().unwrap_or_default();
                    println!("  {}@{}", name, v);
                }
            } else {
                // Rich, three-layer report.
                if use_color() {
                    println!(
                        "{}",
                        "JAGR — Three-Layer Resolver Analysis".bright_cyan().bold()
                    );
                } else {
                    println!("JAGR — Three-Layer Resolver Analysis");
                }
                println!("Manifest:        {}", report.manifest_path);
                println!(
                    "Root:            {}@{}",
                    report.root_name, report.root_version
                );
                println!(
                    "Packages:        {} ({} version-pairs, {} edges)",
                    report.package_count, report.version_pair_count, report.edge_count
                );
                println!();
                // Layer 3
                dim("── Layer 3: Tropical Geometry (PTIME filter) ──");
                println!(
                    "  Result:        {} ({})",
                    if report.layer3_satisfiable {
                        "satisfiable"
                    } else {
                        "unsatisfiable"
                    },
                    report.layer3_kind
                );
                if let Some(t) = report.layer_times_ms.get("layer3") {
                    println!("  Time:          {:.2} ms", t);
                }
                println!();
                // Layer 2
                dim("── Layer 2: Tropical Sheaf Cohomology (topological filter) ──");
                println!("  β₀:            {}", report.betti_0);
                println!("  β₁:            {}", report.betti_1);
                println!("  β₂:            {}", report.betti_2);
                println!("  SNF satisfied: {}", report.snf_satisfied);
                println!("  SNF trivial:   {}", report.snf_trivial);
                println!("  Difficulty:    {}", report.snf_difficulty);
                println!(
                    "  Gluing obstruction: {}",
                    if report.gluing_has_obstruction {
                        "present"
                    } else {
                        "absent"
                    }
                );
                if let Some(t) = report.layer_times_ms.get("layer2") {
                    println!("  Time:          {:.2} ms", t);
                }
                println!();
                // Layer 1
                dim("── Layer 1: Core-Guided MaxSAT (exact solver) ──");
                println!(
                    "  Invoked:       {}",
                    report.layer1_variant != "not_invoked"
                );
                println!("  Variant:       {}", report.layer1_variant);
                println!("  Cost:          {}", report.layer1_cost);
                println!("  Satisfiable:   {:?}", report.layer1_satisfiable);
                if let Some(t) = report.layer_times_ms.get("layer1") {
                    println!("  Time:          {:.2} ms", t);
                }
                println!();
                // Strategy and selection
                dim("── Strategy & Selection ──");
                println!("  Strategy:      {}", report.strategy_used);
                println!("  Satisfiable:   {}", report.is_satisfiable);
                println!("  Total cost:    {}", report.cost);
                if let Some(t) = report.layer_times_ms.get("total") {
                    println!("  Total time:    {:.2} ms", t);
                }
                println!();
                let mut names: Vec<&String> = report.selection.keys().collect();
                names.sort();
                println!("  Selected ({}):", names.len());
                for name in names {
                    let v = report.selection.get(name).cloned().unwrap_or_default();
                    println!("    {}@{}", name, v);
                }
                println!();
                // Derivation graph
                dim("── Derivation Graph (Conflict Explainer) ──");
                println!("  Nodes:         {}", report.derivation_node_count);
                println!("  Edges:         {}", report.derivation_edge_count);
                println!();
                if !report.explanation.is_empty() {
                    dim("── Explanation ──");
                    for line in report.explanation.lines() {
                        println!("  {}", line);
                    }
                }
                if let Some(p) = output_path {
                    // Write the same rich text to the file.
                    // (We don't re-render; we just record the manifest info so users
                    // know what was written.)
                    fs::write(&p, format!("# JAGR analysis for {}\n# See `jhol jagr --json {}` for machine-readable form\n",
                        report.manifest_path, report.manifest_path))
                        .map_err(|e| format!("Write failed: {}", e))?;
                }
            }

            // --explain-each: print per-package layer decisions
            if sub_m.get_flag("explain-each") {
                let explanations = report.selection_with_explanations();
                if use_color() {
                    println!(
                        "{}",
                        "JAGR per-package layer decisions:".bright_cyan().bold()
                    );
                } else {
                    println!("JAGR per-package layer decisions:");
                }
                if explanations.is_empty() {
                    info("No packages selected.");
                } else {
                    for (name, explanation) in &explanations {
                        println!("  {:<40} {}", name, explanation);
                    }
                }
            }

            // --compare-resolvers: run JAGR + pubgrub + sat and compare.
            if sub_m.get_flag("compare-resolvers") {
                let rows =
                    jhol_core::jagr_compare_resolvers(&manifest_path).map_err(|e| e.to_string())?;
                if use_color() {
                    println!(
                        "{}",
                        "JAGR vs Legacy Resolvers — Side-by-Side"
                            .bright_cyan()
                            .bold()
                    );
                } else {
                    println!("JAGR vs Legacy Resolvers — Side-by-Side");
                }
                println!(
                    "{:<12} {:<12} {:<10} {:<10}",
                    "resolver", "time_ms", "sat", "pkgs"
                );
                for row in &rows {
                    let time_ms = row.layer_times_ms.get("total").copied().unwrap_or(0.0);
                    println!(
                        "{:<12} {:<12.4} {:<10} {:<10}",
                        row.strategy_used,
                        time_ms,
                        if row.is_satisfiable { "yes" } else { "no" },
                        row.selection.len(),
                    );
                }
            }

            // --upgrade-plan: for each declared dep, simulate a candidate
            // version change and report the topological impact.
            if sub_m.get_flag("upgrade-plan") {
                let plan =
                    jhol_core::jagr_upgrade_plan(&manifest_path).map_err(|e| e.to_string())?;
                if use_color() {
                    println!("{}", "JAGR Topological Upgrade Plan".bright_cyan().bold());
                } else {
                    println!("JAGR Topological Upgrade Plan");
                }
                println!(
                    "{:<6} {:<30} {:<14} {:<14} {:<6} {:<6} {}",
                    "risk", "package", "from", "to", "β₁→β₁", "SNF", "recommendation"
                );
                for entry in &plan {
                    let risk_str = entry.risk.as_str();
                    let snf = if entry.snf_satisfied_before != entry.snf_satisfied_after {
                        format!(
                            "{}→{}",
                            entry.snf_satisfied_before, entry.snf_satisfied_after
                        )
                    } else {
                        entry.snf_satisfied_before.to_string()
                    };
                    println!(
                        "{:<6} {:<30} {:<14} {:<14} {:<6} {:<6} {}",
                        format!("{} {}", entry.risk.emoji(), risk_str),
                        truncate(&entry.package, 30),
                        truncate(&entry.from_version, 14),
                        truncate(&entry.to_version, 14),
                        format!("{}→{}", entry.betti_1_before, entry.betti_1_after),
                        snf,
                        entry.recommendation,
                    );
                }
                // Summary footer
                let n_stable = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::SNFStable)
                    .count();
                let n_warn = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::TopologyChanged)
                    .count();
                let n_broken = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::SNFBroken)
                    .count();
                let n_unsat = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::Unsatisfiable)
                    .count();
                let n_skip = plan
                    .iter()
                    .filter(|e| e.risk == jhol_core::UpgradeRisk::Skipped)
                    .count();
                println!("\nSummary: {} SNF-stable, {} topology-changed, {} SNF-broken, {} UNSAT, {} skipped ({} total)",
                    n_stable, n_warn, n_broken, n_unsat, n_skip, plan.len());
            }

            // --dry-run-upgrade <pkg>@<version>: simulate a single upgrade
            // and report the JAGR topological impact.
            if let Some(target) = sub_m.get_one::<String>("dry-run-upgrade") {
                let (pkg, ver) = target.split_once('@').ok_or_else(|| {
                    format!("--dry-run-upgrade requires PKG@VERSION, got '{}'", target)
                })?;
                let entry = jhol_core::jagr_simulate_upgrade(&manifest_path, pkg, ver)
                    .map_err(|e| e.to_string())?;
                if use_color() {
                    println!("{}", "JAGR Dry-Run Upgrade Impact".bright_cyan().bold());
                } else {
                    println!("JAGR Dry-Run Upgrade Impact");
                }
                println!("  Package:        {}", entry.package);
                println!("  From:           {}", entry.from_version);
                println!("  To:             {}", entry.to_version);
                println!(
                    "  Risk:           {} {}",
                    entry.risk.emoji(),
                    entry.risk.as_str()
                );
                println!(
                    "  β₁:             {} → {}",
                    entry.betti_1_before, entry.betti_1_after
                );
                println!(
                    "  SNF satisfied:  {} → {}",
                    entry.snf_satisfied_before, entry.snf_satisfied_after
                );
                println!("  Recommendation: {}", entry.recommendation);
            }

            // --lockfile-only: resolve with JAGR, then write package-lock.json
            // without installing anything. The lockfile is written alongside
            // the manifest that was analyzed, NOT hard-coded to cwd.
            if lockfile_only {
                if !report.is_satisfiable {
                    return Err(format!(
                        "JAGR found no satisfying assignment; lockfile cannot be written.\n{}",
                        report.explanation
                    ));
                }
                if use_color() {
                    println!(
                        "{}",
                        "JAGR lockfile-only: resolving manifest and writing package-lock.json..."
                            .bright_cyan()
                    );
                } else {
                    println!(
                        "JAGR lockfile-only: resolving manifest and writing package-lock.json..."
                    );
                }
                // Resolve `manifest_path` to an absolute path (it can be
                // relative to cwd as passed via --manifest).
                let pj = if manifest_path.is_absolute() {
                    manifest_path.clone()
                } else {
                    cwd.join(&manifest_path)
                };
                let lock_path = pj.with_file_name("package-lock.json");
                let tree = jhol_core::resolve_full_tree(&pj)
                    .map_err(|e| format!("JAGR lockfile write failed: {}", e))?;
                jhol_core::write_package_lock(&lock_path, &pj, &tree)
                    .map_err(|e| format!("Write lockfile failed: {}", e))?;
                success(&format!(
                    "JAGR: wrote {} ({} packages resolved in {:.2}ms)",
                    lock_path.display(),
                    tree.len(),
                    report.layer_times_ms.get("total").copied().unwrap_or(0.0)
                ));
                return Ok(());
            }
        }
        _ => {
            if use_color() {
                println!("{}", "jhol".bright_cyan().bold());
                dim("Fast, offline-friendly package manager — cache first, Bun/npm backend.");
            } else {
                println!("jhol — Fast, offline-friendly package manager");
            }
            dim("\nRun `jhol --help` for details.");
        }
    }

    Ok(())
}

fn main() {
    // ── Zero-binary-startup fast path ───────────────────────────────
    // Before ANY clap parsing, check daemon status mmap + .nm_fingerprint.
    // If node_modules is fresh, exit(0) immediately — zero work.
    #[cfg(unix)]
    jhol_core::daemon_cache::try_exit_before_clap();

    if !use_color() {
        colored::control::set_override(false);
    }

    let code = match std::panic::catch_unwind(|| run()) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            print_error("", &e);
            1
        }
        Err(_) => {
            print_error("", "An unexpected error occurred. Please report this issue at https://github.com/bhuvanprakash/jhol/issues");
            1
        }
    };
    std::process::exit(code);
}
