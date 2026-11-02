use chrono::Local;
use hex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Result, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use std::thread;
use std::time::Duration;

// ── Cached env var reader ────────────────────────────────────────────────────────
// Avoids redundant syscalls: each env::var is a kernel call (~0.5-1µs).
// Cache results so repeated reads of same var hit L1, not kernel.

/// Read an env var, caching the result in a OnceLock.
/// Returns None if the var is not set. Poisoned cache panics (should never happen).
pub fn cached_env_var(key: &'static str) -> Option<String> {
    let cache = ENV_CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Ok(guard) = cache.read() {
        if let Some(val) = guard.get(key) {
            return val.clone();
        }
    }
    let val = env::var(key).ok();
    if let Ok(mut guard) = cache.write() {
        guard.insert(key, val.clone());
    }
    val
}

static ENV_CACHE: OnceLock<RwLock<HashMap<&'static str, Option<String>>>> = OnceLock::new();

/// Invalidate the entire env var cache (called when env vars change at runtime).
pub fn invalidate_env_cache() {
    if let Ok(mut guard) = ENV_CACHE
        .get_or_init(|| RwLock::new(HashMap::new()))
        .write()
    {
        guard.clear();
    }
}

pub const LOG_FILE: &str = "logs.txt";
pub const NPM_SHOW_TIMEOUT_SECS: u64 = 15;
pub const NPM_INSTALL_TIMEOUT_SECS: u64 = 120;
pub const CACHE_MANIFEST_NAME: &str = "manifest.json";
pub const CACHE_MANIFEST_SIG: &str = "manifest.sig";
pub const FALLBACK_TELEMETRY_NAME: &str = "fallback_telemetry.json";

/// Returns the path to the cache directory. Uses JHOL_CACHE_DIR if set;
/// otherwise Windows: %USERPROFILE%\.jhol-cache, Unix: $HOME/.jhol-cache
pub fn get_cache_dir() -> String {
    if let Ok(dir) = env::var("JHOL_CACHE_DIR") {
        return dir;
    }
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let base = if cfg!(target_os = "windows") {
                env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string())
            } else {
                env::var("HOME").unwrap_or_else(|_| ".".to_string())
            };
            let sep = if cfg!(target_os = "windows") {
                "\\"
            } else {
                "/"
            };
            format!("{}{}.jhol-cache", base, sep)
        })
        .clone()
}

pub fn init_cache() -> Result<()> {
    static INIT: OnceLock<()> = OnceLock::new();
    if INIT.get().is_some() {
        return Ok(());
    }
    let cache_dir = get_cache_dir();
    fs::create_dir_all(&cache_dir)?;
    let log_path = PathBuf::from(format!("{}/{}", cache_dir, LOG_FILE));
    if !log_path.exists() {
        File::create(&log_path)?;
    }
    let _ = INIT.get_or_init(|| ());
    Ok(())
}

/// Process-wide silence latch (used by `jhol gui` so worker jobs cannot
/// `eprintln!` / progress onto the alternate screen and corrupt the TUI).
static OUTPUT_SILENCED: AtomicBool = AtomicBool::new(false);

/// Force quiet output for the rest of the process (or until cleared).
/// Safe to call before/while worker threads run — no `set_var`.
pub fn set_output_silenced(on: bool) {
    OUTPUT_SILENCED.store(on, Ordering::SeqCst);
}

pub fn is_quiet() -> bool {
    if OUTPUT_SILENCED.load(Ordering::Relaxed) {
        return true;
    }
    if env::var("JHOL_QUIET")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        return true;
    }
    env::var("JHOL_LOG")
        .map(|v| v.to_lowercase() == "quiet" || v.to_lowercase() == "error")
        .unwrap_or(false)
}

pub fn is_verbose() -> bool {
    if let Ok(v) = env::var("JHOL_VERBOSE") {
        return v == "1" || v == "true" || v == "yes";
    }
    false
}

pub fn clamp_prealloc(hint: usize) -> usize {
    hint.min(64 * 1024 * 1024)
}

pub fn hoisted_fallback_env(cwd: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let nm = cwd.join("node_modules");
    if nm.exists() {
        let path_str = nm.to_string_lossy().into_owned();
        if let Ok(existing) = env::var("NODE_PATH") {
            if !existing.is_empty() {
                let sep = if cfg!(windows) { ";" } else { ":" };
                map.insert(
                    "NODE_PATH".to_string(),
                    format!("{}{}{}", path_str, sep, existing),
                );
            } else {
                map.insert("NODE_PATH".to_string(), path_str);
            }
        } else {
            map.insert("NODE_PATH".to_string(), path_str);
        }
    }
    map
}

/// First non-empty value among `names`, if any is set.
fn env_override(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// The platform the install targets.
///
/// Overridable so one lockfile can be probed for another OS — `ci` on a Linux
/// box uses the same code path a cross-check from macOS does. Without this,
/// `target_*` silently reported the *host* no matter what the environment said,
/// so cross-platform installs could not be tested or selected.
pub fn target_os() -> String {
    if let Some(v) = env_override(&["JHOL_OS", "npm_config_os", "npm_config_platform"]) {
        return v;
    }
    if cfg!(target_os = "macos") {
        "darwin".to_string()
    } else if cfg!(target_os = "windows") {
        "win32".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else if cfg!(target_os = "freebsd") {
        "freebsd".to_string()
    } else if cfg!(target_os = "openbsd") {
        "openbsd".to_string()
    } else {
        std::env::consts::OS.to_string()
    }
}

pub fn target_cpu() -> String {
    if let Some(v) = env_override(&["JHOL_CPU", "npm_config_cpu", "npm_config_arch"]) {
        return v;
    }
    match std::env::consts::ARCH {
        "x86_64" => "x64".to_string(),
        "aarch64" => "arm64".to_string(),
        "x86" => "ia32".to_string(),
        "arm" => "arm".to_string(),
        other => other.to_string(),
    }
}

/// The libc the install targets (Linux only), overridable via `JHOL_LIBC`.
pub fn target_libc() -> Option<String> {
    if let Some(v) = env_override(&["JHOL_LIBC", "npm_config_libc"]) {
        return Some(v);
    }
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_env = "musl")]
        {
            Some("musl".to_string())
        }
        #[cfg(not(target_env = "musl"))]
        {
            Some("glibc".to_string())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub fn log_file_only(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_message = format!("[{}] {}", timestamp, message);
    let log_path = format!("{}/{}", get_cache_dir(), LOG_FILE);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(file, "{}", log_message);
    }
}

fn should_write_log_file() -> bool {
    if let Ok(v) = env::var("JHOL_LOG_FILE") {
        let lower = v.to_lowercase();
        return lower == "1" || lower == "true" || lower == "yes";
    }
    // Default: avoid synchronous log-file writes in quiet mode for speed.
    !is_quiet()
}

pub fn log(message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_message = format!("[{}] {}", timestamp, message);

    if !is_quiet() {
        eprintln!("{}", log_message);
    }

    if !should_write_log_file() {
        return;
    }

    let log_path = format!("{}/{}", get_cache_dir(), LOG_FILE);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(file, "{}", log_message);
    }
}

