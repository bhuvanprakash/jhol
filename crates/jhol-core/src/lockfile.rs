//! Read package-lock.json and bun.lock for resolved versions (deterministic installs).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn package_name_from_lockfile_key(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }

    let segments: Vec<&str> = key
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return None;
    }

    // npm lockfiles can use either `node_modules/<pkg>` keys or plain package keys.
    let tail = if let Some(nm_idx) = segments.iter().rposition(|seg| *seg == "node_modules") {
        &segments[nm_idx + 1..]
    } else {
        &segments[..]
    };
    if tail.is_empty() {
        return None;
    }

    let first = tail[0];
    let name = if first.starts_with('@') {
        let second = tail.get(1)?;
        format!("{}/{}", first, second)
    } else if tail.len() == 1 {
        first.to_string()
    } else {
        // Unrecognized non-scoped nested path without node_modules marker.
        return None;
    };

    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockfileInstallEntry {
    pub package: String,
    pub version: String,
    pub spec: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub install_path: String,
    pub top_level: bool,
}

fn is_safe_lockfile_path(raw: &str) -> bool {
    // Lockfile keys are logical paths; normalize extra slashes before safety checks.
    let normalized = raw.trim_matches('/');
    if normalized.is_empty() {
        return false;
    }
    let p = std::path::Path::new(normalized);
    !p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn is_top_level_lockfile_path(path: &str) -> bool {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return false;
    }
    if !trimmed.starts_with("node_modules/") {
        // Plain package keys in lockfile v3 are top-level by definition.
        if trimmed.starts_with('@') {
            return trimmed.split('/').filter(|seg| !seg.is_empty()).count() == 2;
        }
        return !trimmed.contains('/');
    }
    let rest = &trimmed["node_modules/".len()..];
    !rest.contains("/node_modules/")
}

fn normalize_lock_install_path(raw: &str) -> String {
    raw.trim_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn current_npm_os() -> String {
    crate::utils::target_os()
}

fn current_npm_cpu() -> String {
    crate::utils::target_cpu()
}

fn field_allows_current(value: Option<&serde_json::Value>, current: &str) -> bool {
    let Some(value) = value else {
        return true;
    };
    let Some(list) = value.as_array() else {
        return value.as_str().map(|s| s == current).unwrap_or(true);
    };

    let mut positives = Vec::new();
    for entry in list {
        let Some(raw) = entry.as_str() else {
            continue;
        };
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(neg) = token.strip_prefix('!') {
            if neg == current {
                return false;
            }
        } else {
            positives.push(token);
        }
    }

    positives.is_empty() || positives.contains(&current)
}

fn package_supported_on_current_platform(val: &serde_json::Value) -> bool {
    platform_matches(
        val,
        &current_npm_os(),
        &current_npm_cpu(),
        crate::utils::target_libc().as_deref(),
    )
}

/// Native addon names often encode the target (`@next/swc-linux-x64-gnu`,
/// `@esbuild/darwin-arm64`). When those tokens are present they are a stronger
/// signal than a missing `os`/`cpu` field in the lockfile.
pub(crate) fn package_name_matches_current_platform(name: &str) -> bool {
    name_matches_platform(
        name,
        &current_npm_os(),
        &current_npm_cpu(),
        crate::utils::target_libc().as_deref(),
    )
}

fn normalize_os_token(token: &str) -> Option<&'static str> {
    match token {
        "darwin" | "macos" | "osx" => Some("darwin"),
        "linux" => Some("linux"),
        // Musl-flavoured Linux names are single tokens (`sharp-linuxmusl-arm64`), so
        // the plain `linux` arm never sees them - and on a Mac the libc check then
        // had nothing to reject either.
        "linuxmusl" | "linux-musl" => Some("linux"),
        // Other runtimes that can never load here.
        "openharmony" | "harmony" => Some("openharmony"),
        "ios" => Some("ios"),
        "win32" | "windows" | "win" => Some("win32"),
        "android" => Some("android"),
        "freebsd" => Some("freebsd"),
        "netbsd" => Some("netbsd"),
        "openbsd" => Some("openbsd"),
        "sunos" => Some("sunos"),
        "aix" => Some("aix"),
        _ => None,
    }
}

fn normalize_cpu_token(token: &str) -> Option<&'static str> {
    match token {
        "arm64" | "aarch64" => Some("arm64"),
        "x64" | "amd64" | "x86_64" => Some("x64"),
        "ia32" | "x86" | "i386" | "i686" => Some("ia32"),
        "arm" | "armv7" | "armv6" => Some("arm"),
        "ppc64" => Some("ppc64"),
        "s390x" => Some("s390x"),
        "riscv64" => Some("riscv64"),
        "loong64" | "loongarch64" => Some("loong64"),
        // WebAssembly targets: no Node process on a native host can load them, so
        // they must never be selected here (measured: `@img/sharp-wasm32` was
        // installed on macOS because the token was unknown and the name looked
        // platform-neutral).
        "wasm32" | "wasm" => Some("wasm32"),
        _ => None,
    }
}

fn name_matches_platform(name: &str, os: &str, cpu: &str, libc: Option<&str>) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);
    let mut found_os: Vec<&str> = Vec::new();
    let mut found_cpu: Vec<&str> = Vec::new();
    let mut wants_musl = false;
    let mut wants_gnu = false;
    for token in last.split(['-', '_']) {
        if let Some(mapped) = normalize_os_token(token) {
            found_os.push(mapped);
        }
        if let Some(mapped) = normalize_cpu_token(token) {
            found_cpu.push(mapped);
        }
        if token == "musl" || token == "linuxmusl" || token == "linux-musl" {
            wants_musl = true;
        }
        if token == "gnu" || token == "glibc" {
            wants_gnu = true;
        }
    }
    if found_os.is_empty() && found_cpu.is_empty() && !wants_musl && !wants_gnu {
        return true;
    }
    let os_ok = found_os.is_empty() || found_os.contains(&os);
    let cpu_ok = found_cpu.is_empty() || found_cpu.contains(&cpu);
    let libc_ok = match libc {
        Some("musl") if wants_gnu && !wants_musl => false,
        Some(other) if other != "musl" && wants_musl && !wants_gnu => false,
        _ => true,
    };
    os_ok && cpu_ok && libc_ok
}

pub(crate) fn extract_platform_tokens_from_name(name: &str) -> (Vec<String>, Vec<String>) {
    let last = name.rsplit('/').next().unwrap_or(name);
    let mut found_os: Vec<String> = Vec::new();
    let mut found_cpu: Vec<String> = Vec::new();
    for token in last.split(['-', '_']) {
        if let Some(mapped) = normalize_os_token(token) {
            found_os.push(mapped.to_string());
        }
        if let Some(mapped) = normalize_cpu_token(token) {
            found_cpu.push(mapped.to_string());
        }
    }
    (found_os, found_cpu)
}

fn skip_lockfile_materialization(name: &str, val: &serde_json::Value) -> bool {
    if !package_name_matches_current_platform(name) {
        return true;
    }
    let optional = val
        .get("optional")
        .and_then(|o| o.as_bool())
        .unwrap_or(false);
    if optional && !package_supported_on_current_platform(val) {
        return true;
    }
    // `--omit`: the lockfile records `dev: true` on dev-only entries (npm does
    // too, and we now write it), so the tree walk is where omission lands.
    // Two sources are consulted because a lockfile written by an older build
    // or by npm may carry the marker, our own may not yet, and a project can
    // have both patterns:
    //   1. the entry's own `dev` / `optional` flag, and
    //   2. the manifest itself — a package that is declared under
    //      devDependencies and NOT under dependencies is a dev dependency by
    //      definition.
    let omit = crate::omit::current();
    if omit.is_empty() {
        return false;
    }
    if omit.dev && val.get("dev").and_then(|d| d.as_bool()).unwrap_or(false) {
        return true;
    }
    if omit.optional && optional {
        return true;
    }
    if omit.dev || omit.optional {
        if let Some(manifest) = read_manifest_deps(std::path::Path::new("package.json")) {
            let base = name.rsplit('/').next().unwrap_or(name);
            let declared_prod = manifest.dependencies.contains_key(name)
                || manifest.dependencies.contains_key(base);
            let declared_dev =
                manifest.dev.contains_key(name) || manifest.dev.contains_key(base);
            let declared_opt =
                manifest.optional.contains_key(name) || manifest.optional.contains_key(base);
            if omit.dev && declared_dev && !declared_prod {
                return true;
            }
            if omit.optional && declared_opt && !declared_prod {
                return true;
            }
        }
    }
    false
}

/// Pure, env-free platform filter (testable): does `val`'s `os`/`cpu`/`libc`
/// accept the given target platform?
fn platform_matches(val: &serde_json::Value, os: &str, cpu: &str, libc: Option<&str>) -> bool {
    field_allows_current(val.get("os"), os)
        && field_allows_current(val.get("cpu"), cpu)
        && match libc {
            Some(libc) => field_allows_current(val.get("libc"), libc),
            None => true,
        }
}

/// Lockfile kind detected in a directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockfileKind {
    None,
    NpmShrinkwrap,
    Npm,
    Bun,
    Yarn,
    Pnpm,
}

/// Return which lockfile is present in the given directory (package-lock.json or bun.lock).
pub fn detect_lockfile(dir: &Path) -> LockfileKind {
    if dir.join("bun.lock").exists() {
        LockfileKind::Bun
    } else if dir.join("npm-shrinkwrap.json").exists() {
        LockfileKind::NpmShrinkwrap
    } else if dir.join("package-lock.json").exists() {
        LockfileKind::Npm
    } else if dir.join("pnpm-lock.yaml").exists() {
        LockfileKind::Pnpm
    } else if dir.join("yarn.lock").exists() {
        LockfileKind::Yarn
    } else {
        LockfileKind::None
    }
}

