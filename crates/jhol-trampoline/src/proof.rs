use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

const INPUTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "yarn.lock",
    ".jholrc",
    ".npmrc",
];

/// Identity shared by Jhol's installer and its Rust front door.
pub fn project_identity(root: &Path) -> Option<String> {
    if !root.join("package.json").is_file() {
        return None;
    }
    if !INPUTS[1..7].iter().any(|name| root.join(name).is_file()) {
        return None;
    }

    let mut hash = Sha256::new();
    hash.update(b"jhol-instant-proof-v3\0");
    hash.update(std::env::consts::OS.as_bytes());
    hash.update([0]);
    hash.update(std::env::consts::ARCH.as_bytes());
    hash.update([0]);
    for name in INPUTS {
        hash.update(name.as_bytes());
        hash.update([0]);
        match std::fs::read(root.join(name)) {
            Ok(bytes) => {
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                hash.update(u64::MAX.to_le_bytes())
            }
            Err(_) => return None,
        }
    }
    if let Some(home) = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
    {
        for name in [".jholrc", ".npmrc"] {
            hash.update(b"user-config\0");
            hash.update(name.as_bytes());
            hash.update([0]);
            match std::fs::read(Path::new(&home).join(name)) {
                Ok(bytes) => {
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(bytes);
                }
                _ => hash.update(u64::MAX.to_le_bytes()),
            }
        }
    }
    for name in [
        "JHOL_OFFLINE",
        "JHOL_NO_CACHE",
        "JHOL_BACKEND",
        "JHOL_RESOLVER",
        "JHOL_SCRIPT_ALLOWLIST",
        "JHOL_STRICT_PEER_DEPS",
        "JHOL_ENV",
        "NODE_ENV",
        "JHOL_LINK",
        "JHOL_OS",
        "JHOL_CPU",
        "JHOL_LIBC",
        "npm_config_os",
        "npm_config_platform",
        "npm_config_cpu",
        "npm_config_arch",
        "npm_config_libc",
    ] {
        hash.update(name.as_bytes());
        hash.update([0]);
        if let Ok(value) = std::env::var(name) {
            hash.update(value.as_bytes());
        }
        hash.update([0]);
    }
    Some(hex(&hash.finalize()))
}

/// Validate marker identity and every installed path recorded by npm lock v2/v3.
/// Unknown lock formats miss safely; the regular CLI handles them.
pub fn prepared_tree_is_valid(root: &Path) -> bool {
    if !shortcut_policy_allows(root) {
        return false;
    }
    let Some(identity) = project_identity(root) else {
        return false;
    };
    if std::fs::read_to_string(root.join("node_modules/.jhol-ok"))
        .map(|value| value.trim() != identity)
        .unwrap_or(true)
    {
        return false;
    }
    locked_layout_is_valid(root, &root.join("node_modules"))
}

/// Front-door shortcuts cannot skip configured install side effects.
pub fn shortcut_policy_allows(root: &Path) -> bool {
    // Hooks and script opt-ins must run through the CLI even for an unchanged
    // tree. Input identity alone cannot prove those side effects happened now.
    if config_requires_cli(&root.join(".jholrc"))
        || std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|home| config_requires_cli(&Path::new(&home).join(".jholrc")))
            .unwrap_or(true)
        || std::env::var("JHOL_SCRIPT_ALLOWLIST").is_ok_and(|value| !value.is_empty())
    {
        return false;
    }
    true
}

fn config_requires_cli(path: &Path) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return error.kind() != std::io::ErrorKind::NotFound,
    };
    let Ok(config) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return true;
    };
    !config.is_object()
        || config.get("hooks").is_some()
        || config.get("env").is_some()
        || config.get("noScripts").and_then(serde_json::Value::as_bool) == Some(false)
        || config
            .get("install")
            .and_then(|install| install.get("noScripts"))
            .and_then(serde_json::Value::as_bool)
            == Some(false)
}