pub fn log_error(message: &str) {
    eprintln!("ERROR: {}", message);
    log(message);
}

pub fn format_cache_name(package: &str) -> String {
    package.replace('@', "-")
}

/// Convert stem like "lodash-4.17.21" or "@scope/pkg-1.0.0" to spec "lodash@4.17.21" / "@scope/pkg@1.0.0".
fn stem_to_spec(stem: &str) -> String {
    if let Some(pos) = stem.rfind('-') {
        let suffix = &stem[pos + 1..];
        if suffix.chars().any(|c| c.is_ascii_digit()) {
            return format!("{}@{}", &stem[..pos], suffix);
        }
    }
    stem.replace('-', "@")
}

fn cache_dir_path() -> PathBuf {
    PathBuf::from(get_cache_dir())
}

/// Content-addressable store dir: cache_dir/store/
/// Layout is owned by `content.rs`; this is only the ambient-cache wrapper.
fn store_dir() -> PathBuf {
    crate::content::store_dir(&cache_dir_path())
}

/// Sharded store_unpacked dir: cache_dir/store_unpacked/ab/<hash>/
/// First 2 hex chars of hash form the shard. Prevents O(n) readdir slowdown
/// with 10K+ packages. Each shard caps at ~256 entries.
pub fn store_unpacked_dir() -> PathBuf {
    crate::content::unpacked_root(&cache_dir_path())
}

/// Return sharded path for a given hash: store_unpacked/ab/<full_hash>
pub fn sharded_unpacked_path(hash: &str) -> PathBuf {
    crate::content::unpacked_dir(&cache_dir_path(), hash)
}

/// Whether to use symlinks/junctions when installing from store (JHOL_LINK=0 to disable).
pub fn use_link() -> bool {
    env::var("JHOL_LINK")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(true)
}

/// Whether an explicit `JHOL_LINK` value requests a symlink instead of the
/// default copy-on-write package clone.
pub fn use_symlink() -> bool {
    env::var("JHOL_LINK")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(false)
}

/// Materialize `node_modules/<package_name>` from the content-addressed store.
/// Defaults to a copy-on-write directory clone so Node resolves package self-imports
/// from the project path. `JHOL_LINK=1` opts into a symlink; `JHOL_LINK=0` opts into
/// the existing copy fallback.
pub fn link_package_from_store(
    store_unpacked_path: &Path,
    node_modules: &Path,
    package_name: &str,
) -> std::result::Result<(), String> {
    if !use_link() {
        return Err("JHOL_LINK=0".to_string());
    }
    let link_path = if package_name.starts_with('@') {
        if let Some(slash) = package_name.find('/') {
            let scope_dir = node_modules.join(&package_name[..slash]);
            scope_dir.join(&package_name[slash + 1..])
        } else {
            return Err(format!("invalid scoped package name: {}", package_name));
        }
    } else {
        node_modules.join(package_name)
    };

    if let Ok(meta) = fs::symlink_metadata(&link_path) {
        if meta.file_type().is_dir() {
            let _ = fs::remove_dir_all(&link_path);
        } else {
            let _ = fs::remove_file(&link_path);
        }
    }
    if let Some(parent) = link_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if use_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(store_unpacked_path, &link_path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(store_unpacked_path, &link_path)
            .map_err(|e| e.to_string())?;
    } else {
        let materialize =
            crate::jgrid::materialize::Cloner::clone_directory(store_unpacked_path, &link_path);
        if let Err(error) = materialize {
            let _ = fs::remove_dir_all(&link_path);
            return Err(error);
        }
    }
    Ok(())
}

// -- collect_store_links mtime cache ------------------------------------------
// Keyed by (node_modules canonicalized path, mtime_sec, mtime_nsec).
// APFS and ext4 both update the directory mtime on any entry add/remove,
// so a matching mtime means the instance set is identical -- safe to reuse.
struct StoreLinksCache {
    key: (PathBuf, u64, u32),
    value: StoreInstances,
}

static STORE_LINKS_CACHE: OnceLock<std::sync::Mutex<Option<StoreLinksCache>>> = OnceLock::new();

fn store_links_cache_lock() -> &'static std::sync::Mutex<Option<StoreLinksCache>> {
    STORE_LINKS_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Repair the store so runtime `require()` works for link-mode installs.
///
/// `node_modules/<pkg>` is a symlink into `store_unpacked/<shard>/<hash>/`.
/// Node resolves `require('dep')` from the *realpath* of the requiring file, so
/// a dependency looked up from inside that store dir never reaches the project's
/// `node_modules`. This creates, inside each store package, a
/// `node_modules/<dep>` symlink to the dependency's store dir — the pnpm
/// virtual-store layout.
///
/// **Chosen per dependent, by range.** The store is the authority for which
/// versions exist; the project tree is a projection of it. A dependency edge is
/// resolved against the range the *dependent* declares, never against a single
/// hoisted version of that name. Keying by name alone is wrong whenever a name
/// has more than one version — which is exactly what a nested
/// (`node_modules/<parent>/node_modules/<dep>`) copy expresses, and it is how
/// `agent-base` (declares `debug: "4"`) previously ended up linked to the
/// hoisted `debug@2.6.9`.
pub fn relink_store_dependencies(node_modules: &Path) {
    if !use_link() {
        return;
    }
    let Ok(instances) = collect_store_links(node_modules) else {
        return;
    };
    if std::env::var("JHOL_DEBUG_LAYOUT").is_ok() {
        crate::utils::log(&format!(
            "[collect] {} names, {} instances; debug -> {:?}",
            instances.len(),
            instances.values().map(|v| v.len()).sum::<usize>(),
            instances
                .get("debug")
                .map(|v| v.iter().map(|i| i.version.to_string()).collect::<Vec<_>>())
        ));
    }
    let mut source = StoreSource::load();
    let dirs: Vec<PathBuf> = instances
        .values()
        .flatten()
        .map(|i| i.dir.clone())
        .collect();
    // Fixed point: linking one package can make a deeper one resolvable, so run
    // a few passes. The store is cleared between passes because it grows as we
    // unpack on miss. **Stop as soon as a pass changes nothing** — most installs
    // converge (or are already correct) in one pass, and the extra passes are
    // pure walk cost on the warm path.
    let mut any_changed = false;
    for _ in 0..3 {
        source.scanned.clear();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut pass_changed = false;
        for dir in &dirs {
            if link_store_dir_deps(dir, &instances, &mut source, &mut seen, 0) {
                pass_changed = true;
            }
        }
        if !pass_changed {
            break;
        }
        any_changed = true;
    }
    if any_changed {
        if let Ok(mut guard) = store_links_cache_lock().lock() {
            *guard = None;
        }
    }
}

// ── the instance model ───────────────────────────────────────────────────────

/// One materialized package **instance**: (name, version) at a concrete store
/// path, plus what that version declares.
///
/// The unit a layout is made of — see `Documentation/LOCKFILE_DESIGN.md`,
/// "Instance placement: the canonical install slot/path for each resolved
/// package instance".
#[allow(dead_code)]
#[derive(Clone, Debug)]
struct StoreInstance {
    name: String,
    version: String,
    dir: PathBuf,
    deps: HashMap<String, String>,
}