/// Read package.json and return (dependencies, devDependencies) as name -> version spec.
use std::sync::{OnceLock, RwLock};
use std::time::SystemTime;

static PKG_JSON_CACHE: OnceLock<RwLock<Option<(SystemTime, HashMap<String, String>)>>> =
    OnceLock::new();

/// Read dependencies from package.json, with caching.
/// The manifest's dependencies that a **registry resolver** is allowed to see.
///
/// `file:`/`link:`/`workspace:`/`git:` specifiers are materialized by the installer's spec-grammar
/// pre-pass; they are not versions and cannot be resolved from a registry. Feeding them to a resolver
/// makes it fetch the package *of that name* from npm and then reject its own answer ("JAGR-∞ legality:
/// local-lib@0.1.0 violates direct range file:../lib"), which broke `update`, `install --lockfile-only`
/// and `why --jagr` on any project with a local or git dependency. One helper, used by every resolver
/// and by prefetch, is what keeps this rule in one place.
pub fn registry_only_deps(path: &Path) -> Option<std::collections::HashMap<String, String>> {
    read_package_json_deps(path).map(|deps| {
        deps.into_iter()
            .filter(|(_, spec)| {
                matches!(
                    crate::spec_grammar::classify(spec),
                    crate::spec_grammar::Spec::Registry(_)
                )
            })
            .collect()
    })
}

pub fn read_package_json_deps(path: &Path) -> Option<HashMap<String, String>> {
    // Check cache
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    let cache = PKG_JSON_CACHE.get_or_init(|| RwLock::new(None));
    if let Ok(guard) = cache.read() {
        if let Some(ref cached) = *guard {
            if cached.0 == mtime {
                return Some(cached.1.clone());
            }
        }
    }
    // Cache miss — read and parse
    let deps = read_package_json_deps_uncached(path)?;
    if let Ok(mut guard) = cache.write() {
        *guard = Some((mtime, deps.clone()));
    }
    Some(deps)
}

/// A manifest's dependency classes, kept separate so an install can omit
/// some (see `crate::omit`) while `doctor`/`audit`/`why` still see the whole
/// picture. `read_package_json_deps` remains the merged view.
#[derive(Debug, Clone, Default)]
pub struct ManifestDeps {
    pub dependencies: HashMap<String, String>,
    pub dev: HashMap<String, String>,
    pub optional: HashMap<String, String>,
    pub peer: HashMap<String, String>,
}

