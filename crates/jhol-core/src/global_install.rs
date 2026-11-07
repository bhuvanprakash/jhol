//! `jhol install -g <pkg>` — machine-wide package installation.
//!
//! npm puts global packages in `{prefix}/lib/node_modules` and shims their
//! `bin` entries into `{prefix}/bin`. jhol does the same, with its own prefix
//! (`$JHOL_GLOBAL_PREFIX`, else `~/.jhol/global`), so a global install never
//! touches a user's npm prefix and the two can coexist.
//!
//! Layout:
//! ```text
//! <prefix>/node_modules/<pkg>        installed package
//! <prefix>/bin/<bin-name>            shim → ../node_modules/<pkg>/<bin>
//! <prefix>/package.json              manifest recording everything installed
//! <prefix>/package-lock.json         the lockfile for the global tree
//! ```
//!
//! The shim is a tiny POSIX script rather than a symlink so it works for
//! packages whose `bin` is a shell script or a `.js` file without a shebang
//! (we add `#!/usr/bin/env node` behaviour by exec'ing the current `node` when
//! the target is a `.js` file and no shebang is present).

use std::path::{Path, PathBuf};

/// The global prefix: `$JHOL_GLOBAL_PREFIX`, else `~/.jhol/global`.
pub fn prefix() -> PathBuf {
    if let Ok(p) = std::env::var("JHOL_GLOBAL_PREFIX") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".jhol").join("global")
}

pub fn node_modules() -> PathBuf {
    prefix().join("node_modules")
}

pub fn bin_dir() -> PathBuf {
    prefix().join("bin")
}

fn manifest_path() -> PathBuf {
    prefix().join("package.json")
}

/// Read the global manifest (created on first install).
pub fn read_manifest() -> serde_json::Value {
    std::fs::read_to_string(manifest_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| {
            serde_json::json!({
                "name": "jhol-global",
                "version": "1.0.0",
                "private": true,
                "description": "Globally installed packages (jhol)",
                "dependencies": {}
            })
        })
}

pub fn write_manifest(v: &serde_json::Value) -> Result<(), String> {
    let p = manifest_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(&p, text + "\n").map_err(|e| e.to_string())
}

/// Record `name@range` in the global manifest's dependencies.
pub fn record_dependency(name: &str, range: &str) -> Result<(), String> {
    let mut m = read_manifest();
    let deps = m
        .as_object_mut()
        .map(|o| {
            o.entry("dependencies")
                .or_insert_with(|| serde_json::json!({}))
        })
        .and_then(|d| d.as_object_mut());
    if let Some(deps) = deps {
        deps.insert(name.to_string(), serde_json::json!(range));
    }
    write_manifest(&m)
}

/// Drop `name` from the global manifest.
pub fn forget_dependency(name: &str) -> Result<(), String> {
    let mut m = read_manifest();
    if let Some(deps) = m.get_mut("dependencies").and_then(|d| d.as_object_mut()) {
        deps.remove(name);
    }
    write_manifest(&m)
}

/// Names recorded in the global manifest.
pub fn installed() -> Vec<(String, String)> {
    read_manifest()
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|o| {
            let mut v: Vec<(String, String)> = o
                .iter()
                .map(|(k, val)| (k.clone(), val.as_str().unwrap_or("").to_string()))
                .collect();
            v.sort();
            v
        })
        .unwrap_or_default()
}

/// Create the executable shim for one `bin` entry.
///
/// `bin_rel` is the path inside the package (from its `bin` field). The shim
/// resolves its own directory at runtime, so moving the prefix keeps it
/// working.
pub fn write_shim(name: &str, pkg: &str, bin_rel: &str) -> Result<PathBuf, String> {
    let dir = bin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let shim = dir.join(name);
    // A POSIX shim: locate the prefix from $0, then exec the target. `.js`
    // targets without a shebang are run with the current `node`; anything else
    // is exec'd directly (already executable, or has its own shebang).
    let script = format!(
        "#!/bin/sh\n\
         # jhol global shim: {pkg} -> {bin_rel}\n\
         self=\"$0\"\n\
         while [ -L \"$self\" ]; do\n\
         \x20 d=$(dirname -- \"$self\")\n\
         \x20 self=$(readlink -- \"$self\")\n\
         \x20 case $self in /*) ;; *) self=\"$d/$self\" ;; esac\n\
         done\n\
         dir=$(CDPATH= cd -- \"$(dirname -- \"$self\")\" && pwd)\n\
         target=\"$dir/../node_modules/{pkg}/{bin_rel}\"\n\
         if [ ! -e \"$target\" ]; then\n\
         \x20 echo \"jhol: {name}: missing $target\" >&2; exit 127\n\
         fi\n\
         case \"$target\" in\n\
         \x20 *.js|*.cjs|*.mjs)\n\
         \x20   if head -c 2 \"$target\" 2>/dev/null | grep -q '#!'; then exec \"$target\" \"$@\"; fi\n\
         \x20   exec node \"$target\" \"$@\" ;;\n\
         \x20 *) exec \"$target\" \"$@\" ;;\n\
         esac\n",
        pkg = pkg,
        bin_rel = bin_rel,
        name = name
    );
    std::fs::write(&shim, script).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    Ok(shim)
}

