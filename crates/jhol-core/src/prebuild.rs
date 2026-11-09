//! Prebuild-install equivalent: fetch prebuilt native binaries from GitHub
//! releases (or a configured mirror) into the store dir, without executing any
//! lifecycle scripts.
//!
//! npm packages that ship N-API/node ABI prebuilds declare a
//! `prebuild-install` line in `scripts.install`/`scripts.postinstall` and a
//! `binary` config (e.g. `sqlite3`, `kerberos`, `@mongodb-js/zstd`). Running
//! that script would download a tarball and unpack it next to `package.json`.
//! Jhol does not run lifecycle scripts by default, so the native `.node`
//! binary would be missing and `require()` fails at runtime. This module
//! replicates prebuild-install's download URL construction and unpacks the
//! prebuilt binary directly into the store dir, keeping the process
//! deterministic, offline-friendly, and script-free.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Marker file placed in a store dir after a prebuilt binary for the current
/// platform/arch/abi is present. Guards against re-fetching on every run and
/// detects when a shared cache was populated by a different platform.
fn marker_name(platform: &str, arch: &str, abi: &str) -> String {
    format!(".jhol-prebuild-{}-{}-{}", platform, arch, abi)
}

/// Probe node once for the runtime ABI (process.versions.modules) and N-API
/// version (process.versions.napi). Returns None when node is unavailable or
/// the probe fails — in that case prebuilds are skipped (the package installs
/// without a native binary, same as today).
fn node_runtime_info() -> Option<(String, String)> {
    static INFO: OnceLock<Option<(String, String)>> = OnceLock::new();
    INFO.get_or_init(|| {
        if let (Ok(abi), Ok(napi)) = (
            std::env::var("JHOL_NODE_ABI"),
            std::env::var("JHOL_NODE_NAPI"),
        ) {
            return Some((abi, napi));
        }
        let out = std::process::Command::new("node")
            .arg("-p")
            .arg("process.versions.modules + ' ' + (process.versions.napi || '')")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let mut parts = stdout.split_whitespace();
        let abi = parts.next()?.to_string();
        let napi = parts.next().unwrap_or("").to_string();
        Some((abi, napi))
    })
    .clone()
}

/// npm platform string (process.platform) for the current host.
fn npm_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// npm arch string (process.arch) for the current host.
fn npm_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    }
}

/// libc suffix for the URL template. Empty on macOS/Windows and on glibc
/// Linux; `musl` on musl Linux (matches prebuild-install's detect-libc).
fn npm_libc() -> String {
    if std::env::consts::OS != "linux" {
        return String::new();
    }
    if std::env::var("LIBC")
        .map(|v| v.eq_ignore_ascii_case("musl"))
        .unwrap_or(false)
        || std::env::var("npm_config_libc")
            .map(|v| v.eq_ignore_ascii_case("musl"))
            .unwrap_or(false)
        || Path::new("/etc/ld-musl-x86_64.so.1").exists()
        || Path::new("/etc/ld-musl-aarch64.so.1").exists()
    {
        return "musl".to_string();
    }
    String::new()
}

/// Replicate github-from-package: extract `https://github.com/owner/repo`
/// from the `repository` field (falling back to scanning the whole manifest).
fn github_from_package(pkg: &Value) -> Option<String> {
    let repository = pkg.get("repository");
    let candidates: Vec<String> = if let Some(repo) = repository {
        vec![repo.to_string()]
    } else {
        vec![]
    };
    // JSON.stringify(pkg) is the fallback in the original; emulate cheaply by
    // also scanning the full serialized manifest when repository is absent.
    let full = if repository.is_some() {
        String::new()
    } else {
        pkg.to_string()
    };
    let mut candidates = candidates;
    if !full.is_empty() {
        candidates.push(full);
    }
    for c in candidates {
        // Pattern: github.com/<owner>/<repo> with either `/` or `:` separator,
        // repo may end in `.git`. Matches the original regex
        // /\bgithub.com[:\/]([^\/"]+)\/([^\/"]+)/
        let c = c.replace("\\/", "/");
        for needle in ["github.com:", "github.com/", "github.com\\"] {
            if let Some(idx) = c.find(needle) {
                let rest = &c[idx + needle.len()..];
                let rest = rest.trim_start_matches(|ch| ch == '/' || ch == ':');
                let owner = rest.split(['/', ':', '"']).next().unwrap_or("").to_string();
                if owner.is_empty() {
                    continue;
                }
                let remainder = &rest[owner.len()..];
                let remainder = remainder
                    .strip_prefix('/')
                    .or_else(|| remainder.strip_prefix(':'))
                    .unwrap_or(remainder);
                let repo = remainder
                    .split(['/', '"', '\\'])
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(".git")
                    .to_string();
                if repo.is_empty() {
                    continue;
                }
                return Some(format!("https://github.com/{}/{}", owner, repo));
            }
        }
    }
    None
}

/// Replace `{token}` templates in a URL string with values from the package.
fn expand_template(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in values {
        let placeholder = format!("{{{}}}", k);
        out = out.replace(&placeholder, v);
    }
    out
}