/// Every instance of every installed name.
type StoreInstances = HashMap<String, Vec<StoreInstance>>;

/// Read each entry's **instance first**, then guard the descent.
///
/// The `seen` set is a cycle brake for the walk, not a filter on what counts as
/// a package: deduping first would silently drop any directory already visited
/// as a link target — which is how a nested copy vanishes from the model.
fn collect_into(
    node_modules: &Path,
    out: &mut StoreInstances,
    seen: &mut std::collections::HashSet<PathBuf>,
    depth: usize,
) -> std::result::Result<(), String> {
    if depth > 4 {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(node_modules) else {
        return if depth == 0 {
            Err("missing node_modules".to_string())
        } else {
            Ok(())
        };
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == ".bin" {
            continue;
        }
        let entry_path = entry.path();
        if name.starts_with('@') {
            if let Ok(scoped) = fs::read_dir(&entry_path) {
                for se in scoped.flatten() {
                    let full = format!("{}/{}", name, se.file_name().to_string_lossy());
                    let p = se.path();
                    if let Some(inst) = instance_at(&full, &p) {
                        push_instance(out, inst);
                        if seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())) {
                            let nm2 = p.join("node_modules");
                            if nm2.is_dir() {
                                let _ = collect_into(&nm2, out, seen, depth + 1);
                            }
                        }
                    }
                }
            }
            let _ = seen.insert(
                entry_path
                    .canonicalize()
                    .unwrap_or_else(|_| entry_path.clone()),
            );
            continue;
        }
        if let Some(inst) = instance_at(&name, &entry_path) {
            push_instance(out, inst);
        }
        if seen.insert(
            entry_path
                .canonicalize()
                .unwrap_or_else(|_| entry_path.clone()),
        ) {
            let nested = entry_path.join("node_modules");
            if nested.is_dir() {
                let _ = collect_into(&nested, out, seen, depth + 1);
            }
        }
    }
    Ok(())
}

fn push_instance(out: &mut StoreInstances, inst: StoreInstance) {
    let bucket = out.entry(inst.name.clone()).or_default();
    if bucket
        .iter()
        .any(|i| i.version == inst.version && i.dir == inst.dir)
    {
        return;
    }
    bucket.push(inst);
}

/// Resolve one `node_modules/<name>` entry into an instance, reporting *why* it
/// was rejected when `JHOL_DEBUG_LAYOUT` is set.
fn instance_at(name: &str, path: &Path) -> Option<StoreInstance> {
    let dbg = std::env::var("JHOL_DEBUG_LAYOUT").is_ok()
        && (name == "debug" || name == "agent-base" || name == "ms");
    let parent = path.parent()?;
    let Some((dir, deps)) = resolve_single_link(parent, name, path) else {
        if dbg {
            crate::utils::log(&format!(
                "[inst] REJECT {} at {} (resolve_single_link failed)",
                name,
                path.display()
            ));
        }
        return None;
    };
    let Some(version) = read_package_json_version(&dir) else {
        if dbg {
            crate::utils::log(&format!(
                "[inst] REJECT {} at {} (no version in {})",
                name,
                path.display(),
                dir.display()
            ));
        }
        return None;
    };
    if version.is_empty() {
        return None;
    }
    if dbg {
        crate::utils::log(&format!(
            "[inst] {} at {} -> {} {}",
            name,
            path.display(),
            version,
            dir.display()
        ));
    }
    Some(StoreInstance {
        name: name.to_string(),
        version,
        dir,
        deps,
    })
}

fn read_package_json_version(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// Which versions a name has in the **store**.
///
/// Tier 1: the store index. Tier 2: a scan of `store_unpacked/<shard>/<hash>`
/// (content-addressed, so each dir's `package.json` is the truth), memoized and
/// cleared between passes because the store grows during linking.
struct StoreSource {
    by_name: HashMap<String, Vec<(String, PathBuf)>>,
    scanned: std::collections::HashSet<String>,
}

impl StoreSource {
    fn load() -> Self {
        let mut by_name: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
        let cache = PathBuf::from(crate::utils::get_cache_dir());
        for (spec, hash) in read_store_index() {
            let Some(idx) = spec.rfind('@') else { continue };
            if idx == 0 || spec[idx + 1..].contains('/') {
                continue;
            }
            let (name, version) = (&spec[..idx], &spec[idx + 1..]);
            if name.is_empty() || version.is_empty() || hash.len() < 2 {
                continue;
            }
            let dir = crate::content::unpacked_dir(&cache, &hash);
            if dir.join("package.json").exists() {
                by_name
                    .entry(name.to_string())
                    .or_default()
                    .push((version.to_string(), dir));
            }
        }
        StoreSource {
            by_name,
            scanned: std::collections::HashSet::new(),
        }
    }

    fn push(&mut self, name: &str, version: &str, dir: PathBuf) {
        let bucket = self.by_name.entry(name.to_string()).or_default();
        if bucket.iter().any(|(v, d)| v == version && *d == dir) {
            return;
        }
        bucket.push((version.to_string(), dir));
    }

    /// Last resort: find every unpacked dir whose `package.json` names `name`.
    fn scan_for(&mut self, name: &str) {
        if !self.scanned.insert(name.to_string()) {
            return;
        }
        let cache = PathBuf::from(crate::utils::get_cache_dir());
        let Ok(shards) = fs::read_dir(crate::content::unpacked_root(&cache)) else {
            return;
        };
        for shard in shards.flatten() {
            let Ok(entries) = fs::read_dir(shard.path()) else {
                continue;
            };
            for e in entries.flatten() {
                let dir = e.path();
                let pj = dir.join("package.json");
                if !pj.exists() {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&pj) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if v.get("name").and_then(|n| n.as_str()) != Some(name) {
                    continue;
                }
                if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                    self.push(name, ver, dir);
                }
            }
        }
    }

    /// Highest version of `name` in the store satisfying `spec`.
    fn select(&mut self, name: &str, spec: &str) -> Option<(String, PathBuf)> {
        let mut pick = |cands: &Vec<(String, PathBuf)>| -> Option<(String, PathBuf)> {
            let mut best: Option<&(String, PathBuf)> = None;
            for c in cands {
                if !spec.is_empty() && !crate::registry::version_satisfies(spec, &c.0) {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some(b) => match (semver::Version::parse(&c.0), semver::Version::parse(&b.0)) {
                        (Ok(x), Ok(y)) => x > y,
                        _ => false,
                    },
                };
                if better {
                    best = Some(c);
                }
            }
            best.cloned()
        };
        if let Some(found) = self.by_name.get(name).and_then(&mut pick) {
            return Some(found);
        }
        self.scan_for(name);
        self.by_name.get(name).and_then(&mut pick)
    }

    fn select_any(&mut self, name: &str) -> Option<(String, PathBuf)> {
        self.select(name, "*")
    }
}

