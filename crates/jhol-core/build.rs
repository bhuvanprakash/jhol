//! Build script to populate bundled packages with actual tarball data
//!
//! This script downloads the top 100 npm packages at build time
//! and embeds them directly into the binary for instant installs.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../binary-packages/index.json");

    // Check if binary-packages directory exists with pre-populated data
    let binary_packages_dir = workspace_binary_packages_dir();
    if binary_packages_dir.exists() {
        println!("cargo:warning=Binary packages directory found - using pre-cached data");

        // Generate bundled packages data from binary-packages
        generate_bundled_packages_from_binary(&binary_packages_dir);
        return;
    }

    // Fallback: generate stub data for development
    println!("cargo:warning=No binary-packages directory - generating stub data");
    generate_stub_bundled_packages();
}

fn workspace_binary_packages_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("binary-packages"))
        .unwrap_or_else(|| PathBuf::from("binary-packages"))
}

fn generate_bundled_packages_from_binary(binary_dir: &Path) {
    let index_path = binary_dir.join("index.json");
    if !index_path.exists() {
        println!("cargo:warning=No index.json found - using stub data");
        generate_stub_bundled_packages();
        return;
    }

    // Read index
    let index_content = fs::read_to_string(&index_path).expect("Failed to read index.json");
    let index: serde_json::Value =
        serde_json::from_str(&index_content).expect("Failed to parse index.json");

    // Generate Rust code for bundled packages
    let mut rust_code = String::new();
    rust_code.push_str("use std::collections::HashMap;\n\n");
    rust_code.push_str("pub fn get_bundled_package_data() -> HashMap<String, Box<[u8]>> {\n");
    rust_code.push_str("    let mut packages = HashMap::new();\n");

    if let Some(obj) = index.as_object() {
        for (pkg_spec, hash_value) in obj {
            let hash = hash_value.as_str().unwrap_or("");
            let bin_path = binary_dir.join(format!("{}.jhol", hash));

            if bin_path.exists() {
                let escaped_path = bin_path.display().to_string().replace('\\', "\\\\");
                rust_code.push_str(&format!(
                    "    packages.insert(\"{}\".to_string(), Box::<[u8]>::from(&include_bytes!(r#\"{}\"#)[..]));\n",
                    pkg_spec, escaped_path
                ));
            }
        }
    }

    rust_code.push_str("    packages\n");
    rust_code.push_str("}\n");

    // Write generated code
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("bundled_packages_data.rs");
    fs::write(&dest_path, rust_code).expect("Failed to write generated code");
}

fn generate_stub_bundled_packages() {
    // Generate stub data for development (no actual tarballs)
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("bundled_packages_data.rs");

    let rust_code = r#"
use std::collections::HashMap;

pub fn get_bundled_package_data() -> HashMap<String, Box<[u8]>> {
    let mut packages = HashMap::new();

    // Stub data - actual tarballs will be populated when binary-packages/ exists
    // This allows compilation without network access

    packages
}
"#;

    fs::write(&dest_path, rust_code).expect("Failed to write stub code");
}