/// Returns true when the package.json declares a prebuild-install invocation
/// in an install-ish script, plus the runtime token (`node` or `napi`).
fn uses_prebuild_install(pkg: &Value) -> Option<String> {
    let scripts = pkg.get("scripts")?;
    for key in ["install", "postinstall", "preinstall"] {
        if let Some(Value::String(script)) = scripts.get(key) {
            if script.contains("prebuild-install") {
                // Detect `-r napi` / `--runtime napi` / `--runtime=napi` args.
                let runtime = if script.contains("-r napi")
                    || script.contains("--runtime napi")
                    || script.contains("--runtime=napi")
                {
                    "napi".to_string()
                } else if script.contains("-r node")
                    || script.contains("--runtime node")
                    || script.contains("--runtime=node")
                {
                    "node".to_string()
                } else {
                    // pkg.config.runtime is a rare override prebuild-install honours.
                    pkg.get("config")
                        .and_then(|c| c.get("runtime"))
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "node".to_string())
                };
                return Some(runtime);
            }
        }
    }
    None
}

/// Resolve the effective ABI used in the prebuild URL template.
///
/// For the `node` runtime this is `process.versions.modules` (e.g. 147).
/// For the `napi` runtime prebuild-install picks the best supported N-API
/// build version: the largest `binary.napi_versions` <= the running node's
/// N-API version (e.g. sqlite3 advertises [3,6] and node ships N-API 10 → 6).
/// Returns None when no usable ABI can be determined.
fn resolve_abi(pkg: &Value, runtime: &str, node_abi: &str) -> Option<String> {
    if runtime != "napi" {
        return Some(node_abi.to_string());
    }
    let napi_versions = pkg
        .get("binary")
        .and_then(|b| b.get("napi_versions"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_i64().map(|i| i as i32))
                .collect::<Vec<i32>>()
        });
    let our_napi = node_abi.parse::<i32>().unwrap_or(0);
    let best = napi_versions
        .unwrap_or_default()
        .into_iter()
        .filter(|&v| v <= our_napi)
        .max()
        .unwrap_or(0);
    if best == 0 {
        return None;
    }
    Some(best.to_string())
}

/// Compute the prebuilt-binary download URL for a package manifest.
/// Returns None when no URL can be determined (no github repo, no host, etc.).
fn prebuild_url_for_manifest(pkg: &Value, runtime: &str) -> Option<String> {
    let name = pkg.get("name")?.as_str()?.to_string();
    let pkg_name = name.split('/').last().unwrap_or(&name).to_string();
    let version = pkg.get("version")?.as_str()?.to_string();
    let (node_abi, _) = node_runtime_info()?;
    let platform = npm_platform();
    let arch = npm_arch();
    let libc = npm_libc();
    let abi = resolve_abi(pkg, runtime, &node_abi)?;

    let package_name = format!(
        "{}-v{}-{}-v{}-{}{}-{}.tar.gz",
        pkg_name, version, runtime, abi, platform, libc, arch
    );

    let binary = pkg.get("binary");
    let host = binary.and_then(|b| b.get("host")).and_then(|h| h.as_str());

    let template: String = if let Some(host) = host {
        let remote_path = binary
            .and_then(|b| b.get("remote_path"))
            .and_then(|r| r.as_str())
            .unwrap_or("");
        let package_name_override = binary
            .and_then(|b| b.get("package_name"))
            .and_then(|p| p.as_str())
            .unwrap_or(&package_name);
        [host, remote_path, package_name_override]
            .iter()
            .map(|s| s.trim_matches('/'))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    } else {
        let repo = github_from_package(pkg)?;
        format!(
            "{}/releases/download/{{tag_prefix}}{{version}}/{}",
            repo, package_name
        )
    };

    let values = [
        ("name", pkg_name.as_str()),
        ("package_name", pkg_name.as_str()),
        ("version", version.as_str()),
        ("major", version.split('.').next().unwrap_or("")),
        ("minor", version.split('.').nth(1).unwrap_or("")),
        ("patch", version.split('.').nth(2).unwrap_or("")),
        ("prerelease", version.split('-').nth(1).unwrap_or("")),
        ("build", version.split('+').nth(1).unwrap_or("")),
        ("abi", abi.as_str()),
        ("node_abi", node_abi.as_str()),
        ("runtime", runtime),
        ("platform", platform),
        ("arch", arch),
        ("libc", libc.as_str()),
        ("configuration", "Release"),
        (
            "module_name",
            binary
                .and_then(|b| b.get("module_name"))
                .and_then(|m| m.as_str())
                .unwrap_or(""),
        ),
        ("tag_prefix", "v"),
    ];
    Some(expand_template(&template, &values))
}

/// Cache directory for downloaded prebuild tarballs, keyed by URL digest so
/// different platforms/abis never collide.
fn prebuild_cache_path(cache_dir: &Path, url: &str) -> PathBuf {
    let digest = format!("{:x}", Sha256::digest(url.as_bytes()));
    let short: String = digest.chars().take(12).collect();
    let base = url
        .rsplit('/')
        .next()
        .unwrap_or("prebuild.tgz")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-");
    cache_dir
        .join("prebuilds")
        .join(format!("{}-{}", short, base))
}