/// The instance of `name` satisfying `spec`, preferring `existing` when it
/// already does — that is what keeps a nested copy alive.
fn select_instance<'a>(
    candidates: Option<&'a Vec<StoreInstance>>,
    spec: &str,
    existing: Option<&str>,
) -> Option<&'a StoreInstance> {
    let candidates = candidates?;
    let satisfies = |v: &str| spec.is_empty() || crate::registry::version_satisfies(spec, v);
    if let Some(cur) = existing {
        if satisfies(cur) {
            if let Some(found) = candidates.iter().find(|i| i.version == cur) {
                return Some(found);
            }
        }
    }
    let mut best: Option<&StoreInstance> = None;
    for i in candidates.iter().filter(|i| satisfies(&i.version)) {
        let better = match best {
            None => true,
            Some(b) => match (
                semver::Version::parse(&i.version),
                semver::Version::parse(&b.version),
            ) {
                (Ok(x), Ok(y)) => x > y,
                _ => false,
            },
        };
        if better {
            best = Some(i);
        }
    }
    best
}

/// Symlink `nm/<name> -> dir`, creating the scope directory first.
///
/// Returns `true` when a link was created or replaced.
fn ensure_store_dep_link(nm: &Path, name: &str, dir: &Path) -> bool {
    let link = nm.join(name);
    if let Some(parent) = link.parent() {
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    if let Ok(existing) = fs::read_link(&link) {
        if existing == dir {
            return false;
        }
    }
    if fs::symlink_metadata(&link).is_ok() && fs::remove_file(&link).is_err() {
        let _ = fs::remove_dir_all(&link);
    }
    #[cfg(unix)]
    let ok = std::os::unix::fs::symlink(dir, &link).is_ok();
    #[cfg(windows)]
    let ok = std::os::windows::fs::symlink_dir(dir, &link).is_ok();
    #[cfg(not(any(unix, windows)))]
    let ok = false;
    ok
}

/// Link one store package's dependencies, **chosen per dependent by range**.
///
/// Returns `true` if anything changed (the caller invalidates the mtime cache).
fn link_store_dir_deps(
    dir: &Path,
    instances: &StoreInstances,
    source: &mut StoreSource,
    seen: &mut std::collections::HashSet<PathBuf>,
    depth: usize,
) -> bool {
    if depth > 4 {
        return false;
    }
    let Ok(real) = fs::canonicalize(dir) else {
        return false;
    };
    if !seen.insert(real) {
        return false;
    }
    let Ok(text) = fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let mut deps: HashMap<String, String> = HashMap::new();
    for key in ["dependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(o) = json.get(key).and_then(|d| d.as_object()) {
            for (k, v) in o {
                deps.entry(k.clone())
                    .or_insert_with(|| v.as_str().unwrap_or("").to_string());
            }
        }
    }
    let nm = dir.join("node_modules");
    let mut changed = false;
    // A package that requires itself by name must resolve: the store realpath is
    // not `node_modules/<name>`, so a self-link is what makes `require('<self>')`
    // work from inside the store.
    if let Some(own) = json.get("name").and_then(|v| v.as_str()) {
        if !own.is_empty() && !nm.join(own).exists() {
            changed |= ensure_store_dep_link(&nm, own, dir);
        }
    }
    for (dep_name, spec) in &deps {
        let existing = fs::canonicalize(nm.join(dep_name))
            .ok()
            .and_then(|r| read_package_json_version(&r));
        // Choose by *range*, not by name: the tree first (an existing satisfying
        // link is kept, which is how a nested copy survives), then the store,
        // which is the authority. Never leave a dependency unlinked — a missing
        // link is a hard runtime failure.
        let target = select_instance(instances.get(dep_name), spec, existing.as_deref())
            .map(|t| (t.version.clone(), t.dir.clone()))
            .or_else(|| source.select(dep_name, spec))
            .or_else(|| {
                let f = select_instance(instances.get(dep_name), "", None)
                    .map(|t| (t.version.clone(), t.dir.clone()))
                    .or_else(|| source.select_any(dep_name));
                if let Some((v, _)) = &f {
                    if std::env::var("JHOL_DEBUG_LAYOUT").is_ok() {
                        crate::utils::log(&format!(
                            "[link] fallback {}/{}: nothing satisfies {:?}, using {}",
                            nm.display(),
                            dep_name,
                            spec,
                            v
                        ));
                    }
                }
                f
            });
        let Some((target_version, target_dir)) = target else {
            continue;
        };
        if existing.as_deref() == Some(target_version.as_str()) {
            continue;
        }
        changed |= ensure_store_dep_link(&nm, dep_name, &target_dir);
    }
    // Recurse into real (non-symlink) child package dirs — a nested copy is a
    // real directory and has its own deps to link.
    if let Ok(rd) = fs::read_dir(&nm) {
        for ent in rd.flatten() {
            let child = ent.path();
            let real_child = fs::symlink_metadata(&child)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false);
            if real_child && child.join("package.json").exists() {
                changed |= link_store_dir_deps(&child, instances, source, seen, depth + 1);
            }
        }
    }
    changed
}

fn collect_store_links(node_modules: &Path) -> std::result::Result<StoreInstances, String> {
    // ── Fast path: unchanged node_modules mtime means the same instance set ──
    let nm_abs = node_modules
        .canonicalize()
        .unwrap_or_else(|_| node_modules.to_path_buf());
    let nm_mtime: Option<(u64, u32)> = fs::metadata(node_modules).ok().and_then(|m| {
        // Portable: `Metadata::modified()` is the mtime on every platform the
        // unix-only `MetadataExt::mtime()` used to be needed for — and Windows
        // has it in `std::os::windows` at a different shape. One code path.
        let t = m.modified().ok()?;
        let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some((d.as_secs(), d.subsec_nanos()))
    });
    if let Some((mtime_sec, mtime_nsec)) = nm_mtime {
        if let Ok(guard) = store_links_cache_lock().lock() {
            if let Some(cached) = guard.as_ref() {
                if cached.key == (nm_abs.clone(), mtime_sec, mtime_nsec) {
                    return Ok(cached.value.clone());
                }
            }
        }
    }
    let mut out: StoreInstances = HashMap::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    collect_into(node_modules, &mut out, &mut seen, 0)?;
    if let Some((mtime_sec, mtime_nsec)) = nm_mtime {
        if let Ok(mut guard) = store_links_cache_lock().lock() {
            *guard = Some(StoreLinksCache {
                key: (nm_abs, mtime_sec, mtime_nsec),
                value: out.clone(),
            });
        }
    }
    Ok(out)
}

/// Read one `node_modules/<name>` entry: its realpath and declared deps.
fn resolve_single_link(
    node_modules: &Path,
    name: &str,
    entry_path: &Path,
) -> Option<(PathBuf, HashMap<String, String>)> {
    // Realpath so we land in the store even if this dir is a symlink chain.
    let real = std::fs::canonicalize(entry_path).ok()?;
    let pkg_json_path = real.join("package.json");
    let content = std::fs::read_to_string(&pkg_json_path).ok()?;
    if !content.contains("dependencies")
        && !content.contains("peerDependencies")
        && !content.contains("optionalDependencies")
    {
        return Some((real, HashMap::new()));
    }
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let mut deps: HashMap<String, String> = HashMap::new();
    if let Some(deps_obj) = json.get("dependencies").and_then(|v| v.as_object()) {
        for (k, v) in deps_obj {
            deps.insert(k.clone(), v.as_str().unwrap_or("").to_string());
        }
    }
    if let Some(peers) = json.get("peerDependencies").and_then(|v| v.as_object()) {
        for (k, v) in peers {
            deps.entry(k.clone())
                .or_insert_with(|| v.as_str().unwrap_or("").to_string());
        }
    }
    if let Some(optional) = json.get("optionalDependencies").and_then(|v| v.as_object()) {
        for (k, v) in optional {
            deps.entry(k.clone())
                .or_insert_with(|| v.as_str().unwrap_or("").to_string());
        }
    }
    let _ = node_modules;
    let _ = name;
    Some((real, deps))
}