/// Classified read of package.json's dependency sections (no caching: the
/// merged cached reader covers the hot path, and this one runs once per
/// install decision).
pub fn read_manifest_deps(path: &Path) -> Option<ManifestDeps> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let section = |key: &str| -> HashMap<String, String> {
        v.get(key)
            .and_then(|d| d.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(ManifestDeps {
        dependencies: section("dependencies"),
        dev: section("devDependencies"),
        optional: section("optionalDependencies"),
        peer: section("peerDependencies"),
    })
}

/// The specs an install should materialise for this manifest, applying
/// `omit`. Semantics match npm: a package listed in both `dependencies` and
/// `devDependencies` is a *runtime* dependency (the prod section wins), so it
/// survives `--omit=dev`. `optionalDependencies` overlay `dependencies` in
/// npm, but omitting optional must not remove a package that is also a hard
/// dependency.
pub fn deps_for_install(path: &Path, omit: crate::omit::Omit) -> Option<HashMap<String, String>> {
    let m = read_manifest_deps(path)?;
    let mut out = m.dependencies.clone();
    if !omit.optional {
        out.extend(m.optional.clone());
    } else {
        // Optional entries that are NOT also hard deps are dropped; ones that
        // are hard deps stay (they are required, not optional).
        for (k, v) in m.optional.iter() {
            if out.contains_key(k) {
                continue;
            }
            out.insert(k.clone(), v.clone());
        }
        for (k, _) in m.optional.iter() {
            if m.dependencies.contains_key(k) {
                continue;
            }
            out.remove(k);
        }
    }
    if !omit.dev {
        for (k, v) in m.dev.iter() {
            out.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if !omit.peer {
        for (k, v) in m.peer.iter() {
            if !out.contains_key(k) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Some(out)
}

fn read_package_json_deps_uncached(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let mut deps = HashMap::new();
    if let Some(d) = v.get("dependencies").and_then(|d| d.as_object()) {
        for (k, v) in d {
            if let Some(s) = v.as_str() {
                deps.insert(k.clone(), s.to_string());
            }
        }
    }
    if let Some(d) = v.get("devDependencies").and_then(|d| d.as_object()) {
        for (k, v) in d {
            if let Some(s) = v.as_str() {
                deps.insert(k.clone(), s.to_string());
            }
        }
    }
    if let Some(d) = v.get("optionalDependencies").and_then(|d| d.as_object()) {
        for (k, v) in d {
            if let Some(s) = v.as_str() {
                deps.insert(k.clone(), s.to_string());
            }
        }
    }
    Some(deps)
}

/// Parse npm `overrides` / yarn `resolutions` into a `name -> spec` map.
///
/// Both forms are supported: plain strings and npm's nested object form, where
/// `"."` pins the package itself (`{"pkg": {".": "^2"}}`). These force a version
/// for a package anywhere in the tree, so they must be applied by *replacing* the
/// discovered ranges rather than adding to them — adding usually makes the set
/// unsatisfiable and the package drops out of the tree.
pub fn parse_overrides(v: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for key in ["overrides", "resolutions"] {
        let Some(obj) = v.get(key).and_then(|o| o.as_object()) else {
            continue;
        };
        for (name, val) in obj {
            match val {
                serde_json::Value::String(spec) => {
                    out.insert(name.clone(), spec.clone());
                }
                serde_json::Value::Object(inner) => {
                    // npm's nested form carries two meanings at once:
                    //   {"pkg": {".": "^2"}}          -> pin `pkg` itself
                    //   {"pkg": {"child": "^1"}}      -> pin `child` (only when
                    //                                   required by `pkg`)
                    // We record the child overrides globally: the resolver
                    // constrains a package by the union of incoming ranges and has
                    // no per-edge override slot, so a scoped rule cannot be
                    // distinguished from a global one here. Nested rules are
                    // therefore applied to every dependent that requires the child
                    // — never silently dropped.
                    for (key, val) in inner {
                        let Some(spec) = val.as_str() else { continue };
                        if key == "." {
                            out.insert(name.clone(), spec.to_string());
                        } else {
                            out.insert(key.clone(), spec.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// npm `overrides` / yarn `resolutions` declared in the `package.json` at `path`.
pub fn read_overrides(path: &Path) -> std::collections::HashMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return std::collections::HashMap::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => parse_overrides(&v),
        Err(_) => std::collections::HashMap::new(),
    }
}

/// Parse `optionalDependencies` names out of a `package.json` value.
///
/// npm's contract is that an optional dependency which cannot be installed is
/// skipped with a warning — it must never fail the whole install. Dependency
/// readers merge optional deps into the same map as regular ones, so this
/// recovers the distinction where it matters.
pub fn parse_optional_dependency_names(v: &serde_json::Value) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Some(d) = v.get("optionalDependencies").and_then(|d| d.as_object()) {
        for k in d.keys() {
            out.insert(k.clone());
        }
    }
    out
}

/// Names listed under `optionalDependencies` in the `package.json` at `path`.
pub fn read_optional_dependency_names(path: &Path) -> std::collections::HashSet<String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return std::collections::HashSet::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => parse_optional_dependency_names(&v),
        Err(_) => std::collections::HashSet::new(),
    }
}

// ── Shared lockfile content cache ──────────────────────────────────────────
// All lockfile-reading functions share a single mtime-cached file content read.
// This avoids redundant read_to_string syscalls within the same install flow.

static LOCKFILE_CONTENT_CACHE: OnceLock<RwLock<Option<(PathBuf, SystemTime, String)>>> =
    OnceLock::new();
static RESOLVED_MAP_CACHE: OnceLock<
    RwLock<Option<(PathBuf, SystemTime, HashMap<String, String>)>>,
> = OnceLock::new();
static PARSED_INSTALL_ENTRIES_CACHE: OnceLock<
    RwLock<Option<(PathBuf, SystemTime, Vec<LockfileInstallEntry>)>>,
> = OnceLock::new();

fn lockfile_path(dir: &Path) -> Option<PathBuf> {
    let bun_lock = dir.join("bun.lock");
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    let npm_lock = dir.join("package-lock.json");
    // A project locked by yarn or pnpm is installable too: those files carry the
    // same name -> version information, resolved by our own readers.
    let yarn_lock = dir.join("yarn.lock");
    let pnpm_lock = dir.join("pnpm-lock.yaml");
    if npm_shrinkwrap.exists() {
        Some(npm_shrinkwrap)
    } else if npm_lock.exists() {
        Some(npm_lock)
    } else if bun_lock.exists() {
        Some(bun_lock)
    } else if pnpm_lock.exists() {
        Some(pnpm_lock)
    } else if yarn_lock.exists() {
        Some(yarn_lock)
    } else {
        None
    }
}

/// Resolved `name -> version` for any lockfile we understand.
fn resolved_from_lockfile(path: &Path, content: &str) -> Option<HashMap<String, String>> {
    let name = path.to_string_lossy();
    if name.contains("bun.lock") {
        read_bun_lock_resolved_from_str(content)
    } else if name.contains("pnpm-lock.yaml") {
        read_pnpm_lock_resolved_from_str(content)
    } else if name.contains("yarn.lock") {
        read_yarn_lock_resolved_from_str(content)
    } else {
        read_lockfile_resolved_from_str(content)
    }
}

/// Read lockfile content from dir, cached by mtime. Returns (path, content).
/// All subsequent calls within the same process reuse the cached content
/// until the file mtime changes.
fn cached_lockfile_content(dir: &Path) -> Option<(PathBuf, String)> {
    let path = lockfile_path(dir)?;
    let mtime = std::fs::metadata(&path).ok()?.modified().ok()?;
    let cache = LOCKFILE_CONTENT_CACHE.get_or_init(|| RwLock::new(None));
    if let Ok(guard) = cache.read() {
        if let Some((ref cached_path, ref cached_mtime, ref content)) = *guard {
            if cached_path == &path && cached_mtime == &mtime {
                return Some((path, content.clone()));
            }
        }
    }
    let content = std::fs::read_to_string(&path).ok()?;
    if let Ok(mut guard) = cache.write() {
        *guard = Some((path.clone(), mtime, content.clone()));
    }
    Some((path, content))
}

fn read_lockfile_resolved_from_str(s: &str) -> Option<HashMap<String, String>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut resolved = HashMap::new();
    let mut top_level_names = HashSet::new();
    for (key, val) in packages {
        // Skip entries without a version (e.g. the root entry of a
        // `--package-lock-only` lockfile) instead of failing the whole map.
        let Some(version) = val.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        if is_top_level_lockfile_path(key) {
            // This map is necessarily lossy: npm can install multiple versions
            // of a package at different paths. For callers that need one version
            // per name (e.g. checking direct manifest ranges), the root-visible
            // copy must win over any nested transitive copy, independent of JSON
            // key order.
            top_level_names.insert(name.clone());
            resolved.insert(name, version.to_string());
        } else if !top_level_names.contains(&name) {
            resolved.entry(name).or_insert_with(|| version.to_string());
        }
    }
    Some(resolved)
}

/// Normalise Bun's `bun.lock` flavour of JSON into strict JSON.
///
/// Bun writes plain JSON but leaves **trailing commas** (and may emit comments),
/// which `serde_json` rejects — so a valid `bun.lock` failed to parse at all
/// ("Failed to parse bun.lock as JSON lockfile"). Trailing commas before a closing
/// brace/bracket are removed and comments dropped, both string-aware so label text
/// such as `"a,}"` or `"http://x"` is left untouched.
pub fn normalise_jsonc(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut in_string = false;

    while i < n {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < n {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push('"');
                i += 1;
            }
            '/' if i + 1 < n && chars[i + 1] == '/' => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < n && chars[i + 1] == '*' => {
                i += 2;
                while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            ',' => {
                // Look past whitespace/comments for a closing bracket.
                let mut j = i + 1;
                loop {
                    while j < n && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j + 1 < n && chars[j] == '/' && chars[j + 1] == '/' {
                        while j < n && chars[j] != '\n' {
                            j += 1;
                        }
                        continue;
                    }
                    if j + 1 < n && chars[j] == '/' && chars[j + 1] == '*' {
                        j += 2;
                        while j + 1 < n && !(chars[j] == '*' && chars[j + 1] == '/') {
                            j += 1;
                        }
                        j = (j + 2).min(n);
                        continue;
                    }
                    break;
                }
                if !(j < n && (chars[j] == '}' || chars[j] == ']')) {
                    out.push(',');
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn read_bun_lock_resolved_from_str(s: &str) -> Option<HashMap<String, String>> {
    // Bun's file is JSON with trailing commas; fall back to the normalised text.
    let v: serde_json::Value = serde_json::from_str(s)
        .or_else(|_| serde_json::from_str(&normalise_jsonc(s)))
        .ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut resolved = HashMap::new();
    for (key, val) in packages {
        // Bun's `packages` keys are bare names (`"ms"`, `"@scope/pkg"`) and the
        // resolved spec lives in the value — `["ms@2.1.3", registry, deps, hash]`.
        // Some variants store a plain string instead of an array. The old code
        // parsed only the *key* and returned `None` for the whole file as soon as
        // one key had no `@`.
        let spec = val
            .as_array()
            .and_then(|a| a.first())
            .and_then(|s| s.as_str())
            .or_else(|| val.as_str())
            .unwrap_or(key)
            .to_string();
        let rest = spec.strip_prefix("npm:").unwrap_or(&spec);
        let Some(at_pos) = rest.rfind('@') else {
            continue;
        };
        if at_pos == 0 {
            continue;
        }
        let name = rest[..at_pos].to_string();
        let version = rest[at_pos + 1..].to_string();
        if !version.is_empty() && !name.is_empty() {
            resolved.insert(name, version);
        }
    }
    Some(resolved)
}

/// The package name in a yarn descriptor such as `ms@^2.1.3`, `@scope/pkg@npm:^1`.
///
/// Yarn v1 writes `name@range`; Berry writes `name@npm:range` (or
/// `name@workspace:…`). Only the name is taken from the descriptor — the resolved
/// version comes from the block body — so both are handled the same way.
fn yarn_descriptor_name(descriptor: &str) -> Option<String> {
    let d = descriptor.trim().trim_matches('"');
    if d.is_empty() || d.starts_with("__") {
        return None;
    }
    let at = if let Some(stripped) = d.strip_prefix('@') {
        // Scoped: skip the scope segment, then find the version separator.
        stripped.find('@').map(|p| p + 1)?
    } else {
        d.find('@')?
    };
    let name = d[..at].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// The version recorded on a block body line (`version "2.1.3"`, `version: 2.1.3`).
fn yarn_version_value(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("version")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim();
    let v = rest.trim_matches('"').trim_matches('\'').trim();
    if v.is_empty() || v.contains(' ') {
        None
    } else {
        Some(v.to_string())
    }
}

/// Read a `yarn.lock` (v1 and Berry are both line-based) into `name -> version`.
///
/// A descriptor line has no leading whitespace and ends with `:` (it may list
/// several comma-separated descriptors); the resolved `version` sits on an
/// indented line inside that block.
fn read_yarn_lock_resolved_from_str(s: &str) -> Option<HashMap<String, String>> {
    let mut resolved: HashMap<String, String> = HashMap::new();
    let mut current: Option<String> = None;

    for raw in s.lines() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        if !indented {
            if let Some(descriptors) = raw.trim_end().strip_suffix(':') {
                current = descriptors.split(',').next().and_then(yarn_descriptor_name);
            }
            continue;
        }
        if let (Some(name), Some(version)) = (&current, yarn_version_value(raw)) {
            resolved.insert(name.clone(), version);
            // One version per block; wait for the next descriptor line.
            current = None;
        }
    }

    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

/// Read a `pnpm-lock.yaml` into `name -> version`.
///
/// Only the `packages:` section is needed, and it is line-oriented in every pnpm
/// generation: pnpm 6/8 write `  /name@version:` and pnpm 9 writes
/// `  name@version:`, both as direct children (two-space indent) of `packages:`
/// (pnpm 9 moves the direct-dependency list into an `importers:` block, which we
/// do not need). Peer-suffixed keys (`name@1.2.3(peer@2)`, `name@1.2.3_peer@2`)
/// and non-registry entries (`file:`/`link:`) are handled/skipped rather than
/// producing bogus versions.
fn read_pnpm_lock_resolved_from_str(s: &str) -> Option<HashMap<String, String>> {
    let mut resolved: HashMap<String, String> = HashMap::new();
    let mut in_packages = false;

    for raw in s.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            in_packages = line.trim() == "packages:";
            continue;
        }
        // Only the direct children of `packages:` carry the name@version key.
        if !in_packages || indent != 2 {
            continue;
        }
        let Some(key) = line.trim().strip_suffix(':') else {
            continue;
        };
        // YAML may quote the key with either quote style (`'@scope/pkg@1.2.3'`).
        let key = key.trim_matches(|c| c == '"' || c == '\'');
        let key = key.strip_prefix('/').unwrap_or(key);
        let key = key.split('(').next().unwrap_or(key); // v8/v9 peer suffix
        let key = match key.find('_') {
            // v6 wrote `/name@1.2.3_peer@2.0.0`
            Some(p) if key[p + 1..].contains('@') => &key[..p],
            _ => key,
        };
        let Some(at) = key.rfind('@') else {
            continue;
        };
        if at == 0 {
            continue;
        }
        let name = &key[..at];
        let version = &key[at + 1..];
        if name.is_empty() || version.is_empty() {
            continue;
        }
        // `file:`/`link:`/`workspace:` entries are not registry versions.
        if version.contains(':') || version.contains('/') {
            continue;
        }
        resolved.insert(name.to_string(), version.to_string());
    }

    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

/// Read a `pnpm-lock.yaml` and return resolved versions: name -> exact version.
pub fn read_pnpm_lock_resolved(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    read_pnpm_lock_resolved_from_str(&s)
}

/// Read a `yarn.lock` and return resolved versions: package name -> exact version.
pub fn read_yarn_lock_resolved(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    read_yarn_lock_resolved_from_str(&s)
}

/// Read package-lock.json and return resolved versions: package name -> exact version.
/// Supports lockfileVersion 2 and 3 (packages key).
pub fn read_lockfile_resolved(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    read_lockfile_resolved_from_str(&s)
}

/// Read bun.lock (text JSON format) and return resolved versions: package name -> exact version.
pub fn read_bun_lock_resolved(path: &Path) -> Option<HashMap<String, String>> {
    let s = std::fs::read_to_string(path).ok()?;
    read_bun_lock_resolved_from_str(&s)
}

/// Merge package.json deps with lockfile: for each dep, use lockfile version if present.
/// Returns list of "name@version" for install.
pub fn resolve_deps_for_install(
    package_json_deps: &HashMap<String, String>,
    lockfile_resolved: Option<&HashMap<String, String>>,
) -> Vec<String> {
    let mut names: Vec<&String> = package_json_deps.keys().collect();
    names.sort();

    let mut out = Vec::with_capacity(package_json_deps.len());
    for name in names {
        let spec = package_json_deps.get(name).cloned().unwrap_or_default();
        let version = lockfile_resolved
            .and_then(|r| r.get(name).cloned())
            .unwrap_or(spec);
        out.push(format!("{}@{}", name, version));
    }
    out
}

/// Read resolved versions from whichever lockfile exists in dir (package-lock.json or bun.lock).
pub fn read_resolved_from_dir(dir: &Path) -> Option<HashMap<String, String>> {
    let path = lockfile_path(dir)?;
    let mtime = std::fs::metadata(&path).ok()?.modified().ok()?;
    let cache = RESOLVED_MAP_CACHE.get_or_init(|| RwLock::new(None));
    if let Ok(guard) = cache.read() {
        if let Some((ref cached_path, ref cached_mtime, ref map)) = *guard {
            if cached_path == &path && cached_mtime == &mtime {
                return Some(map.clone());
            }
        }
    }
    let (path, content) = cached_lockfile_content(dir)?;
    let map = resolved_from_lockfile(&path, &content)?;
    if let Ok(mut guard) = cache.write() {
        *guard = Some((path, mtime, map.clone()));
    }
    Some(map)
}

/// Read package-lock.json and return map name@version -> resolved tarball URL (for zero-packument install).
pub fn read_lockfile_resolved_urls(path: &Path) -> Option<HashMap<String, String>> {
    read_lockfile_resolved_urls_with_integrity(path).map(|(urls, _)| urls)
}

fn read_lockfile_resolved_urls_with_integrity_from_str(
    s: &str,
) -> Option<(HashMap<String, String>, HashMap<String, String>)> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut urls = HashMap::new();
    let mut integrity = HashMap::new();
    for (key, val) in packages {
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        let Some(version) = val.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        // Entries without `resolved` (root, link:/file: packages) carry no
        // tarball URL; skip them rather than discarding every other entry.
        let Some(resolved) = val.get("resolved").and_then(|v| v.as_str()) else {
            continue;
        };
        if resolved.ends_with(".tgz") {
            let pkg_key = format!("{}@{}", name, version);
            urls.insert(pkg_key.clone(), resolved.to_string());
            if let Some(sri) = val.get("integrity").and_then(|i| i.as_str()) {
                integrity.insert(pkg_key, sri.to_string());
            }
        }
    }
    Some((urls, integrity))
}

/// Read package-lock.json and return (urls, integrity) maps. Integrity key is name@version -> SRI string.
pub fn read_lockfile_resolved_urls_with_integrity(
    path: &Path,
) -> Option<(HashMap<String, String>, HashMap<String, String>)> {
    let s = std::fs::read_to_string(path).ok()?;
    read_lockfile_resolved_urls_with_integrity_from_str(&s)
}

/// Build npm registry tarball URL for a package version (no packument needed).
/// Scoped: @scope/pkg -> https://registry.npmjs.org/@scope%2Fpkg/-/pkg-1.0.0.tgz
pub fn tarball_url_from_registry(name: &str, version: &str) -> String {
    let registry = crate::config::effective_registry_url(Path::new("."));
    let encoded = if name.starts_with('@') {
        name.replace('/', "%2F")
    } else {
        name.to_string()
    };
    let tarball_name = if name.starts_with('@') {
        name.split('/').next_back().unwrap_or(name).to_string()
    } else {
        name.to_string()
    };
    format!(
        "{}/{}/-/{}-{}.tgz",
        registry.trim_end_matches('/'),
        encoded,
        tarball_name,
        version
    )
}

/// Read resolved tarball URLs from dir: package-lock has "resolved"; bun.lock we build via tarball_url_from_registry.
pub fn read_resolved_urls_from_dir(dir: &Path) -> Option<HashMap<String, String>> {
    read_resolved_urls_and_integrity_from_dir(dir).map(|(urls, _)| urls)
}

/// Read resolved URLs and integrity (when available, e.g. package-lock.json). For bun.lock, integrity is empty.
pub fn read_resolved_urls_and_integrity_from_dir(
    dir: &Path,
) -> Option<(HashMap<String, String>, HashMap<String, String>)> {
    let (path, content) = cached_lockfile_content(dir)?;
    let is_bun = path.to_string_lossy().contains("bun.lock");
    if is_bun {
        let resolved = read_bun_lock_resolved_from_str(&content)?;
        let mut urls = HashMap::new();
        for (name, version) in resolved {
            let spec = format!("{}@{}", name, version);
            urls.insert(spec, tarball_url_from_registry(&name, &version));
        }
        Some((urls, HashMap::new()))
    } else {
        read_lockfile_resolved_urls_with_integrity_from_str(&content)
    }
}

/// Check whether all resolved lockfile entries include integrity strings (package-lock.json only).
/// SRI hashes (`name -> "sha512-…"`) declared by whichever lockfile is present.
///
/// All four formats carry real hashes — npm and bun as a JSON field, yarn v1 as
/// `integrity sha512-…,` inside the descriptor block, and pnpm as
/// `resolution: {integrity: …}` — so strict mode can *verify* downloads for a
/// yarn/pnpm/bun project instead of refusing it. Berry records a `checksum:`
/// instead, which is not SRI, so such a project reports none.
pub fn read_lockfile_integrities(dir: &Path) -> Option<HashMap<String, String>> {
    let (path, content) = cached_lockfile_content(dir)?;
    let name = path.to_string_lossy();
    let mut out: HashMap<String, String> = HashMap::new();

    if name.contains("bun.lock") {
        let v: serde_json::Value = serde_json::from_str(&content)
            .or_else(|_| serde_json::from_str(&normalise_jsonc(&content)))
            .ok()?;
        for (key, val) in v.get("packages")?.as_object()? {
            let spec = val
                .as_array()
                .and_then(|a| a.first())
                .and_then(|s| s.as_str())
                .unwrap_or(key);
            let Some(pkg) = spec.rsplit_once('@').map(|(n, _)| n.to_string()) else {
                continue;
            };
            if let Some(hash) = val
                .as_array()
                .and_then(|a| a.get(3))
                .and_then(|h| h.as_str())
                .filter(|h| h.starts_with("sha"))
            {
                out.insert(pkg, hash.to_string());
            }
        }
    } else if name.contains("yarn.lock") {
        // yarn v1 blocks: `name@range:` … `  integrity sha512-…`
        let mut pkg: Option<String> = None;
        for raw in content.lines() {
            if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
                continue;
            }
            let indented = raw.starts_with(' ') || raw.starts_with('\t');
            if !indented {
                pkg = raw
                    .trim_end()
                    .strip_suffix(':')
                    .and_then(|d| d.split(',').next())
                    .and_then(yarn_descriptor_name);
            } else if let (Some(p), Some(rest)) = (
                pkg.as_ref(),
                raw.trim().strip_prefix("integrity").map(str::trim),
            ) {
                let hash = rest.trim_matches(|c| c == '"' || c == '\'').trim();
                if hash.starts_with("sha") {
                    out.insert(p.clone(), hash.to_string());
                }
            }
        }
    } else if name.contains("pnpm-lock.yaml") {
        let mut pkg: Option<String> = None;
        for raw in content.lines() {
            let line = raw.trim_end();
            if line.trim().is_empty() {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            if indent == 2 {
                pkg = None;
                if let Some(key) = line.trim().strip_suffix(':') {
                    let key = key.trim_matches(|c| c == '"' || c == '\'');
                    let key = key.strip_prefix('/').unwrap_or(key);
                    let key = key.split('(').next().unwrap_or(key);
                    if let Some((n, _)) = key.rsplit_once('@') {
                        if !n.is_empty() {
                            pkg = Some(n.to_string());
                        }
                    }
                }
            } else if let (Some(p), Some(idx)) = (pkg.as_ref(), line.find("integrity:")) {
                let hash = line[idx + "integrity:".len()..]
                    .trim()
                    .trim_end_matches('}')
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'');
                if hash.starts_with("sha") {
                    out.insert(p.clone(), hash.to_string());
                }
            }
        }
    } else {
        // npm / npm-shrinkwrap
        let v: serde_json::Value = serde_json::from_str(&content).ok()?;
        for (key, val) in v.get("packages")?.as_object()? {
            let Some(pkg) = package_name_from_lockfile_key(key) else {
                continue;
            };
            if let Some(hash) = val.get("integrity").and_then(|i| i.as_str()) {
                out.insert(pkg, hash.to_string());
            }
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Names in `package-lock.json` that are **materialized** rather than downloaded:
/// `link: true`, or a `resolved` that is a `file:`/`git…` spec instead of a tarball
/// URL. npm records no SRI for a linked directory or a git checkout, so strict-mode
/// integrity checks must not demand one — otherwise `jhol ci` fails on a project
/// that is perfectly reproducible (a single `file:` or git dependency was enough).
pub fn materialized_package_names(dir: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(path) = ["package-lock.json", "npm-shrinkwrap.json"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
    else {
        return out;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return out;
    };
    let Some(packages) = v.get("packages").and_then(|p| p.as_object()) else {
        return out;
    };
    for (key, entry) in packages {
        if key.is_empty() {
            continue;
        }
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        let linked = entry.get("link").and_then(|l| l.as_bool()) == Some(true);
        let resolved = entry.get("resolved").and_then(|r| r.as_str()).unwrap_or("");
        let not_a_download = resolved.starts_with("file:")
            || resolved.starts_with("git+")
            || resolved.starts_with("git:")
            || (!resolved.is_empty() && !resolved.ends_with(".tgz") && !resolved.contains(".tgz"));
        if linked || not_a_download {
            out.insert(name);
        }
    }
    out
}

pub fn lockfile_integrity_complete(dir: &Path) -> bool {
    let Some(resolved) = read_resolved_from_dir(dir) else {
        return true; // no lockfile at all — nothing to verify
    };
    if resolved.is_empty() {
        return true;
    }
    // Every *downloaded* package must carry a hash. Two exemptions:
    //  * a materialized dependency (link / `file:` / git / alias) has no SRI —
    //    npm records none for a linked directory or a checkout;
    //  * a lockfile with no registry tarballs at all reports no hashes (`None`),
    //    which is not the same as "hashes missing": a project built purely from
    //    local, workspace and git dependencies is perfectly reproducible.
    // Formats that genuinely carry no SRI (Berry `checksum:`) still fail, because
    // neither exemption applies to their resolved registry entries.
    let materialized = materialized_package_names(dir);
    let hashes = read_lockfile_integrities(dir).unwrap_or_default();

    // A package for another OS/CPU/libc is never downloaded, so it can never carry
    // a hash we could check. Requiring one made `jhol ci` refuse a perfectly good
    // `vite` tree: 21 `@esbuild/*` entries for other platforms, no integrity, and
    // a strict-lockfile error about "integrity entries missing".
    let platform_inapplicable = lockfile_platform_inapplicable(dir, &resolved);

    resolved.keys().all(|name| {
        hashes.contains_key(name)
            || materialized.contains(name)
            || platform_inapplicable.contains(name)
    })
}

/// Names whose lockfile entry declares a platform this machine is not.
fn lockfile_platform_inapplicable(
    dir: &Path,
    resolved: &HashMap<String, String>,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(path) = ["package-lock.json", "npm-shrinkwrap.json", "jhol-lock.json"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
    else {
        return out;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return out;
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return out;
    };
    let Some(packages) = doc.get("packages").and_then(|p| p.as_object()) else {
        return out;
    };
    for (key, entry) in packages {
        if !key.contains("node_modules/") || jhol_treecheck::entry_platform_applies(entry) {
            continue;
        }
        if let Some(idx) = key.rfind("node_modules/") {
            let name = &key[idx + "node_modules/".len()..];
            if resolved.contains_key(name) {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// Read all resolved specs (name@version) from lockfile in dir.
/// For package-lock.json: uses `packages` entries with `version`.
/// For bun.lock: uses parsed resolved name/version pairs.
pub fn read_all_resolved_specs_from_dir(dir: &Path) -> Option<Vec<String>> {
    static CACHE: std::sync::OnceLock<
        std::sync::RwLock<Option<(std::time::SystemTime, Vec<String>)>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::RwLock::new(None));

    // Find the actual lockfile path and its mtime
    let npm_shrinkwrap = dir.join("npm-shrinkwrap.json");
    let npm_lock = dir.join("package-lock.json");
    let bun_lock = dir.join("bun.lock");
    let pnpm_lock = dir.join("pnpm-lock.yaml");
    let yarn_lock = dir.join("yarn.lock");

    let target = if npm_shrinkwrap.exists() {
        Some(npm_shrinkwrap)
    } else if npm_lock.exists() {
        Some(npm_lock)
    } else if bun_lock.exists() {
        Some(bun_lock)
    } else if pnpm_lock.exists() {
        Some(pnpm_lock)
    } else if yarn_lock.exists() {
        Some(yarn_lock)
    } else {
        None
    };
    let (target_path, mtime) = {
        let p = target?;
        let m = std::fs::metadata(&p).ok()?.modified().ok()?;
        (p, m)
    };

    // Check cache
    if let Ok(guard) = cache.read() {
        if let Some((cached_mtime, ref cached)) = *guard {
            if cached_mtime == mtime {
                return Some(cached.clone());
            }
        }
    }

    // Read and cache
    // Any lockfile we understand: npm keeps resolved URLs, the rest give
    // name -> version, which is all the spec list needs.
    let resolved = resolved_from_lockfile(&target_path, &cached_lockfile_content(dir)?.1)?;
    let mut specs: Vec<String> = resolved
        .into_iter()
        .map(|(name, version)| format!("{}@{}", name, version))
        .collect();
    specs.sort();
    specs.dedup();

    if let Ok(mut guard) = cache.write() {
        *guard = Some((mtime, specs.clone()));
    }

    Some(specs)
}

/// True when the project's own `package-lock.json` is usable for a
/// lockfile-driven install.
///
/// Deliberately inspects **only** `package-lock.json`:
/// - `bun.lock` alone is not consumable by the npm lockfile reader, so it must
///   not switch an install into lockfile mode (bun is handled via the backend /
///   `import-lock` path).
/// - `npm-shrinkwrap.json` is owned by npm and ignored for jhol detection.
///
/// A `package-lock.json` counts as usable when it carries a `jhol` marker (a
/// jhol-written lockfile is authoritative even for a dependency-free project)
/// or when it has at least one installable `node_modules/*` entry (an
/// npm-written lockfile is authoritative without any jhol marker).
pub fn package_lock_is_usable(dir: &Path) -> bool {
    let path = dir.join("package-lock.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
        if v.get("jhol").is_some() {
            return true;
        }
    }
    read_npm_lock_install_entries_from_str(&content)
        .map(|entries| !entries.is_empty())
        .unwrap_or(false)
}

fn read_npm_lock_install_entries_from_str(s: &str) -> Option<Vec<LockfileInstallEntry>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut entries = Vec::new();

    for (key, val) in packages {
        if key.is_empty() {
            continue;
        }
        if !is_safe_lockfile_path(key) {
            continue;
        }
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        let Some(version) = val.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let version = version.to_string();
        if skip_lockfile_materialization(&name, val) {
            continue;
        }
        // Local packages (`file:`/`link:`/`workspace:`) are symlinked by the
        // installer's local-dependency pre-pass. Their `resolved` is a filesystem
        // path, not a download URL, so treating them as installable entries makes
        // the installer try to fetch `file:../x` as a URL.
        if val.get("link").and_then(|l| l.as_bool()).unwrap_or(false)
            || val
                .get("resolved")
                .and_then(|r| r.as_str())
                .map(|r| r.starts_with("file:"))
                .unwrap_or(false)
        {
            continue;
        }
        let resolved = val
            .get("resolved")
            .and_then(|r| r.as_str())
            .filter(|url| !url.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| tarball_url_from_registry(&name, &version));
        let integrity = val
            .get("integrity")
            .and_then(|i| i.as_str())
            .map(|s| s.to_string());
        let install_path = normalize_lock_install_path(key);

        entries.push(LockfileInstallEntry {
            package: name.clone(),
            version: version.clone(),
            spec: format!("{}@{}", name, version),
            resolved,
            integrity,
            install_path: install_path.clone(),
            top_level: is_top_level_lockfile_path(&install_path),
        });
    }

    entries.sort_by(|a, b| {
        a.install_path
            .cmp(&b.install_path)
            .then(a.package.cmp(&b.package))
            .then(a.version.cmp(&b.version))
    });
    Some(entries)
}

/// Full per-package lockfile data (npm package-lock.json packages entries).
/// Carries dependency maps so a resolved tree can be rebuilt without metadata.
pub struct LockfileFullEntry {
    pub package: String,
    pub version: String,
    pub resolved: String,
    pub integrity: Option<String>,
    pub dependencies: HashMap<String, String>,
    pub peer_dependencies: HashMap<String, String>,
    pub peer_dependencies_meta: HashMap<String, serde_json::Value>,
}

fn read_lockfile_full_entries_from_str(s: &str) -> Option<Vec<LockfileFullEntry>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let packages = v.get("packages")?.as_object()?;
    let mut entries = Vec::new();
    for (key, val) in packages {
        if key.is_empty() {
            continue;
        }
        if !is_safe_lockfile_path(key) {
            continue;
        }
        let Some(name) = package_name_from_lockfile_key(key) else {
            continue;
        };
        let Some(version) = val.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let version = version.to_string();
        if skip_lockfile_materialization(&name, val) {
            continue;
        }
        let resolved = val
            .get("resolved")
            .and_then(|r| r.as_str())
            .filter(|url| !url.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| tarball_url_from_registry(&name, &version));
        let integrity = val
            .get("integrity")
            .and_then(|i| i.as_str())
            .map(|s| s.to_string());

        let mut dependencies = HashMap::new();
        if let Some(deps) = val.get("dependencies").and_then(|d| d.as_object()) {
            for (dn, dv) in deps {
                if let Some(ds) = dv.as_str() {
                    dependencies.insert(dn.clone(), ds.to_string());
                }
            }
        }
        let mut peer_dependencies = HashMap::new();
        if let Some(peers) = val.get("peerDependencies").and_then(|p| p.as_object()) {
            for (pn, pv) in peers {
                if let Some(ps) = pv.as_str() {
                    peer_dependencies.insert(pn.clone(), ps.to_string());
                }
            }
        }
        let peer_dependencies_meta = val
            .get("peerDependenciesMeta")
            .and_then(|m| m.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, vv)| (k.clone(), vv.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        entries.push(LockfileFullEntry {
            package: name,
            version,
            resolved,
            integrity,
            dependencies,
            peer_dependencies,
            peer_dependencies_meta,
        });
    }
    entries.sort_by(|a, b| a.package.cmp(&b.package).then(a.version.cmp(&b.version)));
    Some(entries)
}

/// Read full lockfile package data from dir (npm package-lock.json only; bun.lock returns None).
pub fn read_lockfile_full_entries_from_dir(dir: &Path) -> Option<Vec<LockfileFullEntry>> {
    let (path, content) = cached_lockfile_content(dir)?;
    if path.to_string_lossy().contains("bun.lock") {
        return None;
    }
    read_lockfile_full_entries_from_str(&content)
}

pub fn read_npm_lock_install_entries(path: &Path) -> Option<Vec<LockfileInstallEntry>> {
    let s = std::fs::read_to_string(path).ok()?;
    read_npm_lock_install_entries_from_str(&s)
}

pub fn read_lockfile_install_entries_from_dir(dir: &Path) -> Option<Vec<LockfileInstallEntry>> {
    let path = lockfile_path(dir)?;
    let mtime = std::fs::metadata(&path).ok()?.modified().ok()?;
    let cache = PARSED_INSTALL_ENTRIES_CACHE.get_or_init(|| RwLock::new(None));
    if let Ok(guard) = cache.read() {
        if let Some((ref cached_path, ref cached_mtime, ref entries)) = *guard {
            if cached_path == &path && cached_mtime == &mtime {
                return Some(entries.clone());
            }
        }
    }
    // Binary lockfile cache (sub-millisecond deserialization across processes via bincode)
    let bin_key = {
        let path_str = path.to_string_lossy();
        let dur = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let h = xxhash_rust::xxh3::xxh3_64(path_str.as_bytes()) ^ (dur as u64);
        format!("{:016x}.bin", h)
    };
    let bin_path = PathBuf::from(crate::utils::get_cache_dir())
        .join("lockfiles")
        .join(&bin_key);
    if let Ok(bin_data) = std::fs::read(&bin_path) {
        if let Ok(entries) = bincode::deserialize::<Vec<LockfileInstallEntry>>(&bin_data) {
            if let Ok(mut guard) = cache.write() {
                *guard = Some((path, mtime, entries.clone()));
            }
            return Some(entries);
        }
    }
    let (path, content) = cached_lockfile_content(dir)?;
    let name = path.to_string_lossy();
    // npm's own lockfile carries resolved URLs + integrity, so prefer it. The
    // other three only give name -> version, which is enough: the tarball URL
    // follows from the registry.
    let entries_opt = if name.contains("bun.lock")
        || name.contains("yarn.lock")
        || name.contains("pnpm-lock.yaml")
    {
        let resolved = resolved_from_lockfile(&path, &content)?;
        let mut entries = Vec::new();
        for (name, version) in resolved {
            if !package_name_matches_current_platform(&name) {
                continue;
            }
            let top = format!("node_modules/{}", name);
            entries.push(LockfileInstallEntry {
                package: name.clone(),
                version: version.clone(),
                spec: format!("{}@{}", name, version),
                resolved: tarball_url_from_registry(&name, &version),
                integrity: None,
                install_path: top,
                top_level: true,
            });
        }
        entries.sort_by(|a, b| {
            a.install_path
                .cmp(&b.install_path)
                .then(a.package.cmp(&b.package))
                .then(a.version.cmp(&b.version))
        });
        Some(entries)
    } else {
        read_npm_lock_install_entries_from_str(&content)
    };

    if let Some(ref entries) = entries_opt {
        if let Ok(bin_data) = bincode::serialize(entries) {
            crate::origin::write_file_atomically(&bin_path, &bin_data);
        }
        if let Ok(mut guard) = cache.write() {
            *guard = Some((path, mtime, entries.clone()));
        }
    }
    entries_opt
}

#[cfg(test)]
mod tests {
    /// A materialized dependency (`file:`, git, an `npm:` alias) has no SRI, and npm
    /// records none for a linked directory or a checkout either. Demanding one made
    /// strict mode reject a perfectly reproducible project — a single `file:` or git
    /// dependency was enough to break `jhol ci`.
    #[test]
    fn integrity_complete_ignores_materialized_entries_only() {
        let td = tempfile::tempdir().unwrap();
        let lock = td.path().join("package-lock.json");
        let write = |extra: &str| {
            std::fs::write(
                &lock,
                format!(
                    r#"{{"lockfileVersion":3,"packages":{{
                        "":{{"name":"app","version":"1.0.0"}},
                        "node_modules/ms":{{"version":"2.1.3","resolved":"https://registry.npmjs.org/ms/-/ms-2.1.3.tgz","integrity":"sha512-a=="}},
                        "node_modules/local-lib":{{"version":"1.5.0","resolved":"file:../lib","link":true}},
                        "node_modules/git-dep":{{"version":"2.2.2","resolved":"git+file:///tmp/r","link":true}},
                        "node_modules/my-lodash":{{"name":"lodash","version":"4.17.21","resolved":"https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz","link":true}}
                        {extra}}}}}"#
                ),
            )
            .unwrap();
        };

        write("");
        let names = crate::lockfile::materialized_package_names(td.path());
        assert!(names.contains("local-lib"), "{names:?}");
        assert!(names.contains("git-dep"), "{names:?}");
        assert!(names.contains("my-lodash"), "{names:?}");
        assert!(
            !names.contains("ms"),
            "a tarball is not materialized: {names:?}"
        );
        assert!(
            crate::lockfile::lockfile_integrity_complete(td.path()),
            "materialized entries must not be required to carry SRI"
        );

        // A project built ONLY from materialized deps reports no hashes at all
        // (`None`), which used to fail the check before the exemption was ever
        // consulted — the shape that broke `ci` in the offline QA pass.
        std::fs::write(
            &lock,
            r#"{"lockfileVersion":3,"packages":{
                "":{"name":"app","version":"1.0.0"},
                "node_modules/local-lib":{"version":"1.5.0","resolved":"file:../lib","link":true},
                "node_modules/git-dep":{"version":"2.2.2","resolved":"git+file:///tmp/r","link":true}
            }}"#,
        )
        .unwrap();
        assert!(
            crate::lockfile::lockfile_integrity_complete(td.path()),
            "an all-materialized lockfile has nothing to hash and must pass"
        );

        // Control: a *downloaded* package without integrity is still incomplete.
        write(
            r#","node_modules/other":{"version":"1.0.0","resolved":"https://registry.npmjs.org/other/-/other-1.0.0.tgz"}"#,
        );
        assert!(
            !crate::lockfile::lockfile_integrity_complete(td.path()),
            "a downloaded package with no integrity must still fail the check"
        );
    }

    use super::package_name_from_lockfile_key;
    use std::collections::HashMap;

    #[test]
    fn empty_resolved_url_uses_registry_tarball_url() {
        let lockfile = r#"{
            "lockfileVersion": 3,
            "packages": {
                "": { "name": "fixture" },
                "node_modules/lodash": { "version": "4.17.21", "resolved": "" }
            }
        }"#;

        let entries = super::read_npm_lock_install_entries_from_str(lockfile).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].resolved,
            super::tarball_url_from_registry("lodash", "4.17.21")
        );
    }

    #[test]
    fn parse_lockfile_key_nested() {
        assert_eq!(
            package_name_from_lockfile_key("node_modules/a/node_modules/b"),
            Some("b".to_string())
        );
    }

    #[test]
    fn parse_lockfile_key_scoped_nested() {
        assert_eq!(
            package_name_from_lockfile_key("node_modules/a/node_modules/@scope/pkg"),
            Some("@scope/pkg".to_string())
        );
    }

    #[test]
    fn resolved_map_prefers_top_level_version_over_nested_copy() {
        let lockfile = r#"{
            "lockfileVersion": 3,
            "packages": {
                "": { "name": "fixture", "version": "1.0.0" },
                "node_modules/debug": { "version": "2.6.9" },
                "node_modules/https-proxy-agent/node_modules/debug": { "version": "4.4.3" }
            }
        }"#;

        let resolved = super::read_lockfile_resolved_from_str(lockfile).unwrap();
        assert_eq!(resolved.get("debug").map(String::as_str), Some("2.6.9"));
    }

    #[test]
    fn resolved_map_survives_entries_without_version() {
        // npm's `--package-lock-only` output can omit the version on the root
        // entry; one such entry used to abort the whole map via `?`, which made
        // `jhol why` claim "No lockfile found" on a perfectly good lockfile.
        let lockfile = r#"{
            "lockfileVersion": 3,
            "packages": {
                "": { "name": "fixture", "dependencies": { "left-pad": "^1.3.0" } },
                "node_modules/left-pad": { "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz" }
            }
        }"#;

        let resolved = super::read_lockfile_resolved_from_str(lockfile).unwrap();
        assert_eq!(resolved.get("left-pad").map(String::as_str), Some("1.3.0"));
    }

    #[test]
    fn resolved_urls_survives_entries_without_resolved() {
        // link:/file: entries have no `resolved` tarball URL; they must not
        // poison the URL map for every other package.
        let lockfile = r#"{
            "lockfileVersion": 3,
            "packages": {
                "": { "name": "fixture" },
                "node_modules/local": { "version": "1.0.0", "link": true },
                "node_modules/left-pad": { "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz", "integrity": "sha512-x" }
            }
        }"#;

        let (urls, integrity) =
            super::read_lockfile_resolved_urls_with_integrity_from_str(lockfile).unwrap();
        assert_eq!(
            urls.get("left-pad@1.3.0").map(String::as_str),
            Some("https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz")
        );
        assert_eq!(
            integrity.get("left-pad@1.3.0").map(String::as_str),
            Some("sha512-x")
        );
    }

    #[test]
    fn parse_lockfile_key_redundant_slashes() {
        assert_eq!(
            package_name_from_lockfile_key("//node_modules/a//node_modules//@scope/pkg//"),
            Some("@scope/pkg".to_string())
        );
    }

    #[test]
    fn top_level_path_detection() {
        assert!(super::is_top_level_lockfile_path("node_modules/react"));
        assert!(!super::is_top_level_lockfile_path(
            "node_modules/a/node_modules/react"
        ));
    }

    #[test]
    fn safe_path_rejects_parent_dir() {
        assert!(!super::is_safe_lockfile_path("../node_modules/react"));
        assert!(super::is_safe_lockfile_path("node_modules/react"));
    }

    #[test]
    fn resolve_deps_for_install_is_stable_sorted() {
        let mut deps = HashMap::new();
        deps.insert("z".to_string(), "^1.0.0".to_string());
        deps.insert("a".to_string(), "^1.0.0".to_string());
        let out = super::resolve_deps_for_install(&deps, None);
        assert_eq!(out, vec!["a@^1.0.0".to_string(), "z@^1.0.0".to_string()]);
    }

    #[test]
    fn platform_filter_skips_optional_unsupported() {
        let v = serde_json::json!({
            "optional": true,
            "os": ["!darwin"],
        });
        let should_install = super::package_supported_on_current_platform(&v)
            || !v.get("optional").and_then(|o| o.as_bool()).unwrap_or(false);
        // Test is deterministic on macOS hosts, still valid elsewhere.
        if super::current_npm_os() == "darwin" {
            assert!(!should_install);
        }
    }
}

#[cfg(test)]
mod platform_filter_tests {
    use super::*;

    fn tarball(os: Option<&str>, cpu: Option<&str>, libc: Option<&str>) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        if let Some(os) = os {
            m.insert("os".into(), serde_json::json!([os]));
        }
        if let Some(cpu) = cpu {
            m.insert("cpu".into(), serde_json::json!([cpu]));
        }
        if let Some(libc) = libc {
            m.insert("libc".into(), serde_json::json!([libc]));
        }
        serde_json::Value::Object(m)
    }

    #[test]
    fn platform_filter_selects_matching_variant_only() {
        // The exact cross-platform scenario E20/E21 must guarantee: a macOS
        // lockfile drives a Linux install without pulling the darwin binary.
        let darwin_arm64 = tarball(Some("darwin"), Some("arm64"), None);
        let linux_gnu = tarball(Some("linux"), Some("x64"), Some("glibc"));
        let linux_musl = tarball(Some("linux"), Some("x64"), Some("musl"));
        let win_x64 = tarball(Some("win32"), Some("x64"), None);
        let universal = tarball(None, None, None);

        assert!(platform_matches(&darwin_arm64, "darwin", "arm64", None));
        assert!(!platform_matches(
            &darwin_arm64,
            "linux",
            "x64",
            Some("glibc")
        ));

        assert!(platform_matches(&linux_gnu, "linux", "x64", Some("glibc")));
        assert!(platform_matches(&linux_musl, "linux", "x64", Some("musl")));
        assert!(!platform_matches(&linux_gnu, "linux", "x64", Some("musl")));
        assert!(!platform_matches(
            &linux_musl,
            "linux",
            "x64",
            Some("glibc")
        ));

        assert!(!platform_matches(&win_x64, "linux", "x64", None));

        // No constraints => installable anywhere (and libc is ignored).
        assert!(platform_matches(&universal, "linux", "x64", Some("musl")));
        assert!(platform_matches(&universal, "darwin", "arm64", None));
    }

    #[test]
    fn supports_negated_platform_lists() {
        // `"os": ["!win32"]` means "anything but win32".
        let not_win = serde_json::json!({ "os": ["!win32"] });
        assert!(platform_matches(&not_win, "darwin", "arm64", None));
        assert!(platform_matches(&not_win, "linux", "x64", None));
        assert!(!platform_matches(&not_win, "win32", "x64", None));
    }

    #[test]
    fn native_package_name_encodes_the_target() {
        assert!(name_matches_platform(
            "@next/swc-darwin-arm64",
            "darwin",
            "arm64",
            None
        ));
        assert!(!name_matches_platform(
            "@next/swc-linux-arm64-gnu",
            "darwin",
            "arm64",
            None
        ));
        assert!(!name_matches_platform(
            "@next/swc-darwin-x64",
            "darwin",
            "arm64",
            None
        ));
        assert!(!name_matches_platform(
            "@next/swc-win32-arm64-msvc",
            "darwin",
            "arm64",
            None
        ));
        assert!(name_matches_platform("lodash", "darwin", "arm64", None));
        assert!(name_matches_platform(
            "@next/swc-linux-x64-gnu",
            "linux",
            "x64",
            Some("glibc")
        ));
        assert!(!name_matches_platform(
            "@next/swc-linux-x64-musl",
            "linux",
            "x64",
            Some("glibc")
        ));
        // This tail asserts the CURRENT-platform predicate (a linux-arm64
        // binary is skipped only off linux-arm64) — gated to the machine the
        // assertion was written for, same convention as
        // `lockfile_patch::platform_token_tests`.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let skip = skip_lockfile_materialization(
                "@next/swc-linux-arm64-gnu",
                &serde_json::json!({ "optional": true }),
            );
            assert!(skip);
        }
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;

    #[test]
    fn parses_flat_nested_and_resolutions() {
        let v = serde_json::json!({
            "overrides": {
                "ms": "2.0.0",
                "strip-ansi": { ".": "^7", "ansi-regex": "5.0.1" }
            },
            "resolutions": { "is-number": "7.0.0" }
        });
        let ov = parse_overrides(&v);
        assert_eq!(ov.get("ms").map(String::as_str), Some("2.0.0"));
        // `"."` pins the package itself …
        assert_eq!(ov.get("strip-ansi").map(String::as_str), Some("^7"));
        // … a named key pins that child (flattened, see the doc comment).
        assert_eq!(ov.get("ansi-regex").map(String::as_str), Some("5.0.1"));
        assert_eq!(ov.get("is-number").map(String::as_str), Some("7.0.0"));
    }

    #[test]
    fn ignores_non_string_override_values() {
        let v = serde_json::json!({ "overrides": { "a": 5, "b": null, "c": ["x"] } });
        assert!(parse_overrides(&v).is_empty());
    }
}

#[cfg(test)]
mod lockfile_install_path_tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// Each non-npm lockfile must be usable by the *install* path, not just by
    /// `import-lock`: that is what makes `jhol install` work in a project locked
    /// by another tool.
    #[test]
    fn install_entries_come_from_yarn_and_pnpm_and_bun() {
        // yarn.lock (v1)
        let td = tempdir().unwrap();
        write(
            td.path(),
            "yarn.lock",
            "ms@^2.1.3:\n  version \"2.1.3\"\n  integrity sha512-abc==\n",
        );
        let e = read_lockfile_install_entries_from_dir(td.path()).expect("yarn entries");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].package, "ms");
        assert_eq!(e[0].version, "2.1.3");
        assert!(e[0].resolved.contains("ms"), "resolved: {}", e[0].resolved);

        // pnpm-lock.yaml (v8 shape)
        let td = tempdir().unwrap();
        write(
            td.path(),
            "pnpm-lock.yaml",
            "lockfileVersion: '6.0'\n\npackages:\n\n  /ms@2.1.3:\n    resolution: {integrity: sha512-abc==}\n",
        );
        let e = read_lockfile_install_entries_from_dir(td.path()).expect("pnpm entries");
        assert_eq!(e[0].package, "ms");
        assert_eq!(e[0].version, "2.1.3");

        // bun.lock (JSONC, spec in the value array)
        let td = tempdir().unwrap();
        write(
            td.path(),
            "bun.lock",
            "{\n  \"lockfileVersion\": 1,\n  \"packages\": {\n    \"ms\": [\"ms@2.1.3\", \"\", {}, \"sha512-abc==\"],\n  },\n}\n",
        );
        let e = read_lockfile_install_entries_from_dir(td.path()).expect("bun entries");
        assert_eq!(e[0].package, "ms");
        assert_eq!(e[0].version, "2.1.3");
    }

    /// Strict mode must be able to *verify* a yarn/pnpm/bun install, not just
    /// refuse it. All three formats carry real SRI hashes.
    #[test]
    fn strict_integrity_is_satisfied_by_every_format_that_has_sri() {
        let with_hash = "ms@^2.1.3:\n  version \"2.1.3\"\n  integrity sha512-abc==\n";
        let td = tempdir().unwrap();
        write(td.path(), "yarn.lock", with_hash);
        assert!(
            lockfile_integrity_complete(td.path()),
            "yarn v1 carries SRI"
        );
        assert_eq!(
            read_lockfile_integrities(td.path())
                .and_then(|m| m.get("ms").cloned())
                .as_deref(),
            Some("sha512-abc==")
        );

        // A yarn block without an integrity line is reported incomplete.
        let td = tempdir().unwrap();
        write(td.path(), "yarn.lock", "ms@^2.1.3:\n  version \"2.1.3\"\n");
        assert!(!lockfile_integrity_complete(td.path()));

        let td = tempdir().unwrap();
        write(
            td.path(),
            "pnpm-lock.yaml",
            "packages:\n\n  /ms@2.1.3:\n    resolution: {integrity: sha512-abc==}\n",
        );
        assert!(lockfile_integrity_complete(td.path()), "pnpm carries SRI");

        let td = tempdir().unwrap();
        write(
            td.path(),
            "bun.lock",
            "{\"packages\": {\"ms\": [\"ms@2.1.3\", \"\", {}, \"sha512-abc==\"],}}",
        );
        assert!(lockfile_integrity_complete(td.path()), "bun carries SRI");
    }

    /// Detect reports the format that is actually present.
    #[test]
    fn detect_lockfile_names_yarn_and_pnpm() {
        let td = tempdir().unwrap();
        write(td.path(), "yarn.lock", "");
        assert_eq!(detect_lockfile(td.path()), LockfileKind::Yarn);

        let td = tempdir().unwrap();
        write(td.path(), "pnpm-lock.yaml", "");
        assert_eq!(detect_lockfile(td.path()), LockfileKind::Pnpm);
    }
}

