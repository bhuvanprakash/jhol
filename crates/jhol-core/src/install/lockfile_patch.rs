//! Lockfile patching for dependencies that are not plain registry ranges.
//!
//! `npm:` aliases, local (`file:`/`link:`/`workspace:`) and git dependencies are
//! materialised by the installer rather than downloaded, and optional deps for other
//! platforms are resolved but not installed. Both groups still have to appear in
//! `package-lock.json`, so their entries are added/updated here after the main
//! lockfile write. Kept apart from the installer because it is all package-lock.json
//! bookkeeping, not installation.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct OptionalVariant {
    name: String,
    version: String,
    resolved: String,
    integrity: Option<String>,
    os: Vec<String>,
    cpu: Vec<String>,
    libc: Vec<String>,
}

impl OptionalVariant {
    /// npm negation semantics: unpinned (`os: []`) means every platform;
    /// positives must include this platform; `!<platform>` excludes it.
    pub(super) fn matches_current_platform(&self) -> bool {
        fn allows(list: &[String], current: &str) -> bool {
            if list.is_empty() {
                return true;
            }
            let mut positives = 0usize;
            for item in list {
                if let Some(neg) = item.strip_prefix('!') {
                    if neg == current {
                        return false;
                    }
                } else {
                    positives += 1;
                    if item == current {
                        return true;
                    }
                }
            }
            positives == 0
        }
        let cur_os = Self::npm_os_token(std::env::consts::OS);
        let cur_arch = Self::npm_arch_token(std::env::consts::ARCH);
        allows(&self.os, &cur_os) && allows(&self.cpu, &cur_arch)
    }

    /// Rust's `std::env::consts::OS` speaks `macos`; npm packuments speak
    /// `darwin` (and `aarch64` vs `arm64`). Comparing the raw constants against
    /// packument tokens silently drops THIS machine's own binary variants.
    pub(super) fn npm_os_token(os: &str) -> String {
        match os {
            "macos" => "darwin".to_string(),
            "windows" => "win32".to_string(),
            other => other.to_string(),
        }
    }

    pub(super) fn npm_arch_token(arch: &str) -> String {
        match arch {
            "aarch64" => "arm64".to_string(),
            "x86_64" => "x64".to_string(),
            "x86" => "ia32".to_string(),
            other => other.to_string(),
        }
    }

    pub(super) fn new(
        name: String,
        version: String,
        resolved: String,
        integrity: Option<String>,
        os: Vec<String>,
        cpu: Vec<String>,
        libc: Vec<String>,
    ) -> Self {
        Self {
            name,
            version,
            resolved,
            integrity,
            os,
            cpu,
            libc,
        }
    }
}

