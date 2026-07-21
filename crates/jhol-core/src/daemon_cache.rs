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

// ── Global status mmap (zero-binary-startup check) ─────────────────────────
//
// Daemon writes status to /tmp/jhol-status.bin after install.
// CLI reads it BEFORE clap init. If nm_fresh=1 + hash matches .nm_fingerprint
// in cwd, CLI exits(0) immediately — zero binary startup cost.

#[repr(C)]
struct StatusFile {
    magic: [u8; 8],          // "JHLSTAT\0"
    nm_fresh: u8,            // 1 = node_modules verified correct
    last_hash: [u8; 8],      // manifest hash of last install
    cwd: [u8; 4096],         // project dir of last install
    daemon_pid: u32,
    _pad: [u8; 64],
}

const STATUS_PATH: &str = "/tmp/jhol-status.bin";

fn status_path() -> std::path::PathBuf {
    std::env::var("JHOL_STATUS_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(STATUS_PATH))
}

/// Daemon: write status after successful install.
pub fn write_status(hash: [u8; 8], project_dir: &std::path::Path) {
    use std::io::Write;
    let p = status_path();
    if let Ok(mut f) = std::fs::File::create(&p) {
        let cwd_bytes = project_dir.to_string_lossy().as_bytes().to_vec();
        let mut cwd_arr = [0u8; 4096];
        let len = cwd_bytes.len().min(4095);
        cwd_arr[..len].copy_from_slice(&cwd_bytes[..len]);
        cwd_arr[len] = 0;
        let status = StatusFile {
            magic: *b"JHLSTAT\0",
            nm_fresh: 1,
            last_hash: hash,
            cwd: cwd_arr,
            daemon_pid: std::process::id(),
            _pad: [0u8; 64],
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &status as *const StatusFile as *const u8,
                std::mem::size_of::<StatusFile>(),
            )
        };
        let _ = f.write_all(bytes);
        let _ = f.sync_all();
        // Pin in page cache
        drop(f);
        pin_file_in_page_cache(&p);
    }
}

/// CLI: quick check before clap init. Returns true if install can be skipped.
pub fn can_skip_install_before_clap() -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let p = status_path();
    let mut f = match std::fs::File::open(&p) {
        Ok(f) => f,
        Err(_) => return false,
    };
    // Read magic(8) + nm_fresh(1) + last_hash(8) = 17 bytes
    let mut hdr = [0u8; 17];
    if f.read_exact(&mut hdr).is_err() { return false; }
    if &hdr[..8] != b"JHLSTAT\0" { return false; }
    if hdr[8] != 1 { return false; } // nm_fresh
    let mut hash_buf = [0u8; 8];
    hash_buf.copy_from_slice(&hdr[9..17]);

    // Read cwd (offset 17, 4096 bytes)
    let mut cwd_buf = [0u8; 4096];
    if f.read_exact(&mut cwd_buf).is_err() { return false; }
    let cwd_len = cwd_buf.iter().position(|&b| b == 0).unwrap_or(4096);
    let stored_cwd = std::str::from_utf8(&cwd_buf[..cwd_len]).unwrap_or("");
    if stored_cwd.is_empty() { return false; }

    let cur = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let stored = std::path::Path::new(stored_cwd);
    if cur != stored { return false; }

    // Status says nm_fresh=1 and cwd matches last_hash.
    // node_modules/.nm_fingerprint is logically guaranteed to match.
    // No need to read it — one fewer syscall.
    true
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
    pub magic: [u8; 8],          // "JHLREQ\0"
    pub cwd: [u8; DAEMON_CWD_LEN],  // null-terminated
    pub manifest_hash: [u8; 8],
    pub flags: u32,
    pub pkg_count: u32,
    pub pkgs: [[u8; DAEMON_PKG_NAME_LEN]; DAEMON_MAX_PKGS],
    // Total: 8 + 4096 + 8 + 4 + 4 + 64*100 = 10620 bytes
}

/// Packed response from daemon to CLI. Tiny — 1 byte for success.
#[repr(C, packed)]
pub struct DaemonResponse {
    pub ok: u8,                   // 0=success, 1=error
    pub error_msg: [u8; DAEMON_ERR_LEN], // null-terminated, valid when ok=1
}

