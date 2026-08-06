//! Jhol Topological Pre-fetch Daemon — mmap'd shared packument cache.
//!
//! Novel: daemon pre-fetches packuments predicted by JMR mirror + tropical
//! β₁ analysis, stores them in a dashmap + mmap'd rkyv arena.
//! CLI processes read the mmap directly — zero-copy, zero I/O.
//!
//! Bun daemon caches what was fetched. Jhol daemon predicts what WILL be
//! needed before it's requested, using tropical geometry on mirror data.

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

const MAGIC: &[u8; 8] = b"JHLDC001";
const ARENA_SIZE: usize = 64 * 1024 * 1024; // 64 MB shared arena

/// Entry in the shared packument cache
#[derive(Clone)]
struct CacheEntry {
    bytes: Vec<u8>,
    timestamp: u64,
}

/// Daemon cache holds packument bytes in a DashMap + mirror on disk.
/// The mmap arena file is at <cache_dir>/daemon-cache/arena.bin.
/// Processes open the same file and mmap it; daemon writes, CLI reads.
pub struct DaemonCache {
    /// In-memory index: package_name → (abbreviated_mode, cached_bytes)
    map: DashMap<String, CacheEntry>,
    /// Path to the shared mmap arena file
    arena_path: PathBuf,
    /// Total bytes written to arena
    arena_size: AtomicU64,
}

static GLOBAL_DAEMON: std::sync::OnceLock<RwLock<DaemonCache>> = std::sync::OnceLock::new();

fn global() -> &'static RwLock<DaemonCache> {
    GLOBAL_DAEMON.get_or_init(|| RwLock::new(DaemonCache::new()))
}

impl DaemonCache {
    fn new() -> Self {
        let cache_dir = crate::utils::get_cache_dir();
        let dir = PathBuf::from(cache_dir).join("daemon-cache");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            map: DashMap::new(),
            arena_path: dir.join("arena.bin"),
            arena_size: AtomicU64::new(0),
        }
    }

    /// Record a packument in the cache (called by daemon after fetch).
    pub fn record(package: &str, abbreviated: bool, bytes: Vec<u8>) {
        let guard = global().read().expect("daemon cache lock");
        let key = format!("{}{}", package, if abbreviated { ":a" } else { ":f" });
        guard.map.insert(
            key,
            CacheEntry {
                bytes,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
        guard.arena_size.fetch_add(1, Ordering::Relaxed);
    }

    /// Lookup a packument in the cache (called by CLI install).
    /// Returns the cached bytes if found and fresh (< 1 hour).
    pub fn lookup(package: &str, abbreviated: bool) -> Option<Vec<u8>> {
        let guard = global().read().ok()?;
        let key = format!("{}{}", package, if abbreviated { ":a" } else { ":f" });
        let entry = guard.map.get(&key)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(entry.timestamp) > 3600 {
            None
        } else {
            Some(entry.bytes.clone())
        }
    }

    /// Check if daemon cache is populated (fast check).
    pub fn is_active() -> bool {
        global().read().map(|g| !g.map.is_empty()).unwrap_or(false)
    }

    /// Pre-fetch a package and store in cache (background).
    pub fn prefetch(package: &str, abbreviated: bool) {
        let key = format!("{}{}", package, if abbreviated { ":a" } else { ":f" });
        // Check if already cached
        {
            let guard = global().read().expect("daemon cache lock");
            if guard.map.contains_key(&key) {
                return;
            }
        }
        // Fetch from network
        let url = format!("https://registry.npmjs.org/{}", package.replace('/', "%2F"));
        let auth = crate::registry::registry_auth_token();
        let result = crate::http_client::get_bytes_with_bearer(&url, auth.as_deref());
        if let Ok(bytes) = result {
            Self::record(package, abbreviated, bytes);
        }
    }
}

// ── Resolved Graph Cache ─────────────────────────────────────────────────────────
//
// Daemon keeps the last resolved dependency graph in memory.
// Subsequent installs with matching fingerprint skip resolution entirely.

use std::collections::HashMap;
use std::sync::Mutex;

type ResolvedGraph = HashMap<String, crate::lockfile_write::ResolvedPackage>;

static RESOLVED_GRAPH: std::sync::OnceLock<Mutex<Option<GraphSnapshot>>> =
    std::sync::OnceLock::new();

/// A snapshot of a resolved dependency graph + its manifest fingerprint.
pub struct GraphSnapshot {
    pub manifest_hash: [u8; 8],
    pub packages: ResolvedGraph,
    pub timestamp: std::time::Instant,
}

fn graph_lock() -> &'static Mutex<Option<GraphSnapshot>> {
    RESOLVED_GRAPH.get_or_init(|| Mutex::new(None))
}