/// Required npm lock paths for a project or a prepared bundle.
pub fn locked_layout_is_valid(root: &Path, node_modules: &Path) -> bool {
    let lock = root.join("package-lock.json");
    let lock = if lock.is_file() {
        lock
    } else {
        let shrinkwrap = root.join("npm-shrinkwrap.json");
        if !shrinkwrap.is_file() {
            return false;
        }
        shrinkwrap
    };
    let Ok(bytes) = std::fs::read(lock) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    let Some(packages) = value.get("packages").and_then(serde_json::Value::as_object) else {
        return false;
    };
    for (relative, entry) in packages {
        if relative.is_empty() {
            continue;
        }
        let Some(path) = safe_lock_path(relative) else {
            return false;
        };
        if entry.get("link").and_then(serde_json::Value::as_bool) == Some(true) {
            return false; // Relocating workspace links needs the regular installer.
        }
        if entry.get("optional").and_then(serde_json::Value::as_bool) == Some(true)
            && optional_is_incompatible(entry)
        {
            continue;
        }
        let installed = node_modules.join(path);
        if !installed.is_dir() || !installed.join("package.json").is_file() {
            return false;
        }
        let Ok(installed_manifest) = std::fs::read(installed.join("package.json")) else {
            return false;
        };
        let Ok(installed_manifest) =
            serde_json::from_slice::<serde_json::Value>(&installed_manifest)
        else {
            return false;
        };
        let Some(expected_name) = locked_package_name(relative, entry) else {
            return false;
        };
        if installed_manifest
            .get("name")
            .and_then(serde_json::Value::as_str)
            != Some(expected_name.as_str())
        {
            return false;
        }
        let Some(expected_version) = entry.get("version").and_then(serde_json::Value::as_str)
        else {
            return false;
        };
        if installed_manifest
            .get("version")
            .and_then(serde_json::Value::as_str)
            != Some(expected_version)
        {
            return false;
        }
        if entry.get("link").and_then(serde_json::Value::as_bool) == Some(true)
            && !std::fs::canonicalize(installed).is_ok()
        {
            return false;
        }
    }
    true
}

fn locked_package_name(relative: &str, entry: &serde_json::Value) -> Option<String> {
    if let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) {
        return (!name.is_empty()).then(|| name.to_owned());
    }

    // npm aliases install under the requested path but lock the real package
    // name in the entry. For ordinary packages, the last node_modules segment
    // identifies the expected manifest name.
    let tail = relative.rsplit("node_modules/").next()?;
    let mut segments = tail.split('/');
    let first = segments.next()?;
    if first.starts_with('@') {
        let second = segments.next()?;
        if segments.next().is_some() {
            return None;
        }
        Some(format!("{first}/{second}"))
    } else if segments.next().is_none() {
        Some(first.to_owned())
    } else {
        None
    }
}

fn optional_is_incompatible(entry: &serde_json::Value) -> bool {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let cpu = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        other => other,
    };
    let libc = if cfg!(target_env = "musl") {
        "musl"
    } else {
        "glibc"
    };
    for (field, variables, default) in [
        (
            "os",
            &["JHOL_OS", "npm_config_os", "npm_config_platform"][..],
            os,
        ),
        (
            "cpu",
            &["JHOL_CPU", "npm_config_cpu", "npm_config_arch"][..],
            cpu,
        ),
        ("libc", &["JHOL_LIBC", "npm_config_libc"][..], libc),
    ] {
        let Some(selector) = entry.get(field).and_then(serde_json::Value::as_array) else {
            continue;
        };
        let target = variables
            .iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| default.to_string());
        let values: Vec<_> = selector
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        let excluded = values
            .iter()
            .any(|value| value.strip_prefix('!') == Some(target.trim()));
        let positives: Vec<_> = values
            .iter()
            .filter(|value| !value.starts_with('!'))
            .collect();
        if excluded
            || (!positives.is_empty()
                && !positives
                    .iter()
                    .any(|value| **value == target.trim() || **value == "any"))
        {
            return true;
        }
    }
    false
}

fn safe_lock_path(raw: &str) -> Option<PathBuf> {
    let mut components = Path::new(raw).components();
    if components.next()? != Component::Normal("node_modules".as_ref()) {
        return None;
    }
    let mut path = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(_) => path.push(component.as_os_str()),
            _ => return None,
        }
    }
    Some(path)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Compute XXH3 dep_set_hash for a list of (name, version) pairs.
#[allow(dead_code)]
pub fn compute_dep_set_hash(packages: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(&str, &str)> = packages.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));

    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for (name, version) in &sorted {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update(version.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:016x}", hasher.digest())
}

