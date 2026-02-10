use clap::{Arg, ArgAction, Command};
use std::env;
use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

mod doctor;
mod install;
mod lockfile;
mod registry;
mod utils;

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
        println!("Jhol is already installed at {}", install_path);
        return Ok(());
    }

    let exe_path = env::current_exe().map_err(|e| e.to_string())?;
    println!("Installing Jhol globally at {} ...", install_path);
    fs::copy(&exe_path, &install_path).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&install_path).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&install_path, perms).map_err(|e| e.to_string())?;
    }

    println!("Jhol installed successfully. You can run `jhol` from anywhere.");
    Ok(())
}

fn main() {
    let matches = Command::new("jhol")
        .version("1.0.0")
        .author("Bhuvan Prakash <bhuvanstark6@gmail.com>")
        .about("A fast, offline-friendly package manager (npm-compatible)")
        .subcommand(
            Command::new("install")
                .about("Install one or more packages")
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
                        .help("Less output"),
                ),
        )
        .subcommand(
            Command::new("doctor")
                .about("Check and fix dependency issues")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .action(ArgAction::SetTrue)
                        .help("Update outdated dependencies"),
                )
                .arg(
                    Arg::new("quiet")
                        .short('q')
                        .long("quiet")
                        .action(ArgAction::SetTrue)
                        .help("Less output"),
                ),
        )
        .subcommand(Command::new("global-install").about("Install jhol binary to PATH (e.g. /usr/local/bin)"))
        .subcommand(
            Command::new("cache")
                .about("Manage local package cache")
                .subcommand(Command::new("list").about("List cached packages"))
                .subcommand(Command::new("clean").about("Remove all cached tarballs")),
        )
        .get_matches();

    if let Some(("global-install", _)) = matches.subcommand() {
        if let Err(e) = install_globally() {
            eprintln!("{}", e);
            std::process::exit(1);
        }
        return;
    }

    if let Some(("cache", sub)) = matches.subcommand() {
        if let Err(e) = utils::init_cache() {
            eprintln!("Failed to initialize cache: {}", e);
            std::process::exit(1);
        }
        match sub.subcommand() {
            Some(("list", _)) => {
                match utils::list_cached_packages() {
                    Ok(list) => {
                        if list.is_empty() {
                            println!("No cached packages.");
                        } else {
                            println!("Cached packages ({}):", list.len());
                            for name in list {
                                println!("  {}", name);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to list cache: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Some(("clean", _)) => {
                match utils::cache_clean() {
                    Ok(n) => println!("Removed {} cached package(s).", n),
                    Err(e) => {
                        eprintln!("Failed to clean cache: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            _ => {
                println!("Use `jhol cache list` or `jhol cache clean`.");
            }
        }
        return;
    }

    if let Err(e) = utils::init_cache() {
        eprintln!("Failed to initialize cache: {}", e);
        std::process::exit(1);
    }

    match matches.subcommand() {
        Some(("install", sub_m)) => {
            let no_cache = sub_m.get_flag("no-cache");
            let quiet = sub_m.get_flag("quiet");
            if quiet {
                env::set_var("JHOL_QUIET", "1");
            }
            let opts = install::InstallOptions { no_cache, quiet };
            let packages: Vec<&str> = sub_m
                .get_many::<String>("package")
                .map(|it| it.map(|s| s.as_str()).collect())
                .unwrap_or_default();
            let specs: Vec<String> = if packages.is_empty() {
                match install::resolve_install_from_package_json() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                packages.iter().map(|s| (*s).to_string()).collect()
            };
            let spec_refs: Vec<&str> = specs.iter().map(|s| s.as_str()).collect();
            utils::log(&format!("Installing: {:?}", spec_refs));
            if let Err(e) = install::install_package(&spec_refs, &opts) {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
        Some(("doctor", sub_m)) => {
            let quiet = sub_m.get_flag("quiet");
            if quiet {
                env::set_var("JHOL_QUIET", "1");
            }
            if sub_m.get_flag("fix") {
                utils::log("Running doctor --fix");
                if let Err(e) = doctor::fix_dependencies(quiet) {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            } else {
                utils::log("Running doctor (check only)");
                if let Err(e) = doctor::check_dependencies(quiet) {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            println!("Usage: jhol <command> [options]");
            println!("Commands: install, doctor, cache, global-install");
            println!("Run `jhol --help` for details.");
        }
    }
}