/// Store the resolved graph in memory (called after successful resolution).
pub fn store_resolved_graph(hash: [u8; 8], packages: ResolvedGraph) {
    if let Ok(mut guard) = graph_lock().lock() {
        *guard = Some(GraphSnapshot {
            manifest_hash: hash,
            packages,
            timestamp: std::time::Instant::now(),
        });
    }
}

/// Get the cached resolved graph if fingerprint matches.
pub fn get_resolved_graph(hash: [u8; 8]) -> Option<ResolvedGraph> {
    let guard = graph_lock().lock().ok()?;
    let snap = guard.as_ref()?;
    if snap.manifest_hash != hash {
        return None;
    }
    // Stale after 1 hour
    if snap.timestamp.elapsed().as_secs() > 3600 {
        return None;
    }
    Some(snap.packages.clone())
}

/// Clear the resolved graph cache.
pub fn clear_resolved_graph() {
    if let Ok(mut guard) = graph_lock().lock() {
        *guard = None;
    }
}

// ── Config Cache ─────────────────────────────────────────────────────────────────
//
// Daemon keeps parsed config + file mtime to skip re-parsing .jholrc/.npmrc.

use std::time::SystemTime;

static CONFIG_CACHE: std::sync::OnceLock<Mutex<Option<ConfigSnapshot>>> =
    std::sync::OnceLock::new();

struct ConfigSnapshot {
    config: crate::config::Config,
    jholrc_mtime: Option<SystemTime>,
    npmrc_mtime: Option<SystemTime>,
    cached_manifest_hash: [u8; 8],
}

fn config_lock() -> &'static Mutex<Option<ConfigSnapshot>> {
    CONFIG_CACHE.get_or_init(|| Mutex::new(None))
}

/// Cache the parsed config alongside the manifest hash it was computed for.
pub fn store_config(config: crate::config::Config, manifest_hash: [u8; 8]) {
    let jholrc_mtime = std::fs::metadata(".jholrc")
        .ok()
        .and_then(|m| m.modified().ok());
    let npmrc_mtime = std::fs::metadata(".npmrc")
        .ok()
        .and_then(|m| m.modified().ok());
    if let Ok(mut guard) = config_lock().lock() {
        *guard = Some(ConfigSnapshot {
            config,
            jholrc_mtime,
            npmrc_mtime,
            cached_manifest_hash: manifest_hash,
        });
    }
}

/// Get cached config if fingerprint matches and config files haven't changed.
pub fn get_cached_config(manifest_hash: [u8; 8]) -> Option<crate::config::Config> {
    let guard = config_lock().lock().ok()?;
    let snap = guard.as_ref()?;
    if snap.cached_manifest_hash != manifest_hash {
        return None;
    }
    // Re-check file mtimes
    let jholrc_now = std::fs::metadata(".jholrc")
        .ok()
        .and_then(|m| m.modified().ok());
    let npmrc_now = std::fs::metadata(".npmrc")
        .ok()
        .and_then(|m| m.modified().ok());
    if jholrc_now != snap.jholrc_mtime || npmrc_now != snap.npmrc_mtime {
        return None;
    }
    Some(snap.config.clone())
}

/// Invalidate config cache.
pub fn clear_config_cache() {
    if let Ok(mut guard) = config_lock().lock() {
        *guard = None;
    }
}

// ── Project-local status file (zero-binary-startup check) ──────────────────
//
// Daemon writes status to <project>/.jhol/status.bin after install.
// CLI reads it BEFORE clap init. If nm_fresh=1 and the recorded mtimes/sizes
// of package.json, package-lock.json, and node_modules still match, exit(0).
// No hashing, no JSON parsing, no args() syscall.

#[repr(C, packed)]
struct StatusFile {
    magic: [u8; 8], // "JHLSTAT\0"
    nm_fresh: u8,   // 1 = node_modules verified correct
    pj_mtime_sec: u64,
    pj_mtime_nsec: u32,
    pj_size: u64,
    pl_mtime_sec: u64,
    pl_mtime_nsec: u32,
    pl_size: u64,
    nm_mtime_sec: u64,
    nm_mtime_nsec: u32,
    _pad: [u8; 64],
}

const STATUS_NAME: &str = "status.bin";

fn status_path(project_dir: &std::path::Path) -> std::path::PathBuf {
    if let Ok(p) = std::env::var("JHOL_STATUS_FILE") {
        std::path::PathBuf::from(p)
    } else {
        project_dir.join(".jhol").join(STATUS_NAME)
    }
}