/// Extract resolved package (name, version) pairs from package-lock.json or package.json
#[allow(dead_code)]
pub fn extract_dep_pairs_from_project(root: &Path) -> Option<Vec<(String, String)>> {
    let lock = root.join("package-lock.json");
    if lock.is_file() {
        if let Ok(bytes) = std::fs::read(&lock) {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(packages) = val.get("packages").and_then(serde_json::Value::as_object) {
                    let mut pairs = Vec::new();
                    for (relative, entry) in packages {
                        if relative.is_empty() {
                            continue;
                        }
                        if let Some(path) = safe_lock_path(relative) {
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                let full_name = if let Some(parent) = path.parent() {
                                    if let Some(parent_name) =
                                        parent.file_name().and_then(|p| p.to_str())
                                    {
                                        if parent_name.starts_with('@') {
                                            format!("{}/{}", parent_name, name)
                                        } else {
                                            name.to_string()
                                        }
                                    } else {
                                        name.to_string()
                                    }
                                } else {
                                    name.to_string()
                                };
                                if let Some(version) =
                                    entry.get("version").and_then(serde_json::Value::as_str)
                                {
                                    pairs.push((full_name, version.to_string()));
                                }
                            }
                        }
                    }
                    if !pairs.is_empty() {
                        return Some(pairs);
                    }
                }
            }
        }
    }

    // Fallback: read direct dependencies from package.json
    let pj = root.join("package.json");
    if pj.is_file() {
        if let Ok(bytes) = std::fs::read(&pj) {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(deps) = val
                    .get("dependencies")
                    .and_then(serde_json::Value::as_object)
                {
                    let mut pairs = Vec::new();
                    for (name, spec) in deps {
                        if let Some(spec_str) = spec.as_str() {
                            pairs.push((name.clone(), spec_str.to_string()));
                        }
                    }
                    if !pairs.is_empty() {
                        return Some(pairs);
                    }
                }
            }
        }
    }
    None
}

/// Cache directory discovery
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("JHOL_CACHE_DIR") {
        PathBuf::from(dir)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".jhol-cache")
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        PathBuf::from(profile).join(".jhol-cache")
    } else {
        PathBuf::from(".jhol-cache")
    }
}

/// Attempt direct L0 projection from JGrid bundle into root/node_modules
pub fn try_direct_bundle_projection(root: &Path) -> bool {
    if !shortcut_policy_allows(root) {
        return false;
    }
    let nm = root.join("node_modules");
    if nm.exists() {
        if let Ok(mut entries) = std::fs::read_dir(&nm) {
            if entries.next().is_some() {
                return false;
            }
        }
    }

    if matches!(
        std::env::var("JHOL_JGRID").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    ) || matches!(
        std::env::var("JHOL_JGRID_L0").as_deref(),
        Ok("0") | Ok("false") | Ok("no")
    ) {
        return false;
    }
    let Some(identity) = project_identity(root) else {
        return false;
    };
    let bundle_path = cache_dir().join("grid").join("bundles").join(&identity);
    let bundle_nm = bundle_path.join("node_modules");
    if !bundle_path.join(".jhol-complete").is_file() || !locked_layout_is_valid(root, &bundle_nm) {
        return false;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let candidate = root.join(format!(".jhol-projection-{}-{nonce}", std::process::id()));
    if clone_directory(&bundle_nm, &candidate).is_err()
        || std::fs::write(candidate.join(".jhol-ok"), &identity).is_err()
        || project_identity(root).as_ref() != Some(&identity)
        || std::fs::rename(&candidate, &nm).is_err()
    {
        let _ = std::fs::remove_dir_all(&candidate);
        return false;
    }
    true
}

/// Preserve project isolation for the legacy replay-farm setting. Hard links
/// share writable file data with the bundle, so this route copies instead.
fn isolated_replay_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    copy_dir_recursive(src, dst)
}