#[cfg(test)]
mod pnpm_lock_tests {
    use super::*;

    #[test]
    fn parses_pnpm_v6_v8_leading_slash() {
        let s = "lockfileVersion: '6.0'\n\ndependencies:\n  ms:\n    specifier: ^2.1.3\n    version: 2.1.3\n\npackages:\n\n  /is-number@7.0.0:\n    resolution: {integrity: sha512-x}\n    dev: false\n\n  /ms@2.1.3:\n    resolution: {integrity: sha512-y}\n";
        let r = read_pnpm_lock_resolved_from_str(s).expect("parsed");
        assert_eq!(r.get("ms").map(String::as_str), Some("2.1.3"));
        assert_eq!(r.get("is-number").map(String::as_str), Some("7.0.0"));
    }

    #[test]
    fn parses_pnpm_v9_without_slash_plus_scoped_and_peers() {
        let s = "lockfileVersion: '9.0'\n\nimporters:\n  .:\n    dependencies:\n      ms:\n        specifier: ^2.1.3\n        version: 2.1.3\n\npackages:\n\n  '@scope/pkg@1.2.3':\n    resolution: {integrity: sha512-z}\n\n  ms@2.1.3:\n    resolution: {integrity: sha512-y}\n\n  peer-pkg@2.0.0(react@18.3.1):\n    resolution: {integrity: sha512-w}\n";
        let r = read_pnpm_lock_resolved_from_str(s).expect("parsed");
        assert_eq!(r.get("ms").map(String::as_str), Some("2.1.3"));
        assert_eq!(r.get("@scope/pkg").map(String::as_str), Some("1.2.3"));
        assert_eq!(r.get("peer-pkg").map(String::as_str), Some("2.0.0"));
    }