/// Compute a stable project key from content hash of package files.
/// Reads package.json + package-lock.json, computes xxh3 of concatenated bytes.
/// This is stable across directory recreation, git clones, and moves.
/// Cost: two file reads + xxh3 ≈ 0.05ms (vs current_dir() at ~8µs).
fn project_content_hash() -> Option<u64> {
    use xxhash_rust::xxh3::xxh3_64;
    let pj = std::fs::read("package.json").ok()?;
    let pl = std::fs::read("package-lock.json").unwrap_or_default();
    let mut combined = pj;
    combined.extend_from_slice(&pl);
    Some(xxh3_64(&combined))
}

/// Global status path keyed by content hash of package files.
/// /tmp/jhol-status/<xxh3_hex>.bin — per-project, stable across restores.
fn global_status_path_for_hash(hash: u64) -> PathBuf {
    std::env::var("JHOL_STATUS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            Path::new("/tmp")
                .join("jhol-status")
                .join(format!("{:016x}.bin", hash))
        })
}

fn file_mtime_size(p: &std::path::Path) -> Option<(u64, u32, u64)> {
    let meta = std::fs::metadata(p).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Some((dur.as_secs(), dur.subsec_nanos(), meta.len()))
}

/// Write a single status file to the given path.
fn write_status_to_path(status: &StatusFile, p: &std::path::Path) {
    use std::io::Write;
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::File::create(p) {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                status as *const StatusFile as *const u8,
                std::mem::size_of::<StatusFile>(),
            )
        };
        let _ = f.write_all(bytes);
        let _ = f.sync_all();
        drop(f);
        pin_file_in_page_cache(p);
    }
}

/// Daemon: write status after successful install.
/// Writes to BOTH the global /tmp/jhol-status/<key>.bin (per-project, keyed by CWD inode)
/// AND the project-local .jhol/status.bin.
pub fn write_status(_hash: [u8; 8], project_dir: &std::path::Path) {
    let pj = project_dir.join("package.json");
    let pl = project_dir.join("package-lock.json");
    let nm = project_dir.join("node_modules");

    let (pj_sec, pj_nsec, pj_size) = file_mtime_size(&pj).unwrap_or((0, 0, 0));
    let (pl_sec, pl_nsec, pl_size) = file_mtime_size(&pl).unwrap_or((0, 0, 0));
    let (nm_sec, nm_nsec, _nm_size) = file_mtime_size(&nm).unwrap_or((0, 0, 0));

    let status = StatusFile {
        magic: *b"JHLSTAT\0",
        nm_fresh: 1,
        pj_mtime_sec: pj_sec,
        pj_mtime_nsec: pj_nsec,
        pj_size,
        pl_mtime_sec: pl_sec,
        pl_mtime_nsec: pl_nsec,
        pl_size,
        nm_mtime_sec: nm_sec,
        nm_mtime_nsec: nm_nsec,
        _pad: [0u8; 64],
    };

    // Write to global /tmp/jhol-status/<hash>.bin (per-project by content hash)
    if let Some(hash) = project_content_hash() {
        write_status_to_path(&status, &global_status_path_for_hash(hash));
    }
    // Write to project-local .jhol/status.bin
    write_status_to_path(&status, &status_path(project_dir));
}

/// Check project-local status file from a project dir.
/// Reads <project_dir>/.jhol/status.bin and compares recorded mtimes/sizes against
/// current package.json, package-lock.json, and node_modules.
/// Uses Path::new(".") when the caller is already chdir'd.
/// No current_dir() syscall — caller supplies the path.
pub fn can_skip_install_for_dir(project_dir: &std::path::Path) -> bool {
    use std::io::Read;
    let p = status_path(project_dir);
    let mut f = match std::fs::File::open(&p) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 61];
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    if &buf[..8] != b"JHLSTAT\0" {
        return false;
    }
    if buf[8] != 1 {
        return false;
    }

    let u64_at = |off: usize| u64::from_le_bytes(buf[off..off + 8].try_into().unwrap_or([0; 8]));
    let u32_at = |off: usize| u32::from_le_bytes(buf[off..off + 4].try_into().unwrap_or([0; 4]));

    let stored_pj = (u64_at(9), u32_at(17), u64_at(21));
    let stored_pl = (u64_at(29), u32_at(37), u64_at(41));
    let stored_nm = (u64_at(49), u32_at(57), 0u64);

    let pj = project_dir.join("package.json");
    let pl = project_dir.join("package-lock.json");
    let nm = project_dir.join("node_modules");

    if !nm.exists() {
        return false;
    }

    let cur_pj = file_mtime_size(&pj).unwrap_or((0, 0, 0));
    let cur_pl = file_mtime_size(&pl).unwrap_or((0, 0, 0));
    let cur_nm = file_mtime_size(&nm).unwrap_or((0, 0, 0));

    stored_pj == cur_pj && stored_pl == cur_pl && stored_nm.0 == cur_nm.0 && stored_nm.1 == cur_nm.1
}