/// Platform-native atomic directory clone
pub fn clone_directory(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        extern "C" {
            fn clonefile(
                src: *const std::os::raw::c_char,
                dst: *const std::os::raw::c_char,
                flags: u32,
            ) -> std::os::raw::c_int;
        }
        fn clone_one(
            src: &Path,
            dst: &Path,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let src_c = CString::new(src.as_os_str().as_bytes())?;
            let dst_c = CString::new(dst.as_os_str().as_bytes())?;
            let ret = unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
            if ret == 0 {
                return Ok(());
            }
            let err = std::io::Error::last_os_error();
            // Symlinked children (e.g. node_modules/.bin entries) are recreated
            // manually when clonefile refuses them — never fall back to a serial
            // whole-tree clone over one link.
            if std::fs::symlink_metadata(src)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                let target = std::fs::read_link(src)?;
                std::os::unix::fs::symlink(&target, dst)?;
                return Ok(());
            }
            Err(Box::new(err))
        }

        // Parallel per-child clonefile: each child subtree clones inside the
        // kernel, and independent children clone CONCURRENTLY. A single root
        // clonefile walks all ~2300 nodes serially (~25 ms warm); eight
        // parallel children cut that to ~5 ms for the same CoW result.
        let entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(src) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(e) => return Err(e),
        };
        if entries.is_empty() {
            return std::fs::create_dir_all(dst);
        }
        if std::fs::create_dir(dst).is_err()
            && dst
                .read_dir()
                .map(|mut r| r.next().is_some())
                .unwrap_or(true)
        {
            // dst already existed with content — treat as failure, caller cleans up.
            return Err(std::io::Error::other("clone destination exists"));
        }
        let nthreads = entries.len().min(8);
        let per = (entries.len() + nthreads - 1) / nthreads;
        let fail: Option<std::io::Error> = std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(nthreads);
            for (t, chunk) in entries.chunks(per).enumerate() {
                let _ = t;
                handles.push(s.spawn(move || {
                    let mut first_err = None;
                    for e in chunk {
                        let name = e.file_name();
                        if let Err(err) = clone_one(&e.path(), &dst.join(&name)) {
                            if first_err.is_none() {
                                first_err = Some(std::io::Error::other(format!("{name:?}: {err}")));
                            }
                        }
                    }
                    first_err
                }));
            }
            let mut first = None;
            for h in handles {
                if let Ok(Some(e)) = h.join() {
                    if first.is_none() {
                        first = Some(e);
                    }
                }
            }
            first
        });
        if let Some(err) = fail {
            let _ = std::fs::remove_dir_all(dst);
            // Fallback: one serial root clonefile (kernel walk), then a full copy.
            let src_c = CString::new(src.as_os_str().as_bytes())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let dst_c = CString::new(dst.as_os_str().as_bytes())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let ret = unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
            if ret == 0 {
                return Ok(());
            }
            return Err(err);
        }
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg("--reflink=auto")
            .arg(src)
            .arg(dst)
            .status()?;
        if status.success() {
            return Ok(());
        }
    }

    copy_dir_recursive(src, dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&src_path)?;
        if meta.file_type().is_symlink() {
            #[cfg(unix)]
            {
                let link_target = std::fs::read_link(&src_path)?;
                std::os::unix::fs::symlink(&link_target, &dst_path)?;
            }
            #[cfg(windows)]
            {
                let link_target = std::fs::read_link(&src_path)?;
                if std::fs::metadata(&src_path)
                    .map(|target| target.is_dir())
                    .unwrap_or(false)
                {
                    std::os::windows::fs::symlink_dir(&link_target, &dst_path)?;
                } else {
                    std::os::windows::fs::symlink_file(&link_target, &dst_path)?;
                }
            }
        } else if meta.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn bundle_replay_age_rule_declines_old_markers() {
        if std::env::var("JHOL_OFFLINE").is_ok() || std::env::var("JHOL_PACKUMENT_TTL_SECS").is_ok()
        {
            return; // environment overrides the default policy under test
        }
        let dir = std::env::temp_dir().join(format!("jhol-tramp-ttl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join(".jhol-complete");
        assert!(
            !super::bundle_within_ttl(&marker),
            "missing marker is never fresh"
        );
        std::fs::write(&marker, b"x").unwrap();
        assert!(super::bundle_within_ttl(&marker), "new marker is fresh");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&marker)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(
            !super::bundle_within_ttl(&marker),
            "2 h old exceeds the 1 h default"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn side_effect_configuration_requires_regular_cli() {
        let root = tempdir().unwrap();
        let path = root.path().join(".jholrc");
        assert!(!config_requires_cli(&path));
        for input in [
            r#"{"hooks":{"preInstall":"check"}}"#,
            r#"{"noScripts":false}"#,
            r#"{"install":{"noScripts":false}}"#,
            "invalid",
        ] {
            std::fs::write(&path, input).unwrap();
            assert!(config_requires_cli(&path));
        }
        std::fs::write(
            &path,
            r#"{"output":{"quiet":true},"install":{"noScripts":true}}"#,
        )
        .unwrap();
        assert!(!config_requires_cli(&path));
    }

    #[test]
    fn proof_binds_lockfile_and_missing_installed_paths_miss() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"a":"1"}}"#,
        )
        .unwrap();
        let lock = r#"{"lockfileVersion":3,"packages":{"":{"name":"root"},"node_modules/a":{"version":"1.0.0"}}}"#;
        std::fs::write(dir.path().join("package-lock.json"), lock).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/a")).unwrap();
        std::fs::write(
            dir.path().join("node_modules/a/package.json"),
            r#"{"name":"a","version":"1.0.0"}"#,
        )
        .unwrap();
        let identity = project_identity(dir.path()).unwrap();
        std::fs::write(dir.path().join("node_modules/.jhol-ok"), identity).unwrap();
        assert!(prepared_tree_is_valid(dir.path()));
        std::fs::write(
            dir.path().join("node_modules/a/package.json"),
            r#"{"name":"a","version":"9.0.0"}"#,
        )
        .unwrap();
        assert!(!prepared_tree_is_valid(dir.path()));
        std::fs::write(
            dir.path().join("node_modules/a/package.json"),
            r#"{"name":"a","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::remove_file(dir.path().join("node_modules/a/package.json")).unwrap();
        assert!(!prepared_tree_is_valid(dir.path()));
    }

    #[test]
    fn locked_layout_checks_package_names_and_preserves_aliases() {
        let dir = tempdir().unwrap();
        let node_modules = dir.path().join("node_modules");
        std::fs::write(
            dir.path().join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{
                "node_modules/my-lodash":{"name":"lodash","version":"4.17.21"},
                "node_modules/@scope/widget":{"version":"2.0.0"}
            }}"#,
        )
        .unwrap();
        std::fs::create_dir_all(node_modules.join("my-lodash")).unwrap();
        std::fs::write(
            node_modules.join("my-lodash/package.json"),
            r#"{"name":"lodash","version":"4.17.21"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(node_modules.join("@scope/widget")).unwrap();
        std::fs::write(
            node_modules.join("@scope/widget/package.json"),
            r#"{"name":"@scope/widget","version":"2.0.0"}"#,
        )
        .unwrap();

        assert!(locked_layout_is_valid(dir.path(), &node_modules));

        std::fs::write(
            node_modules.join("my-lodash/package.json"),
            r#"{"name":"my-lodash","version":"4.17.21"}"#,
        )
        .unwrap();
        assert!(!locked_layout_is_valid(dir.path(), &node_modules));

        std::fs::write(
            node_modules.join("my-lodash/package.json"),
            r#"{"name":"lodash","version":"4.17.21"}"#,
        )
        .unwrap();
        std::fs::write(
            node_modules.join("@scope/widget/package.json"),
            r#"{"name":"wrong-widget","version":"2.0.0"}"#,
        )
        .unwrap();
        assert!(!locked_layout_is_valid(dir.path(), &node_modules));

        std::fs::write(
            node_modules.join("@scope/widget/package.json"),
            r#"{"name":"@scope/widget","version":"2.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{"node_modules/@scope/widget":{}}}"#,
        )
        .unwrap();
        assert!(!locked_layout_is_valid(dir.path(), &node_modules));
    }

    #[test]
    fn replay_copy_keeps_project_writes_out_of_shared_bundle() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("bundle");
        let destination = dir.path().join("project-node-modules");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("package.js"), b"bundle contents").unwrap();

        isolated_replay_copy(&source, &destination).unwrap();
        std::fs::write(destination.join("package.js"), b"project change").unwrap();

        assert_eq!(
            std::fs::read(source.join("package.js")).unwrap(),
            b"bundle contents"
        );
        assert_eq!(
            std::fs::read(destination.join("package.js")).unwrap(),
            b"project change"
        );
    }

    #[cfg(unix)]
    #[test]
    fn clone_directory_preserves_package_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let source = dir.path().join("bundle");
        let destination = dir.path().join("project-node-modules");
        std::fs::create_dir_all(source.join("pkg")).unwrap();
        std::fs::write(source.join("pkg/cli.js"), b"#!/usr/bin/env node\n").unwrap();
        symlink("../pkg/cli.js", source.join("tool")).unwrap();

        clone_directory(&source, &destination).unwrap();

        let link = destination.join("tool");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(link).unwrap(),
            PathBuf::from("../pkg/cli.js")
        );
    }

    #[test]
    fn unsafe_lock_paths_miss() {
        assert!(safe_lock_path("node_modules/a").is_some());
        assert!(safe_lock_path("node_modules/../outside").is_none());
        assert!(safe_lock_path("/tmp/outside").is_none());
    }

    #[test]
    fn inactive_optional_paths_do_not_authorize_missing_active_packages() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"name":"root"}"#).unwrap();
        let mut lock = serde_json::json!({"packages": {
            "node_modules/optional": {"version":"1.0.0", "optional":true, "os":["__unavailable_platform__"]}
        }});
        std::fs::write(dir.path().join("package-lock.json"), lock.to_string()).unwrap();
        assert!(locked_layout_is_valid(
            dir.path(),
            &dir.path().join("node_modules")
        ));
        lock["packages"]["node_modules/optional"]
            .as_object_mut()
            .unwrap()
            .remove("os");
        std::fs::write(dir.path().join("package-lock.json"), lock.to_string()).unwrap();
        assert!(!locked_layout_is_valid(
            dir.path(),
            &dir.path().join("node_modules")
        ));
    }

    #[test]
    fn test_trampoline_direct_bundle_projection() {
        let dir = tempdir().unwrap();
        let cache = dir.path().join("cache");
        std::env::set_var("JHOL_CACHE_DIR", &cache);

        let pj = dir.path().join("package.json");
        std::fs::write(&pj, r#"{"dependencies":{"lodash":"4.17.21"}}"#).unwrap();

        std::fs::write(
            dir.path().join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{"node_modules/lodash":{"version":"4.17.21"}}}"#,
        )
        .unwrap();
        let hash = project_identity(dir.path()).unwrap();
        let bundle_dir = cache.join("grid").join("bundles").join(&hash);
        let bundle_nm = bundle_dir.join("node_modules");
        std::fs::create_dir_all(bundle_nm.join("lodash")).unwrap();
        std::fs::write(
            bundle_nm.join("lodash").join("package.json"),
            r#"{"name":"lodash","version":"4.17.21"}"#,
        )
        .unwrap();
        std::fs::write(bundle_dir.join(".jhol-complete"), b"jhol-jgrid-bundle-v1\n").unwrap();

        assert!(try_direct_bundle_projection(dir.path()));
        assert!(dir
            .path()
            .join("node_modules")
            .join("lodash")
            .join("package.json")
            .is_file());
        assert!(dir.path().join("node_modules").join(".jhol-ok").is_file());

        std::fs::remove_dir_all(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("package-lock.json"),
            r#"{"lockfileVersion":3,"packages":{"node_modules/lodash":{"version":"4.17.21","integrity":"changed"}}}"#).unwrap();
        assert!(
            !try_direct_bundle_projection(dir.path()),
            "replayed bundle across changed lock integrity"
        );

        std::env::remove_var("JHOL_CACHE_DIR");
    }
}

