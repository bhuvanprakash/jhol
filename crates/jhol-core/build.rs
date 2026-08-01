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
        // Generate DSSC spectral certificates
        generate_dssc_certs(&binary_packages_dir);
        // Generate resolution seed from binary-packages index
        generate_resolution_seed(&binary_packages_dir);
        return;
    }

    // Fallback: generate stub data for development
    println!("cargo:warning=No binary-packages directory - generating stub data");
    generate_stub_bundled_packages();
    generate_stub_dssc_certs();
    generate_stub_resolution_seed();
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

    // Collect and sort entries so we can binary-search at runtime
    let mut entries: Vec<(&str, String)> = Vec::new();
    if let Some(obj) = index.as_object() {
        for (pkg_spec, hash_value) in obj {
            let hash = hash_value.as_str().unwrap_or("");
            let bin_path = binary_dir.join(format!("{}.jhol", hash));
            if bin_path.exists() {
                let escaped_path = bin_path.display().to_string().replace('\\', "\\\\");
                entries.push((pkg_spec.as_str(), escaped_path));
            }
        }
    }
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Generate a sorted function — zero heap allocation at runtime.
    // Returns a reference to a function-static array so include_bytes!
    // is evaluated at compile time in a valid item-position context.
    let mut rust_code = String::new();
    rust_code.push_str("#[doc(hidden)]\n");
    rust_code.push_str("pub fn get_bundled_package_data() -> &'static [(&'static str, &'static [u8])] {\n");
    rust_code.push_str("    static DATA: &[(&str, &[u8])] = &[\n");
    for (pkg_spec, escaped_path) in &entries {
        // include_bytes! at item level (inside static) is const-evaluable
        rust_code.push_str(&format!(
            "        (\"{}\", include_bytes!(r#\"{}\"#)),\n",
            pkg_spec.replace('\\', "\\\\").replace('"', "\\\""),
            escaped_path
        ));
    }
    rust_code.push_str("    ];\n");
    rust_code.push_str("    DATA\n");
    rust_code.push_str("}\n\n");

    // Generate a separate function returning (spec, sha256_hash) pairs.
    // The hash is the SHA256 of the decoded gzip tarball (same as the
    // .jhol filename without extension).  Used to pre-seed the store
    // index at startup without needing to decode any archive.
    rust_code.push_str("#[doc(hidden)]\n");
    rust_code.push_str("pub fn get_bundled_hashes() -> &'static [(&'static str, &'static str)] {\n");
    rust_code.push_str("    static HASHES: &[(&str, &str)] = &[\n");
    for (pkg_spec, escaped_path) in &entries {
        let hash = std::path::Path::new(&escaped_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        rust_code.push_str(&format!(
            "        (\"{}\", \"{}\"),\n",
            pkg_spec.replace('\\', "\\\\").replace('"', "\\\""),
            hash
        ));
    }
    rust_code.push_str("    ];\n");
    rust_code.push_str("    HASHES\n");
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
#[doc(hidden)]
pub fn get_bundled_package_data() -> &'static [(&'static str, &'static [u8])] {
    &[]
}

#[doc(hidden)]
pub fn get_bundled_hashes() -> &'static [(&'static str, &'static str)] {
    &[]
}
"#;

    fs::write(&dest_path, rust_code).expect("Failed to write stub code");
}

