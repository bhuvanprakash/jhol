//! Compile the Instant-Lock C front door next to cargo artifacts.
//!
//! Emits `target/{profile}/jhol-front` (~34 KB). The Rust bin remains a
//! fallback when clang/SDK is missing; install/wrap prefer `jhol-front`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src = manifest_dir.join("src/trampoline.c");
    println!("cargo:rerun-if-changed=src/trampoline.c");

    let dest = out_dir.join("jhol-c-trampoline");
    let mut cmd = Command::new("clang");
    cmd.arg("-O2").arg("-o").arg(&dest).arg(&src);
    if cfg!(target_os = "macos") {
        if let Ok(sdk) = Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-path"])
            .output()
        {
            if sdk.status.success() {
                let sdk = String::from_utf8_lossy(&sdk.stdout).trim().to_string();
                if !sdk.is_empty() {
                    cmd.arg("-isysroot").arg(&sdk);
                }
            }
        }
    }
    match cmd.status() {
        Ok(st) if st.success() => {
            // OUT_DIR = …/target/{profile}/build/{crate}-{hash}/out → profile dir is nth(3)
            if let Some(profile_dir) = out_dir.ancestors().nth(3) {
                let published = profile_dir.join("jhol-front");
                let _ = fs::copy(&dest, &published);
                println!(
                    "cargo:warning=jhol-front written to {}",
                    published.display()
                );
            }
            println!("cargo:rustc-env=JHOL_C_TRAMPOLINE={}", dest.display());
        }
        Ok(_) | Err(_) => {
            println!("cargo:warning=clang unavailable; use Rust jhol-trampoline as front door");
        }
    }
}
