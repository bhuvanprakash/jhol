use std::env;
use std::fs::{self, OpenOptions, File};
use std::io::{Write, Result};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use chrono::Local;

pub const LOG_FILE: &str = "logs.txt";

/// Returns the path to the cache directory
pub fn get_cache_dir() -> String {
    env::var("HOME").unwrap_or_else(|_| ".".to_string()) + "/.jhol-cache"
}

pub fn init_cache() -> Result<()> {
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;

    let log_path = PathBuf::from(format!("{}/{}", cache_dir, LOG_FILE));
    if !log_path.exists() {
        File::create(&log_path)?;
    }

    Ok(())
}

pub fn log(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_message = format!("[{}] {}", timestamp, message);

    println!("{}", log_message);

    let log_path = format!("{}/{}", get_cache_dir(), LOG_FILE);
    
    let mut should_write = true;
    if let Ok(contents) = fs::read_to_string(&log_path) {
        if let Some(last_line) = contents.lines().last() {
            if last_line == log_message {
                should_write = false;
            }
        }
    }

    if should_write {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            if let Err(e) = writeln!(file, "{}", log_message) {
                eprintln!("Failed to write to log file: {}", e);
            }
        } else {
            eprintln!("Failed to open log file: {}", log_path);
        }
    }
}

pub fn format_cache_name(package: &str) -> String {
    package.replace('@', "-")
}

pub fn is_package_cached(package: &str) -> bool {
    let package_cache_name = format_cache_name(package);
    Path::new(&format!("{}/{}", get_cache_dir(), package_cache_name)).exists()
}
pub fn cache_package(package: &str) {
    let cache_dir = get_cache_dir();
    let package_cache_name = format_cache_name(package);
    let package_path = format!("{}/{}", cache_dir, package_cache_name);

    if let Err(e) = fs::create_dir_all(&cache_dir) {
        eprintln!("Failed to create cache directory: {}", e);
        return;
    }

    let mut attempts = 3;
    while attempts > 0 {
        match fs::write(&package_path, "") {
            Ok(_) => {
                log(&format!("Cached package: {}", package));
                return;
            }
            Err(e) => {
                eprintln!("Attempt {}: Failed to cache package {}: {}", 4 - attempts, package, e);
                attempts -= 1;
                thread::sleep(Duration::from_secs(1));
            }
        }
    }

    eprintln!("Failed to cache package after multiple attempts: {}", package);
}