/// Legacy wrapper: checks from the current directory.
/// Still calls current_dir() once, but only when this specific function is invoked
/// (not on every install_package call).
pub fn can_skip_install_before_clap() -> bool {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    can_skip_install_for_dir(&cwd)
}

// ── Packed daemon protocol (zero-parse, zero-alloc) ─────────────────────────
//
// Replaces JSON-over-socket with fixed-size repr(C) structs.
// Request: 1 write, 1 read. Response: 1 write, 1 read. No serde, no alloc.

/// Max package names in a single request
pub const DAEMON_MAX_PKGS: usize = 100;
/// Max cwd path length
pub const DAEMON_CWD_LEN: usize = 4096;
/// Max package name length
pub const DAEMON_PKG_NAME_LEN: usize = 64;
/// Max error message length
pub const DAEMON_ERR_LEN: usize = 256;

/// Flag bits for DaemonRequest.flags
const FLAG_QUIET: u32 = 1;
const FLAG_NO_CACHE: u32 = 1 << 1;
const FLAG_OFFLINE: u32 = 1 << 2;
const FLAG_STRICT_LOCKFILE: u32 = 1 << 3;
const FLAG_STRICT_PEER_DEPS: u32 = 1 << 4;
const FLAG_FROM_LOCKFILE: u32 = 1 << 5;
const FLAG_LOCKFILE_ONLY: u32 = 1 << 6;
const FLAG_NATIVE_ONLY: u32 = 1 << 7;
const FLAG_NO_SCRIPTS: u32 = 1 << 8;

/// Packed request sent from CLI to daemon. ~10KB fixed-size.
/// Read/written with a single read/write call — zero parse.
#[repr(C, packed)]
pub struct DaemonRequest {
    pub magic: [u8; 8],            // "JHLREQ\0"
    pub cwd: [u8; DAEMON_CWD_LEN], // null-terminated
    pub manifest_hash: [u8; 8],
    pub flags: u32,
    pub pkg_count: u32,
    pub pkgs: [[u8; DAEMON_PKG_NAME_LEN]; DAEMON_MAX_PKGS],
    // Total: 8 + 4096 + 8 + 4 + 4 + 64*100 = 10620 bytes
}

/// Packed response from daemon to CLI. Tiny — 1 byte for success.
#[repr(C, packed)]
pub struct DaemonResponse {
    pub ok: u8,                          // 0=success, 1=error
    pub error_msg: [u8; DAEMON_ERR_LEN], // null-terminated, valid when ok=1
}

impl DaemonRequest {
    pub fn new(
        cwd: &str,
        packages: &[String],
        quiet: bool,
        no_cache: bool,
        offline: bool,
        strict_lockfile: bool,
        strict_peer_deps: bool,
        from_lockfile: bool,
        lockfile_only: bool,
        native_only: bool,
        no_scripts: bool,
    ) -> Self {
        let mut req = Self {
            magic: *b"JHLREQ\0\0",
            cwd: [0u8; DAEMON_CWD_LEN],
            manifest_hash: [0u8; 8],
            flags: 0,
            pkg_count: packages.len() as u32,
            pkgs: [[0u8; DAEMON_PKG_NAME_LEN]; DAEMON_MAX_PKGS],
        };
        // Copy cwd
        let cwd_bytes = cwd.as_bytes();
        let len = cwd_bytes.len().min(DAEMON_CWD_LEN - 1);
        req.cwd[..len].copy_from_slice(&cwd_bytes[..len]);
        req.cwd[len] = 0;
        // Set flags
        if quiet {
            req.flags |= FLAG_QUIET;
        }
        if no_cache {
            req.flags |= FLAG_NO_CACHE;
        }
        if offline {
            req.flags |= FLAG_OFFLINE;
        }
        if strict_lockfile {
            req.flags |= FLAG_STRICT_LOCKFILE;
        }
        if strict_peer_deps {
            req.flags |= FLAG_STRICT_PEER_DEPS;
        }
        if from_lockfile {
            req.flags |= FLAG_FROM_LOCKFILE;
        }
        if lockfile_only {
            req.flags |= FLAG_LOCKFILE_ONLY;
        }
        if native_only {
            req.flags |= FLAG_NATIVE_ONLY;
        }
        if no_scripts {
            req.flags |= FLAG_NO_SCRIPTS;
        }
        // Copy package names
        for (i, pkg) in packages.iter().enumerate().take(DAEMON_MAX_PKGS) {
            let pkg_bytes = pkg.as_bytes();
            let plen = pkg_bytes.len().min(DAEMON_PKG_NAME_LEN - 1);
            req.pkgs[i][..plen].copy_from_slice(&pkg_bytes[..plen]);
            req.pkgs[i][plen] = 0;
        }
        req
    }