    #[test]
    fn skips_non_registry_entries_and_empty_input() {
        let s = "packages:\n\n  /local@file:../local:\n    resolution: {directory: ../local}\n";
        assert!(read_pnpm_lock_resolved_from_str(s).is_none());
        assert!(read_pnpm_lock_resolved_from_str("").is_none());
        assert!(read_pnpm_lock_resolved_from_str("lockfileVersion: '6.0'\n").is_none());
    }
}

#[cfg(test)]
mod yarn_lock_tests {
    use super::*;

    #[test]
    fn parses_yarn_v1() {
        let s = "# THIS IS AN AUTOGENERATED FILE.\n# yarn lockfile v1\n\n\nis-number@^7.0.0:\n  version \"7.0.0\"\n  resolved \"https://registry.yarnpkg.com/is-number/-/is-number-7.0.0.tgz\"\n\nms@^2.1.3:\n  version \"2.1.3\"\n  integrity sha512-abc==\n";
        let r = read_yarn_lock_resolved_from_str(s).expect("parsed");
        assert_eq!(r.get("ms").map(String::as_str), Some("2.1.3"));
        assert_eq!(r.get("is-number").map(String::as_str), Some("7.0.0"));
    }

    #[test]
    fn parses_yarn_berry_and_scoped_names() {
        let s = "# This file is generated by running \"yarn install\"\n\n__metadata:\n  version: 8\n  cacheKey: 10c0\n\n\"@scope/pkg@npm:^1.0.0\":\n  version: 1.2.3\n  resolution: \"@scope/pkg@npm:1.2.3\"\n\n\"ms@npm:^2.1.3\":\n  version: 2.1.3\n  resolution: \"ms@npm:2.1.3\"\n";
        let r = read_yarn_lock_resolved_from_str(s).expect("parsed");
        assert_eq!(r.get("ms").map(String::as_str), Some("2.1.3"));
        assert_eq!(r.get("@scope/pkg").map(String::as_str), Some("1.2.3"));
        // `__metadata` must not be mistaken for a package.
        assert!(!r.contains_key("__metadata"));
    }