/// Index path: cache_dir/store_index.json (pkg@version -> hash)
fn store_index_path() -> PathBuf {
    cache_dir_path().join("store_index.json")
}

fn fallback_telemetry_path() -> PathBuf {
    cache_dir_path().join(FALLBACK_TELEMETRY_NAME)
}

/// Record native->backend fallback reasons for local diagnostics.
pub fn record_fallback_reason(reason: &str, packages: &[String]) {
    let path = fallback_telemetry_path();
    let mut root = read_fallback_telemetry();
    if !root.is_object() {
        root = serde_json::json!({});
    }
    let obj = root.as_object_mut().unwrap();

    let total = obj
        .get("totalFallbacks")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        + 1;
    obj.insert("totalFallbacks".to_string(), serde_json::json!(total));

    {
        let reasons = obj
            .entry("reasons")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .unwrap();
        let reason_count = reasons.get(reason).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
        reasons.insert(reason.to_string(), serde_json::json!(reason_count));
    }

    {
        let by_package = obj
            .entry("byPackage")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .unwrap();
        for pkg in packages {
            let c = by_package.get(pkg).and_then(|v| v.as_u64()).unwrap_or(0) + 1;
            by_package.insert(pkg.clone(), serde_json::json!(c));
        }
    }

    if let Ok(s) = serde_json::to_string(&root) {
        let _ = fs::write(path, s);
    }
}

/// Read fallback telemetry JSON.
pub fn read_fallback_telemetry() -> serde_json::Value {
    let path = fallback_telemetry_path();
    let Ok(s) = fs::read_to_string(path) else {
        return serde_json::json!({
            "totalFallbacks": 0,
            "reasons": {},
            "byPackage": {}
        });
    };
    serde_json::from_str(&s).unwrap_or_else(|_| {
        serde_json::json!({
            "totalFallbacks": 0,
            "reasons": {},
            "byPackage": {}
        })
    })
}

static STORE_INDEX_CACHE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

fn store_index_cache() -> &'static RwLock<HashMap<String, String>> {
    STORE_INDEX_CACHE.get_or_init(|| {
        let map = read_store_index_from_disk();
        RwLock::new(map)
    })
}

fn read_store_index_from_disk() -> HashMap<String, String> {
    let path = store_index_path();
    let s = match fs::read_to_string(&path) {
        Ok(x) => x,
        Err(_) => return HashMap::new(),
    };
    serde_json::from_str(&s).unwrap_or_default()
}

pub fn read_store_index() -> HashMap<String, String> {
    // Fast path: return cached copy (clone is cheap for HashMap<String,String>)
    if let Ok(cache) = store_index_cache().read() {
        return cache.clone();
    }
    read_store_index_from_disk()
}

/// Access the store index with a read-only closure, avoiding a full clone of the HashMap.
/// Prefer this over `read_store_index()` when you only need to query the index (get, contains_key, etc.).
pub fn with_store_index<F, R>(f: F) -> R
where
    F: FnOnce(&HashMap<String, String>) -> R,
{
    let cache = store_index_cache();
    if let Ok(guard) = cache.read() {
        f(&guard)
    } else {
        f(&read_store_index_from_disk())
    }
}

pub fn write_store_index(map: &HashMap<String, String>) -> Result<()> {
    if let Ok(mut cache) = store_index_cache().write() {
        *cache = map.clone();
    }
    let path = store_index_path();
    let s = serde_json::to_string(map).map_err(std::io::Error::other)?;
    fs::write(&path, &s)?;

    if crate::jagrs::mirror_enabled() {
        let map_clone = map.clone();
        std::thread::spawn(move || {
            for (key, hash) in map_clone.iter() {
                if let Some(at) = key.rfind('@') {
                    if at > 0 {
                        let name = &key[..at];
                        let version = &key[at + 1..];
                        if crate::jagrs::lookup_alias(key).is_none() {
                            let _ = crate::jagrs::store_alias(name, version, hash);
                        }
                    }
                }
            }
        });
    }

    Ok(())
}

/// Invalidate the store index cache (called when store directory changes externally).
pub fn invalidate_store_index_cache() {
    if let Ok(mut cache) = store_index_cache().write() {
        *cache = read_store_index_from_disk();
    }
}