/// Fetch the prebuild tarball into the cache dir. Offline-aware: when offline
/// and the tarball is already cached it is reused; otherwise None (caller
/// warns and skips).
fn fetch_prebuild_tarball(url: &str, cache_dir: &Path) -> Option<PathBuf> {
    let dest = prebuild_cache_path(cache_dir, url);
    if dest.exists() {
        return Some(dest);
    }
    if crate::registry::offline_mode() {
        return None;
    }
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let _ = crate::utils::log(&format!("prebuild: cache mkdir failed: {}", e));
            return None;
        }
    }
    // GitHub releases redirect to objects.githubusercontent.com; the shared
    // client follows redirects. Best-effort: on 404 (no prebuilt for this
    // node/abi) or network failure, warn via caller.
    match crate::http_client::get_to_file(url, &dest) {
        Ok(()) => {
            let _ = crate::utils::log(&format!(
                "prebuild: fetched {}",
                dest.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
            Some(dest)
        }
        Err(e) => {
            let _ = crate::utils::log(&format!("prebuild: download failed for {}: {}", url, e));
            let _ = std::fs::remove_file(&dest);
            None
        }
    }
}

/// Extract a `.tar.gz` into `dest`, preserving the tarball's internal paths
/// (prebuild tarballs contain `build/Release/<mod>.node` at the root — no
/// `package/` prefix, so the top-level stripping used for npm tarballs must
/// NOT apply here).
fn extract_prebuild_tarball(tarball: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(tarball).map_err(|e| e.to_string())?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let rel = path.to_string_lossy();
        if rel.is_empty() {
            continue;
        }
        let out_path = dest.join(rel.as_ref());
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out_path.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            entry.unpack(&out_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Ensure the store dir for a package contains its prebuilt native binary.
///
/// - Reads `<unpacked>/package.json`; returns Ok(()) when the package does not
///   use prebuild-install (the common case) or node is unavailable.
/// - Computes the download URL, fetches (offline-aware, cached), and extracts
///   the `.node` alongside `package.json`.
/// - Never fails the install: on any error it logs a warning and returns Ok,
///   matching npm behaviour where a missing prebuilt falls back to node-gyp
///   compilation (which jhol deliberately does not run).
pub fn ensure_prebuilt(unpacked: &Path, cache_dir: &Path) -> Result<(), String> {
    if std::env::var("JHOL_NO_PREBUILD")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Ok(());
    }
    let pkg_json = unpacked.join("package.json");
    let raw = match std::fs::read(&pkg_json) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let pkg: Value = match serde_json::from_slice(&raw) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let runtime = match uses_prebuild_install(&pkg) {
        Some(r) => r,
        None => return Ok(()),
    };
    let Some((node_abi, _)) = node_runtime_info() else {
        let _ =
            crate::utils::log("prebuild: node unavailable; skipping native prebuilt binary fetch");
        return Ok(());
    };
    let Some(url) = prebuild_url_for_manifest(&pkg, &runtime) else {
        let _ = crate::utils::log(&format!(
            "prebuild: cannot determine download URL for {} (runtime {})",
            pkg.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
            runtime
        ));
        return Ok(());
    };
    let Some(abi) = resolve_abi(&pkg, &runtime, &node_abi) else {
        let _ = crate::utils::log(&format!(
            "prebuild: no compatible N-API build version for {}",
            pkg.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        ));
        return Ok(());
    };
    let marker = marker_name(npm_platform(), npm_arch(), &abi);
    if unpacked.join(&marker).exists() {
        return Ok(());
    }

    let Some(tarball) = fetch_prebuild_tarball(&url, cache_dir) else {
        let _ = crate::utils::log(&format!(
            "prebuild: no prebuilt binary available for {} (would require node-gyp compile)",
            pkg.get("name").and_then(|n| n.as_str()).unwrap_or("?")
        ));
        return Ok(());
    };

    // Remove any stale marker + old binary so a re-fetch can't leave orphans.
    for ent in std::fs::read_dir(unpacked).map_err(|e| e.to_string())? {
        if let Ok(e) = ent {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(".jhol-prebuild") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    if let Err(e) = extract_prebuild_tarball(&tarball, unpacked) {
        let _ = crate::utils::log(&format!("prebuild: extraction failed: {}", e));
        return Ok(());
    }
    let _ = std::fs::write(unpacked.join(&marker), "ok");
    let _ = crate::utils::log(&format!(
        "prebuild: installed native binary for {} ({}-{}-{})",
        pkg.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
        npm_platform(),
        npm_arch(),
        abi
    ));
    Ok(())
}

/// Convenience wrapper used by the bundled/store materialization paths.
pub fn ensure_prebuilt_for_store(unpacked: &Path, cache_dir: &Path) {
    let _ = ensure_prebuilt(unpacked, cache_dir);
}

/// Debug/CLI helper: compute the prebuild download URL for a manifest.
pub fn debug_prebuild_url(pkg: &Value, runtime: &str) -> Option<String> {
    prebuild_url_for_manifest(pkg, runtime)
}
