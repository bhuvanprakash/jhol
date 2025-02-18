use clap::{Command, Arg};
use std::fs;
use std::env;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

mod install;
mod doctor;
mod utils;

fn install_globally() {
    let install_path = if cfg!(target_os = "windows") {
        format!(
            "{}\\jhol.exe",
            env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Program Files".to_string())
        )
    } else {
        "/usr/local/bin/jhol".to_string()
    };

    if !Path::new(&install_path).exists() {
        let exe_path = env::current_exe().expect("Failed to get current executable path");

        println!("🔹 Installing Jhol globally at {}", install_path);
        match fs::copy(&exe_path, &install_path) {
            Ok(_) => {
                println!("Jhol installed successfully!");

                #[cfg(unix)]
                {
                    let mut perms = fs::metadata(&install_path).unwrap().permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&install_path, perms).unwrap();
                }
            }
            Err(e) => {
                eprintln!("Failed to install Jhol globally: {}", e);
            }
        }
    }
}

fn main() {
    install_globally();

    if let Err(e) = utils::init_cache() {
        eprintln!("Failed to initialize cache: {}", e);
        std::process::exit(1);
    }

    let matches = Command::new("Jhol Free")
        .version("1.0.0")
        .author("Bhuvan Prakash <bhuvanstark6@gmail.com>")
        .about("A faster, decentralized package manager (Free Version)")
        .subcommand(
            Command::new("install")
                .about("Installs one or more packages")
                .arg(
                    Arg::new("package")
                        .required(true)
                        .num_args(1..)
                        .help("The package(s) to install"),
                ),
        )
        .subcommand(
            Command::new("doctor")
                .about("Scans and fixes package issues")
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .help("Automatically fix broken dependencies"),
                ),
        )
        .get_matches();

    match matches.subcommand() {
        Some(("install", sub_m)) => {
            if let Some(packages) = sub_m.get_many::<String>("package") {
                let packages: Vec<&str> = packages.map(|s| s.as_str()).collect();
                utils::log(&format!("Installing packages: {:?}", packages));
                install::install_package(&packages);
            } else {
                eprintln!("No package name provided. Use `jhol install <package>`.");
            }
        }
        Some(("doctor", sub_m)) => {
            if sub_m.contains_id("fix") {
                utils::log("Fixing dependencies...");
                doctor::fix_dependencies();
            } else {
                utils::log("Scanning dependencies...");
                doctor::check_dependencies();
            }
        }
        _ => {
            println!("ℹ️ Use `jhol --help` for available commands.");
        }
    }
}