/// Fresh-project L0 replay (no lockfile yet): a byte-identical `package.json` on
/// this platform was fully installed before and published as a bundle under the
/// deps-manifest key, carrying the publisher's lockfile. Project it and restore
/// that lockfile.
///
/// Contract = reusing a committed lockfile. The key hashes the package.json
/// BYTES, so this manifest is identical to the publisher's — which the original
/// full install validated through the complete resolution law layer — therefore
/// the restored lockfile satisfies it by construction. Same staleness rule as
/// any cached resolution (no revalidation until something changes).
pub fn try_fresh_project_bundle_replay(root: &Path) -> bool {
    if !root.join("package.json").is_file() {
        return false;
    }
    if root.join("package-lock.json").is_file() || root.join("npm-shrinkwrap.json").is_file() {
        return false;
    }
    let nm = root.join("node_modules");
    if let Ok(mut entries) = std::fs::read_dir(&nm) {
        if entries.next().is_some() {
            return false;
        }
    }
    if let Ok(v) = std::env::var("JHOL_JGRID") {
        if matches!(v.as_str(), "0" | "false" | "no") {
            return false;
        }
    }
    if let Ok(v) = std::env::var("JHOL_JGRID_L0") {
        if matches!(v.as_str(), "0" | "false" | "no") {
            return false;
        }
    }

    let timing = std::env::var("JHOL_TRAMP_TIMING").is_ok();
    let tp = std::time::Instant::now();
    let Ok(pj) = std::fs::read(root.join("package.json")) else {
        return false;
    };
    let key = deps_manifest_key(&pj);
    if timing {
        eprintln!("[tramp]   2b key: {:?}", tp.elapsed());
    }
    let tp = std::time::Instant::now();
    let bundles = cache_dir().join("grid").join("bundles");
    let bundle_dir = bundles.join(&key);
    let source = bundle_dir.join("node_modules");
    if !source.is_dir() || !bundle_dir.join(".jhol-complete").is_file() {
        return false;
    }
    // Lockfile-less replay: same age rule as jhol-core's metadata TTL
    // (`origin::within_metadata_ttl`; keep in sync). An old bundle may hold
    // superseded picks, so decline and let the full path resolve live.
    if !bundle_within_ttl(&bundle_dir.join(".jhol-complete")) {
        return false;
    }
    let Ok(lock_bytes) = std::fs::read(bundle_dir.join("package-lock.json")) else {
        return false;
    };
    if timing {
        eprintln!("[tramp]   2b lookup+lock read: {:?}", tp.elapsed());
    }
    let tp = std::time::Instant::now();

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let candidate = root.join(format!(".jhol-replay-{}-{nonce}", std::process::id()));
    let farm_mode = std::env::var("JHOL_REPLAY_FARM").is_ok();
    let mat = if farm_mode {
        isolated_replay_copy(&source, &candidate)
    } else {
        clone_directory(&source, &candidate)
    };
    if mat.is_err() {
        // farm falls back to CoW clonefile on any error (cross-volume, etc.)
        let _ = std::fs::remove_dir_all(&candidate);
        if farm_mode {
            if clone_directory(&source, &candidate).is_err() {
                let _ = std::fs::remove_dir_all(&candidate);
                return false;
            }
        } else {
            return false;
        }
    }
    if timing {
        eprintln!(
            "[tramp]   2b {}: {:?}",
            if farm_mode { "farm" } else { "clone" },
            tp.elapsed()
        );
    }
    let tp = std::time::Instant::now();
    if std::fs::write(root.join("package-lock.json"), &lock_bytes).is_err() {
        let _ = std::fs::remove_dir_all(&candidate);
        return false;
    }
    if let Some(identity) = project_identity(root) {
        let _ = std::fs::write(candidate.join(".jhol-ok"), identity);
    }
    if std::fs::rename(&candidate, &nm).is_err() {
        let _ = std::fs::remove_dir_all(&candidate);
        return false;
    }
    if timing {
        eprintln!("[tramp]   2b lock+identity+rename: {:?}", tp.elapsed());
    }
    true
}