    #[test]
    fn parses_multiple_descriptors_on_one_line() {
        let s = "ms@^2.0.0, ms@^2.1.0:\n  version \"2.1.3\"\n";
        let r = read_yarn_lock_resolved_from_str(s).expect("parsed");
        assert_eq!(r.get("ms").map(String::as_str), Some("2.1.3"));
    }

    #[test]
    fn empty_or_unrelated_input_is_none() {
        assert!(read_yarn_lock_resolved_from_str("# just a comment\n").is_none());
        assert!(read_yarn_lock_resolved_from_str("").is_none());
    }
}

#[cfg(test)]
mod jsonc_tests {
    use super::*;

    #[test]
    fn strips_trailing_commas_and_comments() {
        let bun = r#"{
  "lockfileVersion": 1,
  "packages": {
    "ms": ["ms@2.1.3", "", {}],
  },
}"#;
        let v: serde_json::Value = serde_json::from_str(&normalise_jsonc(bun)).expect("valid JSON");
        assert_eq!(v["packages"]["ms"][0], "ms@2.1.3");

        let commented = "{\"a\": 1, /* c */ // line\n \"b\": [1,2,],}";
        let v2: serde_json::Value =
            serde_json::from_str(&normalise_jsonc(commented)).expect("valid JSON");
        assert_eq!(v2["b"][1], 2);
    }

    #[test]
    fn leaves_string_contents_alone() {
        // A comma/bracket inside a string must survive, and a real URL must not be
        // mistaken for a line comment.
        let src = r#"{"url": "https://example.com/a,b}", "label": "x,}"}"#;
        let v: serde_json::Value = serde_json::from_str(&normalise_jsonc(src)).unwrap();
        assert_eq!(v["url"], "https://example.com/a,b}");
        assert_eq!(v["label"], "x,}");
    }

    #[test]
    fn reads_a_real_bun_lock_shape() {
        let bun = r#"{
  "lockfileVersion": 1,
  "packages": {
    "ms": ["ms@2.1.3", "", {}, "sha512-x"],
  },
}"#;
        let resolved = read_bun_lock_resolved_from_str(bun).expect("parsed");
        assert_eq!(resolved.get("ms").map(String::as_str), Some("2.1.3"));
    }
}

