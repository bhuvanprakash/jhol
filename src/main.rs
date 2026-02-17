//! Thin CLI layer: parse args, styled output, and call into jhol-core.
//! Crash-proof: panic caught and reported; all errors return Result.

use clap::{Arg, ArgAction, Command};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::collections::HashSet;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// ---- UI helpers (no-op when stdout isn't a TTY) ----

fn use_color() -> bool {
    std::io::stdout().is_terminal()
        && env::var("NO_COLOR").unwrap_or_default().is_empty()
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
    spinner.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⠈⠐⠠⠰⠸⠹")
            .template("{spinner:.dim} {msg}").unwrap(),
    );
    spinner.set_message(message.to_string());
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
        spinner.tick();
        thread::sleep(tick);
        elapsed += tick;
    }
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
        let mut perms = fs::metadata(&install_path).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&install_path, perms).map_err(|e| e.to_string())?;
    }

    success("Jhol installed. You can run `jhol` from anywhere.");
    Ok(())
}

fn run() -> Result<(), String> {
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
                .about("Import/convert lockfile formats (initially bun.lock -> package-lock.json)")
                .arg(
                    Arg::new("from")
                        .long("from")
                        .value_parser(["auto", "bun", "npm"])
                        .default_value("auto")
                        .help("Source lockfile format"),
                ),
        )
        .subcommand(
            Command::new("global-install")
                .about("Install jhol binary to PATH (e.g. /usr/local/bin)"),
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
                .about("Run a script from package.json (no npm/Bun)")
                .arg(
                    Arg::new("script")
                        .required(true)
                        .help("Script name from package.json scripts"),
                )
                .arg(
                    Arg::new("all-workspaces")
                        .long("all-workspaces")
                        .action(ArgAction::SetTrue)
                        .help("Run in all workspace packages"),
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
                .arg(Arg::new("package").required(true).help("Package name")),
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
        .get_matches();

    // global-install (no cache needed)
    if let Some(("global-install", _)) = matches.subcommand() {
        install_globally().map_err(|e| e.to_string())?;
        return Ok(());
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
                info(&format!("Cache: {} tarball(s), {} MB", count, bytes / 1024 / 1024));
            }
            Some(("prune", sub_prune)) => {
                let keep = sub_prune.get_one::<usize>("keep").copied();
                let n = jhol_core::cache_prune(keep)
                    .map_err(|e| format!("Failed to prune cache: {}", e))?;
                success(&format!("Pruned {} tarball(s).", n));
            }
            Some(("export", sub_exp)) => {
                let dir = sub_exp.get_one::<String>("dir").map(|s| s.as_str()).unwrap();
                let n = jhol_core::cache_export(std::path::Path::new(dir))
                    .map_err(|e| format!("Export failed: {}", e))?;
                success(&format!("Exported {} package(s) to {}.", n, dir));
            }
            Some(("import", sub_imp)) => {
                let dir = sub_imp.get_one::<String>("dir").map(|s| s.as_str()).unwrap();
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
            _ => {
                dim("Use `jhol cache list`, `jhol cache size`, `jhol cache prune`, `jhol cache export <dir>`, `jhol cache import <dir>`, `jhol cache clean`, `jhol cache telemetry`, or `jhol cache key`.");
            }
        }
        return Ok(());
    }

    if let Some(("import-lock", sub_m)) = matches.subcommand() {
        let from = sub_m.get_one::<String>("from").map(|s| s.as_str()).unwrap_or("auto");
        let msg = jhol_core::import_lockfile(from).map_err(|e| e.to_string())?;
        success(&msg);
        return Ok(());
    }

    if let Some(("run", sub_m)) = matches.subcommand() {
        let script_name = sub_m.get_one::<String>("script").unwrap().as_str();
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
            let status = jhol_core::run_script(script_name, root, None)
                .map_err(|e| e.to_string())?;
            last_code = status.code().unwrap_or(1);
        }
        std::process::exit(last_code);
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
        let bin_path = jhol_core::find_binary_in_node_modules(binary, &cwd)
            .ok_or_else(|| format!("Binary \"{}\" not found in node_modules/.bin. Run jhol install first.", binary))?;
        let status = jhol_core::exec_binary(&bin_path, &args, &cwd)
            .map_err(|e| e.to_string())?;
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
            return Err("Use --save to update package.json when updating specific packages".to_string());
        }
        success("Update complete.");
        return Ok(());
    }

    if let Some(("why", sub_m)) = matches.subcommand() {
        let package = sub_m.get_one::<String>("package").unwrap();
        let paths = jhol_core::why_package(package).map_err(|e| e.to_string())?;
        for path in paths {
            println!("{}", path);
        }
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
        let backend = match sub_m.get_one::<String>("backend") {
            Some(s) if s == "bun" => Some(jhol_core::Backend::Bun),
            Some(s) if s == "npm" => Some(jhol_core::Backend::Npm),
            _ => config.backend,
        };
        let backend = jhol_core::resolve_backend(backend);
        let json_out = sub_m.get_flag("json");
        let roots: Vec<std::path::PathBuf> = if all_workspaces {
            let r = jhol_core::list_workspace_roots(&cwd).unwrap_or_default();
            if r.is_empty() { vec![cwd.clone()] } else { r.into_iter().map(|p| cwd.join(p)).collect() }
        } else {
            vec![cwd.clone()]
        };
        for root in &roots {
            std::env::set_current_dir(root).map_err(|e| format!("chdir {}: {}", root.display(), e))?;
            let specs = jhol_core::resolve_install_from_package_json(true)?;
            let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
            let opts = jhol_core::InstallOptions {
                no_cache: false,
                quiet,
                backend,
                lockfile_only: false,
                offline,
                strict_lockfile: true,
                from_lockfile: true,
                native_only: true,
                no_scripts: true,
                script_allowlist: None,
            };
            jhol_core::install_package(&refs, &opts)?;
        }
        std::env::set_current_dir(&cwd).ok();
        if json_out {
            println!("{{\"schemaVersion\":\"1\",\"command\":\"ci\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
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

    match matches.subcommand() {
        Some(("install", sub_m)) => {
            let no_cache = sub_m.get_flag("no-cache");
            let quiet = sub_m.get_flag("quiet");
            let lockfile_only = sub_m.get_flag("lockfile-only");
            let offline = sub_m.get_flag("offline")
                || env::var("JHOL_OFFLINE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
            let strict_lockfile = sub_m.get_flag("frozen");
            let native_only = sub_m.get_flag("native-only") || !sub_m.get_flag("fallback-backend");
            let all_workspaces = sub_m.get_flag("all-workspaces");
            let no_scripts = !sub_m.get_flag("scripts") || sub_m.get_flag("no-scripts");
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
                std::env::set_current_dir(root).map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                if !quiet && roots.len() > 1 {
                    info(&format!("Workspace: {}", root.display()));
                }
                if packages.is_empty() && lockfile_only {
                    jhol_core::install_lockfile_only(backend)?;
                    continue;
                }
                let specs: Vec<String> = if packages.is_empty() {
                    jhol_core::resolve_install_from_package_json(strict_lockfile)?
                } else {
                    packages.iter().map(|s| s.clone()).collect()
                };
                if specs.is_empty() {
                    if roots.len() == 1 {
                        dim("No dependencies to install.");
                    }
                    continue;
                }
                let refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
                let from_lockfile = packages.is_empty()
                    && jhol_core::read_resolved_from_dir(std::path::Path::new(".")).is_some();
                let opts = jhol_core::InstallOptions {
                    no_cache,
                    quiet,
                    backend,
                    lockfile_only,
                    offline,
                    strict_lockfile,
                    from_lockfile,
                    native_only,
                    no_scripts,
                    script_allowlist: script_allowlist.clone(),
                };
                jhol_core::log(&format!("Installing: {:?}", specs));
                jhol_core::install_package(&refs, &opts)?;
            }
            std::env::set_current_dir(&cwd).ok();
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
                std::env::set_current_dir(root).map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                if !quiet && !json_out && roots.len() > 1 {
                    info(&format!("Workspace: {}", root.display()));
                }
                if do_fix {
                    jhol_core::log("Running doctor --fix");
                    jhol_core::fix_dependencies(quiet, backend)?;
                } else {
                    jhol_core::log("Running doctor (check only)");
                    jhol_core::check_dependencies(quiet, backend)?;
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
            let gate = sub_m.get_flag("gate");
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
            if gate {
                jhol_core::run_audit_gate(backend)?;
                if json_out {
                    println!("{{\"schemaVersion\":\"1\",\"command\":\"audit\",\"status\":\"ok\",\"gate\":true}}");
                }
            } else if json_out && !do_fix {
                let json_bytes = jhol_core::run_audit_raw(backend).map_err(|e| e.to_string())?;
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
                    std::env::set_current_dir(root).map_err(|e| format!("chdir {}: {}", root.display(), e))?;
                    if !quiet && !json_out && roots.len() > 1 {
                        info(&format!("Workspace: {}", root.display()));
                    }
                    if do_fix {
                        jhol_core::run_audit_fix(backend, quiet)?;
                    } else {
                        jhol_core::run_audit(backend, quiet)?;
                    }
                }
                std::env::set_current_dir(&cwd).ok();
                if json_out {
                    println!("{{\"schemaVersion\":\"1\",\"command\":\"audit\",\"status\":\"ok\",\"workspaces\":{}}}", roots.len());
                } else if quiet {
                    success("Done.");
                }
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
    if !use_color() {
        colored::control::set_override(false);
    }

    let code = match std::panic::catch_unwind(|| run()) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            error(&e);
            1
        }
        Err(_) => {
            error("An unexpected error occurred. Please report this issue.");
            1
        }
    };
    std::process::exit(code);
}