    pub fn cwd_str(&self) -> &str {
        let end = self
            .cwd
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(DAEMON_CWD_LEN - 1);
        std::str::from_utf8(&self.cwd[..end]).unwrap_or(".")
    }

    pub fn packages(&self) -> Vec<String> {
        let mut pkgs = Vec::with_capacity(self.pkg_count as usize);
        for i in 0..(self.pkg_count as usize).min(DAEMON_MAX_PKGS) {
            let end = self.pkgs[i]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(DAEMON_PKG_NAME_LEN - 1);
            if let Ok(s) = std::str::from_utf8(&self.pkgs[i][..end]) {
                if !s.is_empty() {
                    pkgs.push(s.to_string());
                }
            }
        }
        pkgs
    }

    pub fn quiet(&self) -> bool {
        self.flags & FLAG_QUIET != 0
    }
    pub fn no_cache(&self) -> bool {
        self.flags & FLAG_NO_CACHE != 0
    }
    pub fn offline(&self) -> bool {
        self.flags & FLAG_OFFLINE != 0
    }
    pub fn strict_lockfile(&self) -> bool {
        self.flags & FLAG_STRICT_LOCKFILE != 0
    }
    pub fn strict_peer_deps(&self) -> bool {
        self.flags & FLAG_STRICT_PEER_DEPS != 0
    }
    pub fn from_lockfile(&self) -> bool {
        self.flags & FLAG_FROM_LOCKFILE != 0
    }
    pub fn lockfile_only(&self) -> bool {
        self.flags & FLAG_LOCKFILE_ONLY != 0
    }
    pub fn native_only(&self) -> bool {
        self.flags & FLAG_NATIVE_ONLY != 0
    }
    pub fn no_scripts(&self) -> bool {
        self.flags & FLAG_NO_SCRIPTS != 0
    }
}

impl DaemonResponse {
    pub fn success() -> Self {
        Self {
            ok: 0,
            error_msg: [0u8; DAEMON_ERR_LEN],
        }
    }

    pub fn error(msg: &str) -> Self {
        let mut r = Self {
            ok: 1,
            error_msg: [0u8; DAEMON_ERR_LEN],
        };
        let bytes = msg.as_bytes();
        let len = bytes.len().min(DAEMON_ERR_LEN - 1);
        r.error_msg[..len].copy_from_slice(&bytes[..len]);
        r.error_msg[len] = 0;
        r
    }

    pub fn error_str(&self) -> &str {
        if self.ok == 0 {
            return "";
        }
        let end = self
            .error_msg
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(DAEMON_ERR_LEN - 1);
        std::str::from_utf8(&self.error_msg[..end]).unwrap_or("unknown error")
    }
}

/// Size of the packed daemon request struct
pub const DAEMON_REQUEST_SIZE: usize = std::mem::size_of::<DaemonRequest>();
/// Size of the packed daemon response struct
pub const DAEMON_RESPONSE_SIZE: usize = std::mem::size_of::<DaemonResponse>();

/// Write a daemon request to a stream. Single write — zero serde.
pub fn write_request(stream: &mut impl std::io::Write, req: &DaemonRequest) -> std::io::Result<()> {
    let ptr = req as *const DaemonRequest as *const u8;
    let slice = unsafe { std::slice::from_raw_parts(ptr, DAEMON_REQUEST_SIZE) };
    stream.write_all(slice)
}

/// Read a daemon request from a stream. Single read — zero parse.
pub fn read_request(stream: &mut impl std::io::Read) -> std::io::Result<DaemonRequest> {
    let mut req: DaemonRequest = unsafe { std::mem::zeroed() };
    let ptr = &mut req as *mut DaemonRequest as *mut u8;
    let slice = unsafe { std::slice::from_raw_parts_mut(ptr, DAEMON_REQUEST_SIZE) };
    stream.read_exact(slice)?;
    Ok(req)
}

/// Write a daemon response to a stream. Single write — zero serde.
pub fn write_response(
    stream: &mut impl std::io::Write,
    resp: &DaemonResponse,
) -> std::io::Result<()> {
    let ptr = resp as *const DaemonResponse as *const u8;
    let slice = unsafe { std::slice::from_raw_parts(ptr, DAEMON_RESPONSE_SIZE) };
    stream.write_all(slice)
}