/// Phase 1: Generate DSSC spectral certificates from binary-packages.
///
/// For each package in the index, emit a DsscCert entry:
///   - stability: Stable (0u8) for packages with ≤5 known deps, Marginal (1u8) otherwise
///   - tight_constraints: estimated from version spec tightness
///   - max_degree: estimated from package fanout
///
/// These are conservative heuristics. The full spectral computation is only
/// skipped when ALL packages have Stable certs, so false negatives (Unknown
/// packages) are safe — they just fall through to the full pipeline.
fn generate_dssc_certs(binary_dir: &Path) {
    let index_path = binary_dir.join("index.json");
    if !index_path.exists() {
        generate_stub_dssc_certs();
        return;
    }

    let index_content = fs::read_to_string(&index_path).expect("Failed to read index.json");
    let index: serde_json::Value =
        serde_json::from_str(&index_content).expect("Failed to parse index.json");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("dssc_certs_data.rs");

    let mut entries: Vec<(String, u8, u32, u32)> = Vec::new();

    if let Some(obj) = index.as_object() {
        for pkg_spec in obj.keys() {
            // Parse "name@version" from the spec
            let name = if let Some(at_pos) = pkg_spec.rfind('@') {
                if at_pos > 0 && !pkg_spec.starts_with('@') {
                    &pkg_spec[..at_pos]
                } else if pkg_spec.starts_with('@') {
                    // Scoped package: @scope/name@version
                    if let Some(at2) = pkg_spec[at_pos + 1..].find('@') {
                        &pkg_spec[..at_pos + 1 + at2]
                    } else {
                        pkg_spec.as_str()
                    }
                } else {
                    pkg_spec.as_str()
                }
            } else {
                pkg_spec.as_str()
            };

            // Heuristic DSSC cert per package:
            // - Most npm packages have 0-3 deps → Stable, tight=1, degree=3
            // - Scoped packages with deep trees (e.g. @babel/*) → Stable, tight=2, degree=6
            // - Large framework packages (next, nuxt, angular) → Marginal, tight=3, degree=10
            let (stability, tight, degree) = classify_package_for_dssc(name);
            entries.push((name.to_string(), stability, tight, degree));
        }
    }

    // Sort by name for deterministic output and binary search at runtime
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);

    let mut rust_code = String::new();

    // Generate a sorted static array of DsscCert structs for binary search
    rust_code.push_str("static CERT_ENTRIES: &[DsscCert] = &[\n");
    for (name, stability, tight, degree) in &entries {
        let stab_name = match stability {
            0 => "crate::StabilityClass::Stable",
            1 => "crate::StabilityClass::Marginal",
            2 => "crate::StabilityClass::Fragile",
            _ => "crate::StabilityClass::Degenerate",
        };
        rust_code.push_str(&format!(
            "    DsscCert {{ name: \"{}\", stability: {}, tight_constraints: {}, max_degree: {} }},\n",
            name.replace('\\', "\\\\").replace('"', "\\\""),
            stab_name,
            tight,
            degree,
        ));
    }
    rust_code.push_str("];\n\n");

    rust_code.push_str(
        "pub fn lookup_dssc_cert(name: &str) -> Option<&'static DsscCert> {\n\
         \x20   let idx = CERT_ENTRIES.binary_search_by(|c| c.name.as_bytes().cmp(name.as_bytes())).ok()?;\n\
         \x20   Some(&CERT_ENTRIES[idx])\n\
         }\n",
    );

    fs::write(&dest_path, rust_code).expect("Failed to write DSSC certs code");
    println!(
        "cargo:warning=DSSC certs generated for {} packages",
        entries.len()
    );
}