pub fn manifest_signature(manifest_json: &str) -> Result<String> {
    let key = env::var("JHOL_CACHE_SIGNING_KEY")
        .map_err(|_| std::io::Error::other("JHOL_CACHE_SIGNING_KEY not set"))?;
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher.update(manifest_json.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn verify_manifest_signature(manifest_json: &str, signature: &str) -> Result<bool> {
    let expected = manifest_signature(manifest_json)?;
    Ok(expected == signature.trim())
}

pub fn parse_sri(sri: &str) -> Option<(String, Vec<u8>)> {
    let sri = sri.trim();
    let (algo, rest) = sri.split_once('-')?;
    let digest_b64 = rest.split_once('?').map(|(d, _)| d).unwrap_or(rest);
    use base64::Engine;
    let expected = base64::engine::general_purpose::STANDARD
        .decode(digest_b64.as_bytes())
        .ok()?;
    Some((algo.to_lowercase(), expected))
}

pub fn sri_sha512_of_file(path: &Path) -> std::io::Result<String> {
    use sha2::Sha512;
    let mut file = File::open(path)?;
    let mut hasher = Sha512::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(digest);
    Ok(format!("sha512-{}", b64))
}

/// SRI for bytes jhol already holds (the bundled store's decoded tarballs).
/// Lets `--lockfile-only` attest integrity without downloading: a
/// content-addressed store can prove what it owns — no network, no fabrication.
pub fn sri_sha512_of_bytes(bytes: &[u8]) -> String {
    use sha2::Sha512;
    let digest = Sha512::digest(bytes);
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(digest);
    format!("sha512-{}", b64)
}

/// Verify in-memory bytes against SRI string (e.g. "sha512-<base64>").
pub fn verify_sri_bytes(data: &[u8], sri: &str) -> bool {
    let Some((algo, expected)) = parse_sri(sri) else {
        return false;
    };

    match algo.as_str() {
        "sha512" => {
            use sha2::{Digest, Sha512};
            let got = Sha512::digest(data);
            got.as_slice() == expected.as_slice()
        }
        "sha384" => {
            use sha2::{Digest, Sha384};
            let got = Sha384::digest(data);
            got.as_slice() == expected.as_slice()
        }
        _ => false,
    }
}

/// Verify file against SRI string (e.g. "sha512-<base64>"). Returns true if match.
pub fn verify_sri(path: &Path, sri: &str) -> bool {
    let mut f = match File::open(path) {
        Ok(x) => x,
        Err(_) => return false,
    };
    let mut data = Vec::new();
    if f.read_to_end(&mut data).is_err() {
        return false;
    }
    verify_sri_bytes(&data, sri)
}

/// SHA256 hash of file (hex)
/// SHA-256 hex of a byte slice, matching `content_hash` for whole files.
pub fn content_hash_bytes(data: &[u8]) -> String {
    use sha2::Digest as _;
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    d.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn content_hash(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// SHA256 hash of lockfile content for CI cache key. Prefers bun.lock then package-lock.json.
pub fn lockfile_content_hash(dir: &Path) -> Option<String> {
    let bun = dir.join("bun.lock");
    let npm = dir.join("package-lock.json");
    let path = if bun.exists() {
        bun
    } else if npm.exists() {
        npm
    } else {
        return None;
    };
    content_hash(&path).ok()
}

/// Sanitize package name to prevent path traversal attacks
pub fn validate_package_name(package: &str) -> Option<String> {
    sanitize_package_name_impl(package)
}

/// Public alias for backward compat.
pub fn sanitize_package_name(package: &str) -> Option<String> {
    sanitize_package_name_impl(package)
}

fn sanitize_package_name_impl(package: &str) -> Option<String> {
    if package.is_empty()
        || package.starts_with('/')
        || package.starts_with('\\')
        || package.contains('\\')
        || package.contains("..")
        || package.contains("//")
    {
        return None;
    }

    let sanitized: String = package
        .chars()
        .filter(|c| {
            c.is_alphanumeric() || *c == '-' || *c == '.' || *c == '_' || *c == '@' || *c == '/'
        })
        .collect();

    if sanitized.is_empty()
        || sanitized.starts_with('/')
        || sanitized.contains("..")
        || sanitized.contains("//")
    {
        return None;
    }

    Some(sanitized)
}

/// Returns the path to a cached tarball if present.
/// 1) Content-addressable store (store_index.json + store/<hash>.tgz)
/// 2) Legacy: pkg-version.tgz in cache root
pub fn get_cached_tarball(package: &str) -> Option<PathBuf> {
    let cache_dir = cache_dir_path();
    if !cache_dir.exists() {
        return None;
    }
    let base_name = package.split('@').next().unwrap_or(package);
    let versioned_key = format_cache_name(package);

    // 1) Content-addressable store (no clone: uses with_store_index for read-only access)
    let found = with_store_index(|index| -> Option<PathBuf> {
        if package.contains('@') {
            let key = package.to_string();
            if let Some(hash) = index.get(&key) {
                let store_file = crate::content::tarball_path(&cache_dir_path(), &hash);
                if store_file.exists() {
                    return Some(store_file);
                }
            }
        } else {
            for (k, hash) in index {
                if k.starts_with(&format!("{}@", base_name)) {
                    let store_file = crate::content::tarball_path(&cache_dir_path(), &hash);
                    if store_file.exists() {
                        return Some(store_file);
                    }
                }
            }
        }
        None
    });
    if let Some(f) = found {
        return Some(f);
    }

    // 2) Legacy: exact version in cache root
    let exact = cache_dir.join(format!("{}.tgz", versioned_key));
    if exact.exists() {
        return Some(exact);
    }

    // 3) Legacy: no version - any pkg-*.tgz
    if !package.contains('@') {
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{}-", base_name))
                    && name.ends_with(".tgz")
                    && !name.contains("store")
                {
                    return Some(e.path());
                }
            }
        }
    }

    None
}

#[allow(dead_code)]
pub fn is_package_cached(package: &str) -> bool {
    get_cached_tarball(package).is_some()
}

/// Store a package in the cache (content-addressable + legacy path).
/// Prefer `registry::fill_store_from_registry` (no npm subprocess). Kept for legacy callers.
#[deprecated(
    since = "0.1.0",
    note = "Use registry::fill_store_from_registry to populate store without npm pack"
)]
pub fn cache_package_tarball(base_name: &str, version: &str) -> Result<PathBuf> {
    let cache_dir = cache_dir_path();
    fs::create_dir_all(&cache_dir)?;
    fs::create_dir_all(store_dir())?;

    let output = run_command_timeout(
        "npm",
        &["pack", &format!("{}@{}", base_name, version), "--silent"],
        NPM_SHOW_TIMEOUT_SECS,
    )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!(
            "npm pack failed: {}",
            stderr
        )));
    }

    let tgz_name = format!("{}-{}.tgz", base_name, version);
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let from = cwd.join(&tgz_name);

    if !from.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "npm pack did not create tarball",
        ));
    }

    // Content-addressable: hash and store in store/<hash>.tgz
    let hash = content_hash(&from)?;
    let store_file = crate::content::tarball_path(&cache_dir_path(), &hash);
    fs::copy(&from, &store_file)
        .map(|_| ())
        .or_else(|_| fs::rename(&from, &store_file))?;
    let _ = fs::remove_file(&from);

    let pkg_key = format!("{}@{}", base_name, version);
    let mut index = read_store_index();
    index.insert(pkg_key, hash.clone());
    write_store_index(&index)?;

    // Legacy path for backward compatibility
    let key = format!("{}-{}", base_name, version.replace('@', "-"));
    let dest = cache_dir.join(format!("{}.tgz", key));
    let _ = fs::hard_link(&store_file, &dest).or_else(|_| fs::copy(&store_file, &dest).map(|_| ()));

    log(&format!("Cached package: {}@{}", base_name, version));
    Ok(store_file)
}

/// List all cached package tarballs (pkg@version or legacy base names)
pub fn list_cached_packages() -> Result<Vec<String>> {
    let mut names: Vec<String> = with_store_index(|idx| idx.keys().cloned().collect());
    let cache_dir = cache_dir_path();
    if cache_dir.exists() {
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(".tgz") && name != "store_index.json" {
                    let base = name.trim_end_matches(".tgz");
                    if !names.contains(&base.to_string()) && !base.contains('/') {
                        names.push(base.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Remove all cached tarballs (store + legacy). Keeps logs.
pub fn cache_clean() -> Result<usize> {
    let cache_dir = cache_dir_path();
    if !cache_dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".tgz") && fs::remove_file(e.path()).is_ok() {
                removed += 1;
            }
        }
    }
    let store = store_dir();
    if store.exists() {
        if let Ok(entries) = fs::read_dir(&store) {
            for e in entries.flatten() {
                if e.path().extension().map(|x| x == "tgz").unwrap_or(false)
                    && fs::remove_file(e.path()).is_ok()
                {
                    removed += 1;
                }
            }
        }
    }
    let _ = fs::remove_file(store_index_path());
    Ok(removed)
}

/// Return (total size in bytes, count of tarballs) for the cache (store + legacy .tgz in root).
pub fn cache_size_bytes() -> Result<(u64, usize)> {
    let cache_dir = cache_dir_path();
    let mut total: u64 = 0;
    let mut count = 0;
    if cache_dir.exists() {
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().map(|x| x == "tgz").unwrap_or(false) {
                    if let Ok(m) = fs::metadata(&path) {
                        total += m.len();
                        count += 1;
                    }
                }
            }
        }
    }
    let store = store_dir();
    if store.exists() {
        if let Ok(entries) = fs::read_dir(&store) {
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().map(|x| x == "tgz").unwrap_or(false) {
                    if let Ok(m) = fs::metadata(&path) {
                        total += m.len();
                        count += 1;
                    }
                }
            }
        }
    }
    Ok((total, count))
}