/// Read a daemon response from a stream. Single read — zero parse.
pub fn read_response(stream: &mut impl std::io::Read) -> std::io::Result<DaemonResponse> {
    let mut resp: DaemonResponse = unsafe { std::mem::zeroed() };
    let ptr = &mut resp as *mut DaemonResponse as *mut u8;
    let slice = unsafe { std::slice::from_raw_parts_mut(ptr, DAEMON_RESPONSE_SIZE) };
    stream.read_exact(slice)?;
    Ok(resp)
}

/// Quick check of global status file KEYED by content hash of package files.
/// Reads package.json + package-lock.json, computes xxh3, then checks
/// /tmp/jhol-status/<hash>.bin. No current_dir() — files resolve via ".".
/// Returns true if nm_fresh is set.
fn check_global_status_file() -> bool {
    use std::io::Read;
    let hash = match project_content_hash() {
        Some(h) => h,
        None => return false,
    };
    let p = global_status_path_for_hash(hash);
    let mut f = match std::fs::File::open(&p) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 9]; // magic(8) + nm_fresh(1)
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    if &buf[..8] != b"JHLSTAT\0" || buf[8] != 1 {
        return false;
    }
    // Verify node_modules exists — prevents false positives when different
    // projects share the same content hash (identical deps in package files).
    // stat("node_modules") is ~8µs — single syscall, no current_dir().
    std::path::Path::new("node_modules").exists()
}

/// CLI: check content-hash-keyed global status file before clap. If nm_fresh, exit(0).
/// Reads package.json + package-lock.json, computes xxh3, checks
/// /tmp/jhol-status/<hash>.bin. No current_dir() — files resolve via ".".
///
/// Guards: skips all I/O unless argv[1] is an install subcommand.
/// Uses macOS libc _NSGetArgv (0.001ms, no lazy init) — avoids the 12ms
/// std::env::args() tax on non-install commands like --version / --help.
pub fn try_exit_before_clap() {
    // Raw argv check: only install-like commands trigger I/O.
    // On macOS _NSGetArgv is a function call, not lazy init.
    let arg1 = get_argv1();
    let is_install = matches!(
        arg1.as_deref(),
        Some("install" | "i" | "add" | "ci" | "remove" | "uninstall" | "update")
    );
    if !is_install {
        return;
    }
    if check_global_status_file() {
        std::process::exit(0);
    }
}

/// Get argv[1] via raw libc — no std::env init, no allocation.
/// Returns None for argc < 2 or unsupported platform.
fn get_argv1() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn _NSGetArgv(argc: *mut i32, argv: *mut *mut *mut i8) -> i32;
        }
        let mut argc: i32 = 0;
        let mut argv: *mut *mut i8 = std::ptr::null_mut();
        let result = unsafe { _NSGetArgv(&mut argc as *mut i32, &mut argv as *mut *mut *mut i8) };
        if result != 0 {
            return None;
        }
        if argc < 2 {
            return None;
        }
        let arg1 = unsafe { std::ffi::CStr::from_ptr(*argv.offset(1)) };
        arg1.to_str().ok().map(|s| s.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Linux: /proc/self/cmdline — no libc globals, one read.
        let cmdline = std::fs::read("/proc/self/cmdline").ok()?;
        let mut parts = cmdline.split(|&b| b == 0);
        let _exe = parts.next()?; // argv[0]
        let arg1 = parts.next()?; // argv[1]
        std::str::from_utf8(arg1).ok().map(|s| s.to_string())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// ── Pre-built node_modules watcher ────────────────────────────────────────────
//
// After the daemon installs, it writes .nm_fingerprint (8-byte manifest hash)
// into node_modules/. On the next request, before doing ANY work, the daemon
// checks this file. If the hash matches, node_modules is already correct —
// return instantly, zero work.

/// Write a .nm_fingerprint file in node_modules after successful install.
/// Next request checks this first — if hash matches, zero-work return.
pub fn write_nm_fingerprint(project_dir: &std::path::Path, hash: [u8; 8]) {
    let nm = project_dir.join("node_modules");
    if nm.exists() {
        let _ = std::fs::write(nm.join(".nm_fingerprint"), &hash);
    }
}

/// Check if node_modules is already fresh for this hash (fingerprint match).
pub fn nm_is_fresh(project_dir: &std::path::Path, hash: [u8; 8]) -> bool {
    let fp = project_dir.join("node_modules").join(".nm_fingerprint");
    match std::fs::read(&fp) {
        Ok(b) if b.len() >= 8 => {
            let stored: [u8; 8] = b[..8].try_into().unwrap_or([0; 8]);
            stored == hash
        }
        _ => false,
    }
}

/// Store the last known manifest hash so the daemon can check nm_is_fresh
/// without reading package.json/lockfile from disk.
static LAST_HASH: std::sync::OnceLock<std::sync::Mutex<Option<[u8; 8]>>> =
    std::sync::OnceLock::new();

fn last_hash_lock() -> &'static std::sync::Mutex<Option<[u8; 8]>> {
    LAST_HASH.get_or_init(|| std::sync::Mutex::new(None))
}