pub(super) fn optional_variant_from_meta(
    name: &str,
    version: &str,
    meta: &serde_json::Value,
) -> OptionalVariant {
    let entry = meta
        .get("versions")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(version));
    let str_list = |key: &str| -> Vec<String> {
        entry
            .and_then(|e| e.get(key))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let resolved = entry
        .and_then(|e| e.get("dist"))
        .and_then(|d| d.get("tarball"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| crate::lockfile::tarball_url_from_registry(name, version));
    let integrity = entry
        .and_then(|e| e.get("dist"))
        .and_then(|d| d.get("integrity"))
        .and_then(|v| v.as_str())
        .map(String::from);
    OptionalVariant {
        name: name.to_string(),
        version: version.to_string(),
        resolved,
        integrity,
        os: str_list("os"),
        cpu: str_list("cpu"),
        libc: str_list("libc"),
    }
}

/// Merge resolved-but-not-installed optional platform variants into a written
/// `package-lock.json` so it is portable across platforms (npm parity).
/// Backfill `integrity` (sha512 SRI) into a legacy lockfile from the store
/// tarballs this install just fetched and hashed.
///
/// `jhol ci` / `--frozen` reject integrity-less lockfiles with "run install
/// without --frozen to regenerate lockfile with integrity" — but plain install
/// never wrote integrity back, so that message was a lie and legacy projects
/// could never reach `ci`. Hashes resolve through the store index so warm-cache
/// and bundled-store installs backfill too (they fetch nothing). No-op when
/// every entry already carries integrity or a tarball is absent from the store.
pub(crate) fn backfill_lockfile_integrity(lock_path: &Path, cache_dir: &Path) {
    let debug = std::env::var("JHOL_DEBUG_INTEGRITY").is_ok();
    if debug {
        eprintln!("[integrity] backfill entered");
    }
    let mut missing = 0usize;
    let mut no_hash = 0usize;
    let mut written = 0usize;
    let Ok(data) = std::fs::read_to_string(lock_path) else {
        return;
    };
    let Ok(mut lock) = serde_json::from_str::<serde_json::Value>(&data) else {
        return;
    };
    let Some(packages) = lock.get_mut("packages").and_then(|p| p.as_object_mut()) else {
        return;
    };
    let mut changed = false;
    for (key, entry) in packages.iter_mut() {
        let Some(obj) = entry.as_object_mut() else {
            continue;
        };
        let has_sri = obj
            .get("integrity")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if has_sri {
            continue;
        }
        missing += 1;
        let Some(version) = obj.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        // Tail after the LAST "node_modules/" = this entry's own package name
        // (scoped names keep their slash; root key "" never matches a spec).
        let name = key.rsplit("node_modules/").next().unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let spec = format!("{}@{}", name, version);
        let Some(hash) = crate::registry::store_or_binary_hash(&spec) else {
            no_hash += 1;
            if debug {
                eprintln!("[integrity] no store hash for {spec}");
            }
            continue;
        };
        let tarball = crate::content::tarball_path(cache_dir, &hash);
        let mut sri = crate::utils::sri_sha512_of_file(&tarball).ok();
        if sri.is_none() {
            // Bytes jhol already owns: decode this exact version from the bundled
            // store and attest it. Runs once — the decoded tarball is written to
            // the local store file so every later run takes the path above. If
            // neither source holds the bytes, the entry stays unattested: frozen
            // installs refusing them is the CORRECT integrity semantics, not a
            // bug (we never fabricate an SRI).
            if let Some(bytes) = crate::registry::load_binary_tarball(name, version).ok().flatten() {
                sri = Some(crate::utils::sri_sha512_of_bytes(&bytes));
                let _ = std::fs::write(&tarball, &bytes);
            }
        }
        if let Some(sri) = sri {
            obj.insert("integrity".to_string(), serde_json::json!(sri));
            written += 1;
            changed = true;
        } else if debug {
            eprintln!("[integrity] cannot hash {}", tarball.display());
        }
    }
    if debug {
        eprintln!("[integrity] missing={missing} no_hash={no_hash} written={written} changed={changed}");
    }
    if !changed {
        return;
    }
    let Ok(json) = serde_json::to_string(&lock) else {
        return;
    };
    let temp = lock_path.with_extension("tmp");
    if std::fs::write(&temp, json).is_ok() {
        let _ = std::fs::rename(temp, lock_path);
    }
}

pub(super) fn append_optional_variants_to_lockfile(lock_path: &Path, variants: &[OptionalVariant]) {
    if variants.is_empty() {
        return;
    }
    let Ok(data) = std::fs::read_to_string(lock_path) else {
        return;
    };
    let Ok(mut lock) = serde_json::from_str::<serde_json::Value>(&data) else {
        return;
    };
    let Some(packages) = lock.get_mut("packages").and_then(|p| p.as_object_mut()) else {
        return;
    };
    let mut changed = false;
    for v in variants {
        let key = format!("node_modules/{}", v.name);
        if let Some(existing) = packages.get_mut(&key).and_then(|e| e.as_object_mut()) {
            // The optional dep installed on THIS platform: mark it `optional` so
            // other platforms skip it (npm does this). Without the flag the
            // entry is treated as required and installed everywhere.
            if existing.get("optional").and_then(|o| o.as_bool()) != Some(true) {
                existing.insert("optional".to_string(), serde_json::json!(true));
                if !v.os.is_empty() && !existing.contains_key("os") {
                    existing.insert("os".to_string(), serde_json::json!(v.os));
                }
                if !v.cpu.is_empty() && !existing.contains_key("cpu") {
                    existing.insert("cpu".to_string(), serde_json::json!(v.cpu));
                }
                changed = true;
            }
            continue;
        }
        let mut e = serde_json::Map::new();
        e.insert("version".to_string(), serde_json::json!(v.version));
        e.insert("resolved".to_string(), serde_json::json!(v.resolved));
        if let Some(i) = &v.integrity {
            e.insert("integrity".to_string(), serde_json::json!(i));
        }
        e.insert("optional".to_string(), serde_json::json!(true));
        if !v.os.is_empty() {
            e.insert("os".to_string(), serde_json::json!(v.os));
        }
        if !v.cpu.is_empty() {
            e.insert("cpu".to_string(), serde_json::json!(v.cpu));
        }
        if !v.libc.is_empty() {
            e.insert("libc".to_string(), serde_json::json!(v.libc));
        }
        e.insert(
            "dependencies".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
        packages.insert(key, serde_json::Value::Object(e));
        changed = true;
    }
    if changed {
        if let Ok(s) = serde_json::to_string(&lock) {
            let _ = std::fs::write(lock_path, s);
        }
    }
}

/// Record local (`file:`/`link:`/`workspace:`) and `npm:` alias dependencies in
/// the lockfile so it matches npm and stays usable on reinstall.
fn append_special_deps_to_lockfile(lock_path: &Path, specials: &[crate::spec_grammar::SpecialDep]) {
    if specials.is_empty() {
        return;
    }
    let Ok(data) = std::fs::read_to_string(lock_path) else {
        return;
    };
    let Ok(mut lock) = serde_json::from_str::<serde_json::Value>(&data) else {
        return;
    };
    let Some(packages) = lock.get_mut("packages").and_then(|p| p.as_object_mut()) else {
        return;
    };
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut changed = false;
    for dep in specials {
        let key = format!("node_modules/{}", dep.name);
        let mut entry = serde_json::Map::new();
        match &dep.kind {
            crate::spec_grammar::SpecialKind::Local { dir, version, .. } => {
                entry.insert("version".to_string(), serde_json::json!(version));
                // For a `file:` spec record the spec the user wrote: the directory
                // it resolved to can be an internal cache path (a tarball is
                // unpacked into the cache) and a later install would fetch that
                // path as a URL.
                let resolved = if dep.spec.starts_with("file:") {
                    dep.spec.clone()
                } else {
                    format!("file:{}", relative_path(&project_dir, dir))
                };
                entry.insert("resolved".to_string(), serde_json::json!(resolved));
                entry.insert("link".to_string(), serde_json::json!(true));
            }
            crate::spec_grammar::SpecialKind::Alias { real, .. } => {
                let src = packages
                    .get(&format!("node_modules/{}", real))
                    .and_then(|v| v.as_object());
                match src {
                    Some(src) => {
                        entry.insert("name".to_string(), serde_json::json!(real));
                        for k in ["version", "resolved", "integrity"] {
                            if let Some(v) = src.get(k) {
                                entry.insert(k.to_string(), v.clone());
                            }
                        }
                        // The alias name is materialized by the pre-pass (a symlink to
                        // the real package), so mark it like a local. Without the
                        // marker the entry reads as a registry package *named* the
                        // alias, and the installer tries to fetch `my-lodash@4.17.21`
                        // — which exists in no registry and made `install --offline`
                        // fail on a project whose alias was already installed.
                        entry.insert("link".to_string(), serde_json::json!(true));
                    }
                    // Real package not present (e.g. hoisted elsewhere): leave
                    // the existing entry alone rather than writing a partial one.
                    None => continue,
                }
            }
            crate::spec_grammar::SpecialKind::Git {
                url: _,
                committish,
                version,
                ..
            } => {
                // Record the spec the user wrote (`git+file://…`, `github:u/r#tag`)
                // and mark the entry as materialized by the pre-pass, exactly as
                // `file:` does. Storing the clone URL instead made a reinstall try
                // to fetch it, and the entry carried no marker saying it is a
                // checkout rather than a download.
                entry.insert("resolved".to_string(), serde_json::json!(dep.spec));
                entry.insert("link".to_string(), serde_json::json!(true));
                if !version.is_empty() {
                    entry.insert("version".to_string(), serde_json::json!(version));
                }
                if let Some(c) = committish {
                    entry.insert("committish".to_string(), serde_json::json!(c));
                }
            }
        }
        packages.insert(key, serde_json::Value::Object(entry));
        changed = true;
    }
    // The root `dependencies` block must keep the *written spec*. The installer
    // writes the resolved version there (`"git-lib": "4.5.6"`), which silently
    // replaced the user's `git+file://…` / `file:../lib` / `npm:lodash@4` — so the
    // lockfile stopped describing the project and a reinstall resolved the special
    // from the registry instead. Only sections that already declare the name are
    // touched, so no dependency is invented.
    if let Some(root) = packages.get_mut("").and_then(|v| v.as_object_mut()) {
        const SECTIONS: [&str; 4] = [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ];
        let mut rewrote = false;
        for dep in specials {
            for section in SECTIONS {
                if let Some(v) = root
                    .get_mut(section)
                    .and_then(|s| s.as_object_mut())
                    .and_then(|o| o.get_mut(&dep.name))
                {
                    *v = serde_json::json!(dep.spec);
                    rewrote = true;
                    break;
                }
            }
        }
        if rewrote {
            changed = true;
        }
    }
    if changed {
        if let Ok(s) = serde_json::to_string(&lock) {
            let _ = std::fs::write(lock_path, s);
        }
    }
}

/// Symlink alias/local/git dependencies into `node_modules`.
///
/// Runs at the *end* of the install: a registry package hoisted later (e.g. a
/// transitive with the same name) must not clobber a local checkout.
fn relink_special_deps(node_modules: &Path, specials: &[crate::spec_grammar::SpecialDep]) {
    for dep in specials {
        match &dep.kind {
            crate::spec_grammar::SpecialKind::Alias { real, .. } => {
                let real_dir = node_modules.join(real);
                if !real_dir.exists() {
                    continue;
                }
                let real_dir = real_dir.canonicalize().unwrap_or(real_dir);
                let _ = crate::spec_grammar::link_local_dep(node_modules, &dep.name, &real_dir);
            }
            crate::spec_grammar::SpecialKind::Local { dir, .. } => {
                let _ = crate::spec_grammar::link_local_dep(node_modules, &dep.name, dir);
            }
            crate::spec_grammar::SpecialKind::Git { dir: Some(dir), .. } => {
                let _ = crate::spec_grammar::link_local_dep(node_modules, &dep.name, dir);
            }
            _ => {}
        }
    }
}

/// Post-install step for spec-grammar dependencies: link `npm:` aliases and
/// record local/alias/git entries in the lockfile.
pub(super) fn finalize_special_deps(
    lock_path: &Path,
    specials: &[crate::spec_grammar::SpecialDep],
) {
    if specials.is_empty() {
        return;
    }
    relink_special_deps(Path::new("node_modules"), specials);
    append_special_deps_to_lockfile(lock_path, specials);
}

/// Path of `target` relative to `from_dir` (both should be absolute).
fn relative_path(from_dir: &Path, target: &Path) -> String {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = target.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return target.to_string_lossy().to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".to_string());
    }
    for c in &to[common..] {
        parts.push(c.as_os_str().to_string_lossy().to_string());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lockfile patch records local deps with the *written* spec and aliases
    /// with the real package's identity — this is the code that used to leak an
    /// internal cache path (`file:…/local-tarballs/…`) and break the next install.
    #[test]
    fn append_special_deps_patches_local_and_alias_entries() {
        let td = tempfile::tempdir().unwrap();
        let lock = td.path().join("package-lock.json");
        std::fs::write(
            &lock,
            r#"{"lockfileVersion":3,"packages":{
                "":{"name":"app","version":"1.0.0","dependencies":{
                    "local-lib":"2.5.0","my-lodash":"4.17.21","git-lib":"4.5.6"}},
                "node_modules/lodash":{"version":"4.17.21","resolved":"https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz","integrity":"sha512-abc=="}
            }}"#,
        )
        .unwrap();

        let local_dir = td.path().join("lib");
        std::fs::create_dir_all(&local_dir).unwrap();
        let specials = vec![
            crate::spec_grammar::SpecialDep {
                name: "git-lib".to_string(),
                spec: "github:u/r#v1".to_string(),
                kind: crate::spec_grammar::SpecialKind::Git {
                    url: "https://github.com/u/r".to_string(),
                    committish: Some("v1".to_string()),
                    dir: None,
                    version: "4.5.6".to_string(),
                },
            },
            crate::spec_grammar::SpecialDep {
                name: "local-lib".to_string(),
                spec: "file:../lib".to_string(),
                kind: crate::spec_grammar::SpecialKind::Local {
                    dir: local_dir.clone(),
                    version: "2.5.0".to_string(),
                    link: true,
                },
            },
            crate::spec_grammar::SpecialDep {
                name: "my-lodash".to_string(),
                spec: "npm:lodash@4.17.21".to_string(),
                kind: crate::spec_grammar::SpecialKind::Alias {
                    real: "lodash".to_string(),
                    range: "4.17.21".to_string(),
                },
            },
        ];
        append_special_deps_to_lockfile(&lock, &specials);

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
        let pkgs = v["packages"].as_object().unwrap();

        // Local: keeps the user's spec, marked as a link.
        let l = &pkgs["node_modules/local-lib"];
        assert_eq!(l["version"], "2.5.0");
        assert_eq!(l["resolved"], "file:../lib", "must not leak a cache path");
        assert_eq!(l["link"], true);

        // Git: keeps the written spec (not the clone URL) and is marked so a
        // reinstall re-checks the checkout instead of downloading it.
        let g = &pkgs["node_modules/git-lib"];
        assert_eq!(g["resolved"], "github:u/r#v1");
        assert_eq!(g["link"], true);

        // Alias: carries the real package's identity and provenance.
        let a = &pkgs["node_modules/my-lodash"];
        assert_eq!(a["name"], "lodash");
        assert_eq!(a["version"], "4.17.21");
        assert_eq!(a["integrity"], "sha512-abc==");
        assert!(
            a["resolved"].as_str().unwrap().contains("lodash"),
            "resolved: {a}"
        );

        // Root specs: the user's written specifiers survive, so the lockfile still
        // describes the project and a reinstall does not hit the registry.
        let root_deps = pkgs[""]["dependencies"].as_object().unwrap();
        assert_eq!(root_deps["local-lib"], "file:../lib");
        assert_eq!(root_deps["my-lodash"], "npm:lodash@4.17.21");
        assert_eq!(root_deps["git-lib"], "github:u/r#v1");
    }
}