/// Prune cache: remove store tarballs not referenced by index. If keep_recent is Some(N), keep only N most recently used (by mtime).
pub fn cache_prune(keep_recent: Option<usize>) -> Result<usize> {
    let mut index = read_store_index();
    let store = store_dir();
    if !store.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    let mut entries: Vec<(PathBuf, std::time::SystemTime, String)> = Vec::new();
    for e in fs::read_dir(&store)? {
        let e = e?;
        let path = e.path();
        if path.extension().map(|x| x == "tgz").unwrap_or(false) {
            let hash = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let in_index = index.values().any(|v| v == &hash);
            if !in_index {
                if fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            } else if let Ok(meta) = fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    entries.push((path, mtime, hash));
                }
            }
        }
    }
    if let Some(n) = keep_recent {
        if entries.len() > n {
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            let to_remove = entries.split_off(n);
            let keep_hashes: std::collections::HashSet<String> =
                entries.into_iter().map(|(_, _, h)| h).collect();
            index.retain(|_, hash| keep_hashes.contains(hash));
            write_store_index(&index)?;
            for (path, _, _) in to_remove {
                if fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

/// Export cache entries needed for current project (package.json + lockfile) into dir. Writes manifest.json for import. Returns number of files copied.
pub fn cache_export(dir: &Path) -> Result<usize> {
    let pj = Path::new("package.json");
    if !pj.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No package.json in current directory",
        ));
    }
    let deps = crate::lockfile::read_package_json_deps(pj).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Could not read package.json deps",
        )
    })?;
    if deps.is_empty() {
        return Ok(0);
    }
    let specs = crate::lockfile::read_all_resolved_specs_from_dir(Path::new("."))
        .unwrap_or_else(|| crate::lockfile::resolve_deps_for_install(&deps, None));
    fs::create_dir_all(dir)?;
    let mut count = 0;
    let mut manifest: Vec<(String, String)> = Vec::new();
    for spec in specs {
        if let Some(path) = get_cached_tarball(&spec) {
            let name = format!("{}.tgz", format_cache_name(&spec));
            let dest = dir.join(&name);
            fs::copy(&path, &dest)?;
            manifest.push((spec, name));
            count += 1;
        }
    }
    let manifest_json: Vec<serde_json::Value> = manifest
        .iter()
        .map(|(spec, file)| serde_json::json!({ "spec": spec, "file": file }))
        .collect();
    let manifest_path = dir.join(CACHE_MANIFEST_NAME);
    let s = serde_json::to_string(&manifest_json).map_err(std::io::Error::other)?;
    fs::write(manifest_path, s)?;
    if let Ok(sig) = manifest_signature(&serde_json::to_string(&manifest_json).unwrap_or_default())
    {
        let sig_path = dir.join(CACHE_MANIFEST_SIG);
        fs::write(sig_path, sig)?;
    }
    Ok(count)
}

/// Import tarballs from dir into cache. Expects manifest.json (from cache export) or .tgz files named pkg-version.tgz.
/// Returns number of packages imported.
pub fn cache_import(dir: &Path) -> Result<usize> {
    if !dir.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Export directory does not exist",
        ));
    }
    fs::create_dir_all(store_dir())?;
    let mut index = read_store_index();
    let mut count = 0;
    let manifest_path = dir.join(CACHE_MANIFEST_NAME);
    if manifest_path.exists() {
        let s = fs::read_to_string(&manifest_path)?;
        if let Ok(sig) = fs::read_to_string(dir.join(CACHE_MANIFEST_SIG)) {
            if !verify_manifest_signature(&s, &sig).unwrap_or(false) {
                return Err(std::io::Error::other(
                    "Manifest signature verification failed",
                ));
            }
        } else if env::var("JHOL_CACHE_SIGNING_KEY").is_ok() {
            return Err(std::io::Error::other("Manifest signature missing"));
        }
        let entries: Vec<serde_json::Value> = serde_json::from_str(&s).unwrap_or_default();
        for entry in entries {
            let spec = entry.get("spec").and_then(|v| v.as_str()).unwrap_or("");
            let file = entry.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if spec.is_empty() || file.is_empty() {
                continue;
            }
            let path = dir.join(file);
            if !path.exists() {
                continue;
            }
            let hash = content_hash(&path)?;
            let store_file = crate::content::tarball_path(&cache_dir_path(), &hash);
            if !store_file.exists() {
                fs::copy(&path, &store_file)?;
            }
            // Verify hash matches content.
            let content_hash = content_hash(&store_file)?;
            if content_hash != hash {
                return Err(std::io::Error::other(format!(
                    "Cache import hash mismatch for {}",
                    spec
                )));
            }
            index.insert(spec.to_string(), hash);
            count += 1;
        }
    } else {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            if path.extension().map(|x| x == "tgz").unwrap_or(false) {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.is_empty() {
                    continue;
                }
                let hash = content_hash(&path)?;
                let store_file = crate::content::tarball_path(&cache_dir_path(), &hash);
                if !store_file.exists() {
                    fs::copy(&path, &store_file)?;
                }
                let pkg_key = stem_to_spec(stem);
                index.insert(pkg_key, hash);
                count += 1;
            }
        }
    }
    write_store_index(&index)?;
    Ok(count)
}

/// Run a command with a timeout. On timeout the process is killed and an error is returned.
pub fn run_command_timeout(program: &str, args: &[&str], timeout_secs: u64) -> Result<Output> {
    let child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let pid = child.id();
    let kill_handle = thread::spawn(move || {
        thread::sleep(Duration::from_secs(timeout_secs));
        #[cfg(unix)]
        {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
        }
    });

    let out = child.wait_with_output();
    let _ = kill_handle.join();
    out
}

/// Run npm install with timeout
pub fn npm_install_timeout(args: &[&str], timeout_secs: u64) -> Result<Output> {
    let mut a = vec!["install"];
    a.extend(args);
    run_command_timeout("npm", &a, timeout_secs)
}

/// Persistent expansion cache: stores resolved package specs to disk,
/// keyed by a hash of the input package list. Enables O(1) repeat installs
/// by skipping expand_with_transitive_dependencies entirely when the cache hits.
///
/// Cache path: ~/.jhol-cache/expansion-cache/{hash}.txt (newline-separated specs)

fn expansion_cache_dir() -> PathBuf {
    PathBuf::from(get_cache_dir()).join("expansion-cache")
}

fn expansion_cache_path(hash: &str) -> PathBuf {
    expansion_cache_dir().join(format!("{}.txt", hash))
}