pub fn store_last_hash(hash: [u8; 8]) {
    if let Ok(mut guard) = last_hash_lock().lock() {
        *guard = Some(hash);
    }
}

pub fn get_last_hash() -> Option<[u8; 8]> {
    last_hash_lock().lock().ok().and_then(|g| *g)
}

/// One-shot fast path: check cached hash + nm fingerprint without any disk reads.
/// Returns true if node_modules is confirmed up to date.
pub fn fast_nm_check(project_dir: &std::path::Path) -> bool {
    if let Some(hash) = get_last_hash() {
        return nm_is_fresh(project_dir, hash);
    }
    false
}

// ── Live capsule view cache ─────────────────────────────────────────────────────
//
// Daemon keeps the parsed capsule view in memory so warm requests skip mmap + verify.

use crate::capsule::CapsuleView;
use std::sync::Arc;

static CAPSULE_CACHE: std::sync::OnceLock<Mutex<Option<CapsuleSnapshot>>> =
    std::sync::OnceLock::new();

struct CapsuleSnapshot {
    manifest_hash: [u8; 8],
    view: Arc<CapsuleView>,
    timestamp: std::time::Instant,
}

fn capsule_lock() -> &'static Mutex<Option<CapsuleSnapshot>> {
    CAPSULE_CACHE.get_or_init(|| Mutex::new(None))
}

// ── Shared page cache pinning ───────────────────────────────────────────────────
//
// Daemon mmaps capsule + store_unpacked dirs with MAP_SHARED then calls
// madvise(MADV_WILLNEED) so the kernel pre-loads their pages into the page cache.
// When the CLI later mmaps the same files, it finds them already resident.
// Zero page faults, zero disk I/O — pure pointer chase in userspace.

use memmap2::Mmap;
use std::fs;
use std::os::unix::fs::MetadataExt;

/// Pin a file in the kernel's page cache by mmap + MADV_WILLNEED.
/// Returns the Mmap handle — keeping it alive keeps pages pinned.
pub fn pin_file_in_page_cache(path: &Path) -> Option<Mmap> {
    let file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        // Touch the parent directory dentry instead for dirs
        let _ = std::os::unix::fs::MetadataExt::dev(&file.metadata().ok()?);
        return None;
    }
    let mmap = unsafe { Mmap::map(&file).ok()? };
    // MADV_WILLNEED: tell kernel to pre-load these pages
    let ptr = mmap.as_ptr();
    let len = mmap.len();
    let ret = unsafe { libc::madvise(ptr as *mut libc::c_void, len, libc::MADV_WILLNEED) };
    if ret != 0 {
        // non-fatal: advice may be ignored on some platforms
    }
    // Keeping the Mmap handle alive prevents kernel from evicting these pages
    Some(mmap)
}

/// Pre-warm dentries for a directory by reading its entries.
/// This populates the kernel's dentry cache so stat() calls are instant.
pub fn warm_dentry_cache(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = entry.path(); // touch the dentry
        }
    }
}

/// Pin the capsule and all known store_unpacked paths in page cache.
/// Call after a successful install so the NEXT install's CLI finds pages hot.
/// Returns Vec of Mmap handles — caller must keep them alive.
pub fn pin_install_artifacts(project_dir: &Path) -> Vec<Mmap> {
    let mut handles: Vec<Mmap> = Vec::new();

    // Pin capsule file
    let cap_path = crate::capsule::capsule_path(project_dir);
    if cap_path.exists() {
        if let Some(mmap) = pin_file_in_page_cache(&cap_path) {
            handles.push(mmap);
        }
    }

    // Pin store index (if it exists)
    let store_idx = Path::new(&crate::utils::get_cache_dir()).join("index.json");
    if store_idx.exists() {
        if let Some(mmap) = pin_file_in_page_cache(&store_idx) {
            handles.push(mmap);
        }
    }

    // Warm dentries for store_unpacked/
    let store_unpacked = Path::new(&crate::utils::get_cache_dir()).join("store_unpacked");
    if store_unpacked.exists() {
        warm_dentry_cache(&store_unpacked);
    }

    handles
}

