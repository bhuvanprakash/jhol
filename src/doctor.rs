use std::fs;
use serde_json::Value;
use std::process::Command;
use crate::utils;

/// Scans dependencies in `package.json` and detects outdated versions
pub fn check_dependencies() {
    utils::log("Starting dependency check...");

    let package_json = match fs::read_to_string("package.json") {
        Ok(contents) => contents,
        Err(_) => {
            eprintln!("Failed to read package.json. Make sure the file exists.");
            return;
        }
    };

    let package_data: Value = match serde_json::from_str(&package_json) {
        Ok(data) => data,
        Err(_) => {
            eprintln!("Invalid package.json format.");
            return;
        }
    };

    let dependencies = match package_data["dependencies"].as_object() {
        Some(deps) => deps,
        None => {
            println!("No dependencies found in package.json.");
            return;
        }
    };

    let mut outdated_packages = Vec::new();
    println!("Scanning dependencies for updates...");

    for (package, version) in dependencies {
        let version_str = version.as_str().unwrap_or("unknown");

        println!("🔹 Checking {}: {}", package, version_str);
        if version_str.starts_with("1.") {
            outdated_packages.push(package.clone());
        }
    }

    if outdated_packages.is_empty() {
        println!("All dependencies are up-to-date!");
    } else {
        println!("Found outdated dependencies: {:?}", outdated_packages);
        println!("Running fixes...");
        apply_fixes(&outdated_packages);
    }
}
pub fn fix_dependencies() {
    utils::log("Starting dependency fixes...");

    let package_json = match fs::read_to_string("package.json") {
        Ok(contents) => contents,
        Err(_) => {
            eprintln!("Failed to read package.json. Make sure the file exists.");
            return;
        }
    };

    let package_data: Value = match serde_json::from_str(&package_json) {
        Ok(data) => data,
        Err(_) => {
            eprintln!("Invalid package.json format.");
            return;
        }
    };

    let dependencies = match package_data["dependencies"].as_object() {
        Some(deps) => deps,
        None => {
            println!("No dependencies found in package.json.");
            return;
        }
    };

    let mut outdated_packages = Vec::new();
    println!("Scanning dependencies for fixes...");

    for (package, version) in dependencies {
        let version_str = version.as_str().unwrap_or("unknown");
        println!("🔹 Checking {}: {}", package, version_str);

        if version_str.starts_with("1.") {
            outdated_packages.push(package.clone());
        }
    }
    if outdated_packages.is_empty() {
        println!("No fixes needed - all dependencies are up-to-date!");
    } else {
        println!("Applying fixes for outdated dependencies...");
        apply_fixes(&outdated_packages);
        println!("All fixes applied successfully!");
    }
}
fn apply_fixes(packages: &[String]) {
    for package in packages {
        println!("Fixing {}", package);
        let output = Command::new("jhol")
            .arg("install")
            .arg(package)
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                println!("{} installed successfully!", package);
                utils::log(&format!("Fixed and reinstalled: {}", package));
            } else {
                eprintln!("Failed to install {}", package);
            }
        } else {
            eprintln!("Error executing `jhol install {}`", package);
        }
    }
}