fn compute_expansion_cache_key(packages: &[&str], resolution_ctx: &str) -> String {
    let mut sorted: Vec<String> = if packages.is_empty() {
        if let Some(deps) = crate::lockfile::read_package_json_deps(Path::new("package.json")) {
            deps.into_iter()
                .map(|(k, v)| format!("{}@{}", k, v))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        packages.iter().map(|s| (*s).to_string()).collect()
    };
    sorted.sort();
    let input = format!("{}:{}", sorted.join(","), resolution_ctx);
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(&hash[..16])
}

pub fn load_expansion_cache(
    packages: &[&str],
    resolution_ctx: &str,
) -> Option<(Vec<String>, Option<String>)> {
    if packages.is_empty() && !Path::new("package.json").exists() {
        return None;
    }
    let key = compute_expansion_cache_key(packages, resolution_ctx);
    let path = expansion_cache_path(&key);
    let content = std::fs::read_to_string(&path).ok()?;
    let mut parts = content.splitn(2, "\n---VARIANTS---\n");
    let specs_str = parts.next()?;
    let variants = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let specs: Vec<String> = specs_str
        .lines()
        .filter(|l| !l.is_empty())
        .map(|s| s.to_string())
        .collect();
    if specs.is_empty() {
        None
    } else {
        Some((specs, variants))
    }
}

pub fn save_expansion_cache(
    packages: &[&str],
    specs: &[String],
    resolution_ctx: &str,
    variants_json: Option<&str>,
) -> std::result::Result<(), String> {
    if specs.is_empty() {
        return Ok(());
    }
    if packages.is_empty() && !Path::new("package.json").exists() {
        return Ok(());
    }
    let key = compute_expansion_cache_key(packages, resolution_ctx);
    let path = expansion_cache_path(&key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut content = specs.join("\n");
    if let Some(v) = variants_json {
        content.push_str("\n---VARIANTS---\n");
        content.push_str(v);
    }
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── E145: a dependency version must be chosen by RANGE, not by name ──
    // Regression: `agent-base` declares `debug: "4"` but was linked to the
    // hoisted `debug@2.6.9`, because the linker keyed by name alone. These pin
    // the rule that lets the nested `debug@4.4.3` survive.

    fn inst(name: &str, version: &str) -> StoreInstance {
        StoreInstance {
            name: name.to_string(),
            version: version.to_string(),
            dir: PathBuf::from(format!("/store/{}-{}", name, version)),
            deps: HashMap::new(),
        }
    }

    #[test]
    fn select_instance_picks_the_highest_satisfying_version() {
        let cands = vec![
            inst("debug", "2.6.9"),
            inst("debug", "4.4.3"),
            inst("debug", "4.3.7"),
        ];
        let picked = select_instance(Some(&cands), "4", None).expect("a 4.x exists");
        assert_eq!(picked.version, "4.4.3", "highest satisfying 4.x wins");
    }

    #[test]
    fn select_instance_never_returns_a_version_outside_the_range() {
        // The E145 defect exactly: nothing here satisfies `4`, so the caller
        // must fall through to the store (or leave the link alone) — it must
        // never hand back 2.6.9 for a `debug: "4"` edge.
        let cands = vec![inst("debug", "2.6.9")];
        assert!(select_instance(Some(&cands), "4", None).is_none());
    }

    #[test]
    fn select_instance_keeps_an_existing_satisfying_instance() {
        // A nested copy already linked must survive even when a different
        // satisfying version exists: re-pointing it is the bug.
        let cands = vec![inst("debug", "2.6.9"), inst("debug", "4.4.3")];
        let picked = select_instance(Some(&cands), "4", Some("4.4.3")).expect("existing satisfies");
        assert_eq!(picked.version, "4.4.3");
    }

    #[test]
    fn select_instance_replaces_an_existing_version_that_does_not_satisfy() {
        let cands = vec![inst("debug", "2.6.9"), inst("debug", "4.4.3")];
        let picked = select_instance(Some(&cands), "4", Some("2.6.9")).expect("4.x satisfies");
        assert_eq!(
            picked.version, "4.4.3",
            "2.6.9 does not satisfy 4, so 4.4.3 is chosen"
        );
    }

    #[test]
    fn select_instance_empty_spec_takes_the_highest() {
        let cands = vec![inst("debug", "2.6.9"), inst("debug", "4.4.3")];
        let picked = select_instance(Some(&cands), "", None).expect("any version");
        assert_eq!(picked.version, "4.4.3");
    }

    #[test]
    fn ensure_store_dep_link_creates_the_scope_directory() {
        // `@scope/name` links used to fail with ENOENT because the `@scope`
        // directory was never created, so a scoped dependency got no link at
        // all and the package failed to load from inside the store.
        let td = tempfile::tempdir().unwrap();
        let nm = td.path().join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        let target = td.path().join("target-1.0.0");
        std::fs::create_dir_all(&target).unwrap();
        assert!(ensure_store_dep_link(&nm, "@scope/pkg", &target));
        let link = nm.join("@scope").join("pkg");
        assert!(link.symlink_metadata().is_ok(), "scoped link exists");
        // Idempotent: a second identical call reports no change.
        assert!(!ensure_store_dep_link(&nm, "@scope/pkg", &target));
    }

    #[test]
    fn test_format_cache_name() {
        assert_eq!(format_cache_name("lodash"), "lodash");
        assert_eq!(format_cache_name("lodash@4.17.21"), "lodash-4.17.21");
        assert_eq!(format_cache_name("@scope/pkg@1.0.0"), "-scope/pkg-1.0.0");
    }

    #[test]
    fn test_get_cache_dir_non_empty() {
        let dir = get_cache_dir();
        assert!(!dir.is_empty());
        assert!(dir.contains("jhol-cache") || dir.contains(".jhol-cache"));
    }

    #[test]
    fn test_is_package_cached_no_dir() {
        // With a non-existent path in cache dir, should be false
        assert!(!is_package_cached("nonexistent-package-xyz-123"));
    }

    #[test]
    fn test_sanitize_package_name_rejects_path_traversal() {
        assert!(validate_package_name("../etc/passwd").is_none());
        assert!(validate_package_name("..\\etc\\passwd").is_none());
        assert!(validate_package_name("foo/../bar").is_none());
        assert!(validate_package_name("foo/bar/..").is_none());
    }

    #[test]
    fn test_sanitize_package_name_rejects_absolute_paths() {
        assert!(validate_package_name("/etc/passwd").is_none());
        assert!(validate_package_name("\\etc\\passwd").is_none());
    }

    #[test]
    fn test_sanitize_package_name_accepts_valid_names() {
        assert_eq!(validate_package_name("lodash"), Some("lodash".to_string()));
        assert_eq!(
            validate_package_name("@scope/package"),
            Some("@scope/package".to_string())
        );
        assert_eq!(
            validate_package_name("lodash@4.17.21"),
            Some("lodash@4.17.21".to_string())
        );
        assert_eq!(
            validate_package_name("my-package"),
            Some("my-package".to_string())
        );
        assert_eq!(
            validate_package_name("my.package"),
            Some("my.package".to_string())
        );
        assert_eq!(
            validate_package_name("my_package"),
            Some("my_package".to_string())
        );
    }

    #[test]
    fn test_sanitize_package_name_rejects_double_slashes() {
        assert!(validate_package_name("foo//bar").is_none());
    }
}