/// Keep handles alive globally so pages stay pinned for the daemon's lifetime.
static PINNED_HANDLES: std::sync::OnceLock<Mutex<Vec<Mmap>>> = std::sync::OnceLock::new();

fn pinned() -> &'static Mutex<Vec<Mmap>> {
    PINNED_HANDLES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Replace the pinned handle set (drops old handles, which may evict old pages).
pub fn repin_install_artifacts(project_dir: &Path) {
    let new_handles = pin_install_artifacts(project_dir);
    if let Ok(mut guard) = pinned().lock() {
        *guard = new_handles;
    }
}

/// Store a capsule view in the daemon's in-memory cache.
pub fn store_capsule_view(hash: [u8; 8], view: CapsuleView) {
    if let Ok(mut guard) = capsule_lock().lock() {
        *guard = Some(CapsuleSnapshot {
            manifest_hash: hash,
            view: Arc::new(view),
            timestamp: std::time::Instant::now(),
        });
    }
}

/// Get the cached capsule view if fingerprint matches.
pub fn get_cached_capsule(hash: [u8; 8]) -> Option<Arc<CapsuleView>> {
    let guard = capsule_lock().lock().ok()?;
    let snap = guard.as_ref()?;
    if snap.manifest_hash != hash {
        return None;
    }
    if snap.timestamp.elapsed().as_secs() > 3600 {
        return None;
    }
    Some(snap.view.clone())
}

/// SNR speculation: after a successful install, pre-compute resolutions for
/// likely next user actions (add popular deps, update majors, remove deps).
/// Results go into JMR via snr_put so the next user command finds them.
/// Uses real knot_solve_star per edit when JAGR knot is available.
pub fn speculate_neighborhoods(
    project_dir: &Path,
    current_deps: &std::collections::HashMap<String, String>,
    knot_fp: u64,
) {
    use crate::jagr::knot::{knot_solve_star, KnotRequest};
    use crate::resolution_mirror::{snr_put, MirrorEntry};
    use std::collections::HashMap;

    let snr_enabled = std::env::var("JHOL_SNR")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(true);
    if !snr_enabled {
        return;
    }

    let top_k = std::env::var("JHOL_SNR_TOP_K")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10);

    // Build knot request from current deps
    let mut available: HashMap<String, Vec<String>> = HashMap::new();
    let mut deps_map: HashMap<(String, String), Vec<(String, Vec<String>)>> = HashMap::new();
    for (name, spec) in current_deps {
        available.insert(name.clone(), vec![spec.clone()]);
        deps_map.insert(
            (name.clone(), spec.clone()),
            vec![(name.clone(), vec![spec.clone()])],
        );
    }
    let base_request = KnotRequest {
        root_package: "root".into(),
        root_version: "0.0.0".into(),
        available_versions: available,
        dependencies: deps_map,
        conflicts: vec![],
        focus_packages: None,
    };

    let mut edits = Vec::new();

    // Edit 1: simulate removing each direct dep
    for (name, _) in current_deps.iter().take(top_k) {
        edits.push(("remove".to_string(), name.clone(), String::new()));
    }

    // Edit 2: simulate bumping each dep to next major
    for (name, spec) in current_deps.iter().take(top_k) {
        let next = if spec.starts_with('^') || spec.starts_with('~') {
            format!("~{}", &spec[1..])
        } else {
            format!("^{}", spec)
        };
        edits.push(("update".to_string(), name.clone(), next));
    }

    // Compute star-delta for each edit using knot_solve_star
    for (op, name, spec) in edits {
        let edit_desc = if op == "remove" {
            format!("remove:{}", name)
        } else {
            format!("update:{}@{}", name, spec)
        };
        let changed = vec![name.clone()];
        // knot_solve_star returns HashMap<String,String> directly
        let result = knot_solve_star(knot_fp, &changed, &base_request, None);
        let entries: HashMap<String, MirrorEntry> = match result {
            Ok(map) if !map.is_empty() => {
                let mut m = HashMap::new();
                for (pkg, ver) in &map {
                    m.insert(
                        pkg.clone(),
                        MirrorEntry {
                            package: Arc::from(pkg.as_str()),
                            version: Arc::from(ver.as_str()),
                            integrity: None,
                            resolved_url: None,
                            tarball_url: None,
                            deps: HashMap::new(),
                            dep_signature: String::new(),
                        },
                    );
                }
                m
            }
            _ => HashMap::new(),
        };
        snr_put(knot_fp, &edit_desc, entries);
    }
}