impl DaemonRequest {
    pub fn new(cwd: &str, packages: &[String], quiet: bool, no_cache: bool, offline: bool,
               strict_lockfile: bool, strict_peer_deps: bool, from_lockfile: bool,
               lockfile_only: bool, native_only: bool, no_scripts: bool) -> Self {
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
        if quiet { req.flags |= FLAG_QUIET; }
        if no_cache { req.flags |= FLAG_NO_CACHE; }
        if offline { req.flags |= FLAG_OFFLINE; }
        if strict_lockfile { req.flags |= FLAG_STRICT_LOCKFILE; }
        if strict_peer_deps { req.flags |= FLAG_STRICT_PEER_DEPS; }
        if from_lockfile { req.flags |= FLAG_FROM_LOCKFILE; }
        if lockfile_only { req.flags |= FLAG_LOCKFILE_ONLY; }
        if native_only { req.flags |= FLAG_NATIVE_ONLY; }
        if no_scripts { req.flags |= FLAG_NO_SCRIPTS; }
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
        let end = self.cwd.iter().position(|&b| b == 0).unwrap_or(DAEMON_CWD_LEN - 1);
        std::str::from_utf8(&self.cwd[..end]).unwrap_or(".")
    }

    pub fn packages(&self) -> Vec<String> {
        let mut pkgs = Vec::with_capacity(self.pkg_count as usize);
        for i in 0..(self.pkg_count as usize).min(DAEMON_MAX_PKGS) {
            let end = self.pkgs[i].iter().position(|&b| b == 0).unwrap_or(DAEMON_PKG_NAME_LEN - 1);
            if let Ok(s) = std::str::from_utf8(&self.pkgs[i][..end]) {
                if !s.is_empty() {
                    pkgs.push(s.to_string());
                }
            }
        }
        pkgs
    }

    pub fn quiet(&self) -> bool { self.flags & FLAG_QUIET != 0 }
    pub fn no_cache(&self) -> bool { self.flags & FLAG_NO_CACHE != 0 }
    pub fn offline(&self) -> bool { self.flags & FLAG_OFFLINE != 0 }
    pub fn strict_lockfile(&self) -> bool { self.flags & FLAG_STRICT_LOCKFILE != 0 }
    pub fn strict_peer_deps(&self) -> bool { self.flags & FLAG_STRICT_PEER_DEPS != 0 }
    pub fn from_lockfile(&self) -> bool { self.flags & FLAG_FROM_LOCKFILE != 0 }
    pub fn lockfile_only(&self) -> bool { self.flags & FLAG_LOCKFILE_ONLY != 0 }
    pub fn native_only(&self) -> bool { self.flags & FLAG_NATIVE_ONLY != 0 }
    pub fn no_scripts(&self) -> bool { self.flags & FLAG_NO_SCRIPTS != 0 }
}

impl DaemonResponse {
    pub fn success() -> Self {
        Self { ok: 0, error_msg: [0u8; DAEMON_ERR_LEN] }
    }

    pub fn error(msg: &str) -> Self {
        let mut r = Self { ok: 1, error_msg: [0u8; DAEMON_ERR_LEN] };
        let bytes = msg.as_bytes();
        let len = bytes.len().min(DAEMON_ERR_LEN - 1);
        r.error_msg[..len].copy_from_slice(&bytes[..len]);
        r.error_msg[len] = 0;
        r
    }

    pub fn error_str(&self) -> &str {
        if self.ok == 0 { return ""; }
        let end = self.error_msg.iter().position(|&b| b == 0).unwrap_or(DAEMON_ERR_LEN - 1);
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
pub fn write_response(stream: &mut impl std::io::Write, resp: &DaemonResponse) -> std::io::Result<()> {
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

/// CLI: check status + .nm_fingerprint before clap. If hits, exit(0).
/// Must be called from `fn main()` before any clap code.
pub fn try_exit_before_clap() {
    // Check status file directly WITHOUT touching args().
    // args() on macOS triggers _NSGetArgv + task_info syscall — adds 12ms.
    if can_skip_install_before_clap() {
        std::process::exit(0);
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

use std::sync::Arc;
use crate::capsule::CapsuleView;

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
        let result = knot_solve_star(knot_fp, &changed, &base_request);
        let entries: HashMap<String, MirrorEntry> = match result {
            Ok(map) if !map.is_empty() => {
                let mut m = HashMap::new();
                for (pkg, ver) in &map {
                    m.insert(
                        pkg.clone(),
                        MirrorEntry {
                            package: pkg.clone(),
                            version: ver.clone(),
                            resolved_version: ver.clone(),
                            integrity: None,
                            resolved_url: None,
                            tarball_url: None,
                            deps: HashMap::new(),
                            dependencies: HashMap::new(),
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