/// Age rule for lockfile-less bundle replay. Mirrors jhol-core's
/// `origin::within_metadata_ttl`: `JHOL_OFFLINE` trusts any age,
/// `JHOL_PACKUMENT_TTL_SECS` overrides the 3600 s default (0 = forever).
fn bundle_within_ttl(marker: &Path) -> bool {
    let offline = std::env::var("JHOL_OFFLINE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if offline {
        return true;
    }
    let ttl = std::env::var("JHOL_PACKUMENT_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(3600);
    if ttl == 0 {
        return true;
    }
    match std::fs::metadata(marker).and_then(|m| m.modified()) {
        Ok(written) => written
            .elapsed()
            .map(|age| age.as_secs() < ttl)
            .unwrap_or(true),
        Err(_) => false,
    }
}

/// Must match `BundleCache::deps_manifest_key` in jhol-core byte for byte.
fn deps_manifest_key(pj: &[u8]) -> String {
    use xxhash_rust::xxh3::xxh3_64;
    let mut seed = Vec::with_capacity(pj.len() + 32);
    seed.extend_from_slice(b"jhol-bundle-deps-v2");
    seed.push(0);
    seed.extend_from_slice(std::env::consts::OS.as_bytes());
    seed.push(0);
    seed.extend_from_slice(std::env::consts::ARCH.as_bytes());
    seed.push(0);
    seed.extend_from_slice(pj);
    format!("{:016x}", xxh3_64(&seed))
}