#[cfg(test)]
mod optional_dep_tests {
    use super::*;

    #[test]
    fn parses_optional_dependency_names() {
        let pj = serde_json::json!({
            "dependencies": { "ms": "^2.1.3" },
            "optionalDependencies": { "fsevents": "^2.3.3", "@img/sharp-linux-x64": "0.35.4" }
        });
        let names = parse_optional_dependency_names(&pj);
        assert!(names.contains("fsevents"));
        assert!(names.contains("@img/sharp-linux-x64"));
        // Regular dependencies are not optional.
        assert!(!names.contains("ms"));
    }

    #[test]
    fn absent_optional_dependencies_is_empty() {
        assert!(
            parse_optional_dependency_names(&serde_json::json!({ "dependencies": {} })).is_empty()
        );
        assert!(parse_optional_dependency_names(&serde_json::json!({})).is_empty());
    }
}

#[cfg(test)]
mod omit_tests {
    use super::*;
    use crate::omit::Omit;

    fn write_manifest(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let p = dir.join("package.json");
        std::fs::write(&p, body).unwrap();
        p
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("jhol-omit-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const MANIFEST: &str = r#"{
        "name": "t", "version": "1.0.0",
        "dependencies": { "prod-a": "^1.0.0", "shared": "^2.0.0" },
        "devDependencies": { "dev-b": "^3.0.0", "shared": "^9.0.0" },
        "optionalDependencies": { "opt-c": "^4.0.0", "prod-a": "^1.0.0" },
        "peerDependencies": { "peer-d": "^5.0.0" }
    }"#;

    #[test]
    fn full_install_includes_everything() {
        let d = tmp("full");
        let pj = write_manifest(&d, MANIFEST);
        let deps = deps_for_install(&pj, Omit::NONE).unwrap();
        assert!(deps.contains_key("prod-a"));
        assert!(deps.contains_key("dev-b"));
        assert!(deps.contains_key("opt-c"));
        assert!(deps.contains_key("peer-d"));
        // prod section wins for a name in both
        assert_eq!(deps.get("shared").map(|s| s.as_str()), Some("^2.0.0"));
    }

    #[test]
    fn omit_dev_drops_only_dev() {
        let d = tmp("dev");
        let pj = write_manifest(&d, MANIFEST);
        let deps = deps_for_install(&pj, Omit::PROD).unwrap();
        assert!(!deps.contains_key("dev-b"), "devDependencies must be omitted");
        assert!(deps.contains_key("prod-a"));
        assert!(deps.contains_key("opt-c"));
        assert_eq!(deps.get("shared").map(|s| s.as_str()), Some("^2.0.0"));
    }

    #[test]
    fn omit_optional_keeps_hard_deps() {
        let d = tmp("opt");
        let pj = write_manifest(&d, MANIFEST);
        let deps = deps_for_install(
            &pj,
            Omit {
                dev: false,
                optional: true,
                peer: false,
            },
        )
        .unwrap();
        assert!(!deps.contains_key("opt-c"), "optional-only dep dropped");
        assert!(deps.contains_key("prod-a"), "hard dep that is also optional survives");
    }

    #[test]
    fn omit_peer_drops_peer() {
        let d = tmp("peer");
        let pj = write_manifest(&d, MANIFEST);
        let deps = deps_for_install(
            &pj,
            Omit {
                dev: false,
                optional: false,
                peer: true,
            },
        )
        .unwrap();
        assert!(!deps.contains_key("peer-d"));
        assert!(deps.contains_key("prod-a"));
    }
}

#[cfg(test)]
mod platform_integrity_tests {
    use super::lockfile_integrity_complete;

    /// A package for another OS/CPU can never be downloaded here, so it can never
    /// carry a hash we could check. Requiring one made `jhol ci` refuse a good
    /// `vite` tree (21 `@esbuild/*` entries for other platforms).
    #[test]
    fn platform_inapplicable_entries_need_no_integrity() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package-lock.json"),
            r#"{
              "lockfileVersion": 3,
              "packages": {
                "": {},
                "node_modules/esbuild": {"version": "0.21.5", "integrity": "sha512-abc",
                                         "resolved": "https://r/esbuild.tgz"},
                "node_modules/@esbuild/aix-ppc64": {"version": "0.21.5", "optional": true,
                    "os": ["aix"], "cpu": ["ppc64"],
                    "resolved": "https://r/aix-ppc64.tgz"}
              }
            }"#,
        )
        .unwrap();
        assert!(
            lockfile_integrity_complete(tmp.path()),
            "an installable entry has its hash; the foreign one is exempt"
        );
    }

    /// But a *downloadable* entry without a hash must still fail the gate.
    #[test]
    fn installable_entry_without_integrity_still_fails() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package-lock.json"),
            r#"{"lockfileVersion": 3, "packages": {
                "": {},
                "node_modules/esbuild": {"version": "0.21.5",
                    "resolved": "https://r/esbuild.tgz"}
            }}"#,
        )
        .unwrap();
        assert!(!lockfile_integrity_complete(tmp.path()));
    }
}