#[cfg(test)]
mod platform_token_tests {
    use super::OptionalVariant;

    fn variant(os: &[&str], cpu: &[&str]) -> OptionalVariant {
        OptionalVariant::new(
            "p".to_string(),
            "1.0.0".to_string(),
            "r".to_string(),
            None,
            os.iter().map(|s| s.to_string()).collect(),
            cpu.iter().map(|s| s.to_string()).collect(),
            Vec::new(),
        )
    }

    #[test]
    fn darwin_arm64_is_this_machine() {
        // Rust consts say macos/aarch64; npm packuments say darwin/arm64.
        // A regression here silently drops THIS machine's own binaries.
        if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
            return;
        }
        let v = variant(&["darwin"], &["arm64"]);
        assert!(v.matches_current_platform(), "own binary must be current-platform");
    }

    #[test]
    fn foreign_platforms_are_not_current() {
        if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
            return;
        }
        assert!(!variant(&["linux"], &["x64"]).matches_current_platform());
        assert!(!variant(&["win32"], &["x64"]).matches_current_platform());
        assert!(!variant(&["darwin"], &["x64"]).matches_current_platform(), "right OS, wrong arch");
        assert!(variant(&[], &[]).matches_current_platform(), "unpinned = everywhere");
    }

    #[test]
    fn negation_semantics() {
        if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
            return;
        }
        assert!(variant(&["!win32"], &[]).matches_current_platform(), "not-windows includes macos");
        assert!(!variant(&["!darwin"], &[]).matches_current_platform(), "not-darwin excludes macos");
    }
}