/// Install shims for every `bin` entry a package declares.
/// Returns the shim paths written.
pub fn link_bins(pkg_dir: &Path, pkg_name: &str) -> Result<Vec<PathBuf>, String> {
    let pj = pkg_dir.join("package.json");
    let Ok(text) = std::fs::read_to_string(&pj) else {
        return Ok(Vec::new());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(Vec::new());
    };
    let Some(bin) = v.get("bin") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    match bin {
        serde_json::Value::String(target) => {
            // A single string bin takes the package's own name.
            let name = pkg_name.rsplit('/').next().unwrap_or(pkg_name);
            out.push(write_shim(name, pkg_name, target)?);
        }
        serde_json::Value::Object(map) => {
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            for name in names {
                if let Some(target) = map.get(name).and_then(|t| t.as_str()) {
                    out.push(write_shim(name, pkg_name, target)?);
                }
            }
        }
        _ => {}
    }
    Ok(out)
}

/// Remove a package's shims (by package name).
pub fn unlink_bins(pkg_dir: &Path, pkg_name: &str) {
    let pj = pkg_dir.join("package.json");
    let Ok(text) = std::fs::read_to_string(&pj) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let dir = bin_dir();
    match v.get("bin") {
        Some(serde_json::Value::String(_)) => {
            let name = pkg_name.rsplit('/').next().unwrap_or(pkg_name);
            let _ = std::fs::remove_file(dir.join(name));
        }
        Some(serde_json::Value::Object(map)) => {
            for name in map.keys() {
                let _ = std::fs::remove_file(dir.join(name));
            }
        }
        _ => {}
    }
}


/// Install packages globally: materialise them under the prefix and shim
/// their `bin` entries into `<prefix>/bin`.
///
/// The install runs with the prefix as its project directory, so every
/// existing install lane (lockfile, integrity, receipts) applies unchanged —
/// the global tree is just another project whose manifest we own.
pub fn install_global(
    packages: &[&str],
    options: &crate::install::InstallOptions,
) -> Result<Vec<String>, String> {
    let prefix = prefix();
    std::fs::create_dir_all(prefix.join("node_modules")).map_err(|e| e.to_string())?;
    // The manifest doubles as the project package.json for the install.
    write_manifest(&read_manifest())?;

    let prev = std::env::current_dir().map_err(|e| e.to_string())?;
    std::env::set_current_dir(&prefix).map_err(|e| format!("chdir prefix: {}", e))?;
    let result = (|| -> Result<Vec<String>, String> {
        crate::install::install_package(packages, options)
            .map_err(|e| format!("{}", e))?;
        // Record what was asked for, and shim every newly installed bin.
        let mut shimmed = Vec::new();
        let nm = std::path::PathBuf::from("node_modules");
        if packages.is_empty() {
            // Whole-manifest install: shim everything recorded.
            for (name, _) in installed() {
                let dir = nm.join(&name);
                if dir.is_dir() {
                    for s in link_bins(&dir, &name)? {
                        shimmed.push(s.display().to_string());
                    }
                }
            }
        } else {
            for spec in packages {
                let name = base_name(spec);
                let dir = nm.join(&name);
                // Record what actually landed, as `^<version>` — the same
                // shape `npm i -g` writes. Recording the raw request
                // ("latest") made every later global install collide: the
                // manifest then declared a range no resolved version could
                // satisfy, and the resolver refused the tree.
                let range = installed_range(&dir, &name, spec);
                record_dependency(&name, &range)?;
                if dir.is_dir() {
                    for s in link_bins(&dir, &name)? {
                        shimmed.push(s.display().to_string());
                    }
                }
            }
        }
        Ok(shimmed)
    })();
    let _ = std::env::set_current_dir(&prev);
    result
}

/// `@scope/name@range` -> `@scope/name`.
fn base_name(spec: &str) -> String {
    if let Some(rest) = spec.strip_prefix('@') {
        // scoped: the version separator is the last '@'
        match rest.rsplit_once('@') {
            Some((name, _)) if !name.is_empty() => format!("@{}", name),
            _ => spec.to_string(),
        }
    } else {
        spec.rsplit_once('@').map(|(n, _)| n.to_string()).unwrap_or_else(|| spec.to_string())
    }
}

/// Remove a globally installed package and its shims.
pub fn uninstall_global(name: &str) -> Result<bool, String> {
    let dir = node_modules().join(name);
    let existed = dir.exists();
    if existed {
        unlink_bins(&dir, name);
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    forget_dependency(name)?;
    Ok(existed)
}


/// The range to record for a just-installed global package: `^version` from
/// the installed manifest, falling back to the requested spec when the
/// package has no readable version.
fn installed_range(dir: &std::path::Path, name: &str, requested: &str) -> String {
    let pj = dir.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pj) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                return format!("^{}", ver);
            }
        }
    }
    // Keep an explicit range the user asked for; otherwise pin to name alone.
    match requested.rsplit_once('@') {
        Some((n, r)) if !n.is_empty() && !r.is_empty() => r.to_string(),
        _ => {
            let _ = name;
            "*".to_string()
        }
    }
}

#[cfg(test)]
mod naming_tests {
    use super::base_name;

    #[test]
    fn base_name_strips_version() {
        assert_eq!(base_name("typescript"), "typescript");
        assert_eq!(base_name("typescript@5.4.0"), "typescript");
        assert_eq!(base_name("@scope/pkg"), "@scope/pkg");
        assert_eq!(base_name("@scope/pkg@1.2.3"), "@scope/pkg");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_content_targets_package_bin() {
        let d = std::env::temp_dir().join(format!("jhol-shim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::env::set_var("JHOL_GLOBAL_PREFIX", &d);
        let shim = write_shim("demo", "demo-pkg", "bin/cli.js").unwrap();
        let body = std::fs::read_to_string(&shim).unwrap();
        assert!(body.starts_with("#!/bin/sh"));
        assert!(body.contains("../node_modules/demo-pkg/bin/cli.js"));
        let _ = std::fs::remove_dir_all(&d);
        std::env::remove_var("JHOL_GLOBAL_PREFIX");
    }
}