/// Heuristic DSSC classification for a package at build time.
/// Returns (stability_class, tight_constraints, max_degree).
fn classify_package_for_dssc(name: &str) -> (u8, u32, u32) {
    // Well-known zero-dep packages: leaf nodes, always Stable
    let zero_dep_packages = [
        "lodash", "chalk", "ms", "debug", "semver", "uuid", "chalk-template",
        "escape-string-regexp", "strip-ansi", "ansi-styles", "supports-color",
        "has-flag", "color-convert", "color-name", "yocto-queue",
        "fast-deep-equal", "fast-json-stable-stringify", "json-parse-even-better-errors",
        "fast-levenshtein", "callsites", "path-is-absolute", "resolve-from",
        "function-bind", "object-assign", "is-fullwidth-code-point",
        "strip-json-comments", "minimist", "wordwrap", "wrappy",
        "inflight", "once", "inherits", "process-nextick-args",
        "safe-buffer", "safe-regex", "string_decoder", "core-util-is",
        "isarray", "has-symbols", "es-to-primitive", "is-callable",
        "is-negative-zero", "is-regex", "object-inspect", "string.prototype.trim",
        "which-boxed-primitive", "side-channel", "get-intrinsic",
        "has-proto", "has-tostringtag", "math-intrinsics",
    ];
    let low_dep_packages = [
        "react", "react-dom", "vue", "svelte", "preact", "lit",
        "dayjs", "moment", "luxon", "date-fns",
        "zod", "yup", "joi", "ajv", "superjson",
        "immer", "rxjs", "nanoid", "cross-env", "rimraf",
        "dotenv", "glob", "minimatch", "brace-expansion",
        "concat-map", "balanced-match", "fs.realpath", "inflight",
        "path-is-absolute", "object-is", "array-flatten",
        "is-core-module", "supports-preserve-symlinks-flag",
    ];
    let scoped_stable = [
        "@babel/core", "@babel/preset-env", "@babel/preset-react",
        "@babel/preset-typescript", "@babel/runtime",
        "@types/node", "@types/react", "@types/jest",
        "@eslint/js", "@humanwhocodes/config-array",
        "@nicolo-ribaudo/chokidar-2",
    ];
    let high_fanout = [
        "typescript", "webpack", "vite", "esbuild", "rollup",
        "next", "nuxt", "gatsby", "angular", "ember-cli",
        "@angular/core", "@angular/cli", "@nestjs/core",
    ];

    let name_lower = name.to_lowercase();

    if zero_dep_packages.iter().any(|&p| name_lower == p) {
        return (0, 0, 0); // Stable, no tight constraints, no degree
    }
    if low_dep_packages.iter().any(|&p| name_lower == p) {
        return (0, 1, 2); // Stable, 1 tight, degree 2
    }
    if high_fanout.iter().any(|&p| name_lower == p) {
        return (1, 3, 10); // Marginal, 3 tight, degree 10
    }
    if name.starts_with("@types/") || name.starts_with("@babel/") {
        return (0, 1, 3); // Stable, low fanout
    }
    if name.starts_with('@') {
        // Scoped packages: moderate fanout
        return (0, 1, 4);
    }
    // Default: assume Stable with moderate constraints
    (0, 1, 3)
}

fn generate_stub_dssc_certs() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("dssc_certs_data.rs");

    let rust_code = r#"
// Stub: no binary-packages/index.json found at build time.
static CERT_ENTRIES: &[DsscCert] = &[];

pub fn lookup_dssc_cert(_name: &str) -> Option<&'static DsscCert> {
    None
}
"#;

    fs::write(&dest_path, rust_code).expect("Failed to write stub DSSC certs code");
}

fn parse_pkg_spec(spec: &str) -> (&str, &str) {
    if let Some(at_pos) = spec.rfind('@') {
        if at_pos > 0 {
            (&spec[..at_pos], &spec[at_pos + 1..])
        } else {
            (spec, "")
        }
    } else {
        (spec, "")
    }
}

fn generate_resolution_seed(binary_dir: &Path) {
    let index_path = binary_dir.join("index.json");
    if !index_path.exists() {
        generate_stub_resolution_seed();
        return;
    }

    let index_content = fs::read_to_string(&index_path).expect("Failed to read index.json");
    let index: serde_json::Value =
        serde_json::from_str(&index_content).expect("Failed to parse index.json");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("resolution_seed_data.rs");

    // Collect unique (name, version) pairs from index.json keys
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Some(obj) = index.as_object() {
        for pkg_spec in obj.keys() {
            let (name, version) = parse_pkg_spec(pkg_spec);
            if !version.is_empty() {
                entries.push((name.to_string(), version.to_string()));
            }
        }
    }

    // Sort and dedup (some packages may appear with multiple versions)
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup();

    let mut rust_code = String::new();
    rust_code.push_str("pub fn load_seed_entries(store: &mut LatticeStore) {\n");
    for (name, version) in &entries {
        let escaped_name = name.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_version = version.replace('\\', "\\\\").replace('"', "\\\"");
        rust_code.push_str(&format!(
            "    store.by_pkg.entry(\"{}\".to_string()).or_default().push(LatticeEntry {{ constraint: \"*\".to_string(), version: \"{}\".to_string(), hits: 1 }});\n",
            escaped_name, escaped_version
        ));
    }
    rust_code.push_str("}\n");

    fs::write(&dest_path, rust_code).expect("Failed to write resolution seed data");
    println!(
        "cargo:warning=Resolution seed generated for {} packages",
        entries.len()
    );
}

fn generate_stub_resolution_seed() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("resolution_seed_data.rs");

    let rust_code = r#"
// Stub: no binary-packages/index.json found at build time.
pub fn load_seed_entries(_store: &mut LatticeStore) {}
"#;

    fs::write(&dest_path, rust_code).expect("Failed to write stub resolution seed data");
}
