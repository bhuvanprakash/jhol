//! JAGR Graph Capsule — zero-parse, mmap'd dependency graph.
//!
//! Store resolved dependency graph as flat `repr(C)` arrays.
//! Install: mmap → pointer cast → linear scan → CAS hardlink.
//! No JSON parse, no bincode, no tree build, no allocations.
//!
//! Layout: [Header][PackageEntry×N][StringTable][DepEdge×M]

use memmap2::Mmap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

// ── Constants ────────────────────────────────────────────────────────────────────

const MAGIC: [u8; 8] = *b"JHLCPL\0\0";
const CAPSULE_VERSION: u32 = 1;
const PACKAGE_ENTRY_SIZE: usize = 64;
const DEP_EDGE_SIZE: usize = 8;
const HEADER_SIZE: usize = 64;

// ── On-disk packed structures ────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CapsuleHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub package_count: u32,
    pub dep_count: u32,
    pub string_table_len: u32,
    pub manifest_hash: [u8; 8],
    pub total_unpacked_bytes: u64,
    _reserved: [u8; 24],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PackageEntry {
    pub name_off: u32,
    pub version_off: u32,
    pub tarball_hash: [u8; 32],
    pub unpacked_size: u32,
    pub dep_start: u32,
    pub dep_count: u16,
    pub flags: u16,
    pub install_order: u16,
    pub url_off: u32,         // offset into string table for tarball URL (0 = not stored)
    _reserved: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DepEdge {
    pub pkg_idx: u32,
    pub edge_type: u16,
    _reserved: u16,
}

// ── Zero-copy mmap view ──────────────────────────────────────────────────────────

pub struct CapsuleView {
    _mmap: Mmap,
    header: *const CapsuleHeader,
    entries: *const PackageEntry,
    string_table: *const u8,
    dep_edges: *const DepEdge,
    pkg_count: usize,
    str_len: usize,
    dep_count: usize,
}

// Safety: CapsuleView is Send+Sync because Mmap is Send+Sync and the raw pointers
// point into the mmap region which is immutable after construction.
unsafe impl Send for CapsuleView {}
unsafe impl Sync for CapsuleView {}

impl CapsuleView {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|e| format!("capsule open {}: {}", path.display(), e))?;
        let mmap = unsafe { Mmap::map(&file).map_err(|e| format!("capsule mmap: {}", e))? };

        if mmap.len() < HEADER_SIZE {
            return Err("capsule: file too short for header".into());
        }

        let header = mmap.as_ptr() as *const CapsuleHeader;
        let h = unsafe { &*header };

        if h.magic != MAGIC {
            return Err("capsule: bad magic".into());
        }
        if h.version != CAPSULE_VERSION {
            return Err(format!("capsule: unsupported version {}", h.version));
        }

        let pkg_count = h.package_count as usize;
        let dep_count = h.dep_count as usize;
        let str_len = h.string_table_len as usize;

        let entries_start = HEADER_SIZE;
        let entries_end = entries_start + pkg_count * PACKAGE_ENTRY_SIZE;
        let str_start = entries_end;
        let str_end = str_start + str_len;
        let deps_start = str_end;
        let deps_end = deps_start + dep_count * DEP_EDGE_SIZE;

        if mmap.len() < deps_end {
            return Err(format!("capsule: truncated ({} < {})", mmap.len(), deps_end));
        }

        let entries = unsafe { mmap.as_ptr().add(entries_start) as *const PackageEntry };
        let string_table = unsafe { mmap.as_ptr().add(str_start) };
        let dep_edges = unsafe { mmap.as_ptr().add(deps_start) as *const DepEdge };

        Ok(Self {
            _mmap: mmap,
            header,
            entries,
            string_table,
            dep_edges,
            pkg_count,
            str_len,
            dep_count,
        })
    }

    // ── Accessors ────────────────────────────────────────────────────────────────

    pub fn package_count(&self) -> usize {
        self.pkg_count
    }

    pub fn manifest_hash(&self) -> [u8; 8] {
        unsafe { (*self.header).manifest_hash }
    }

    pub fn total_unpacked_bytes(&self) -> u64 {
        unsafe { (*self.header).total_unpacked_bytes }
    }

    pub fn entry(&self, idx: usize) -> &PackageEntry {
        assert!(idx < self.pkg_count, "capsule entry idx {} >= {}", idx, self.pkg_count);
        unsafe { &*self.entries.add(idx) }
    }

    pub fn entries_slice(&self) -> &[PackageEntry] {
        unsafe { std::slice::from_raw_parts(self.entries, self.pkg_count) }
    }

    pub fn entry_name(&self, entry: &PackageEntry) -> &str {
        let start = entry.name_off as usize;
        let remaining = &unsafe { std::slice::from_raw_parts(self.string_table, self.str_len) }[start..];
        let end = remaining.iter().position(|&b| b == 0).unwrap_or(remaining.len());
        unsafe { std::str::from_utf8_unchecked(&remaining[..end]) }
    }

    pub fn entry_version(&self, entry: &PackageEntry) -> &str {
        let start = entry.version_off as usize;
        let remaining = &unsafe { std::slice::from_raw_parts(self.string_table, self.str_len) }[start..];
        let end = remaining.iter().position(|&b| b == 0).unwrap_or(remaining.len());
        unsafe { std::str::from_utf8_unchecked(&remaining[..end]) }
    }

    /// Read tarball URL from string table (zero-copy).
    pub fn entry_url(&self, entry: &PackageEntry) -> &str {
        if entry.url_off == 0 { return ""; }
        let start = entry.url_off as usize;
        let remaining = &unsafe { std::slice::from_raw_parts(self.string_table, self.str_len) }[start..];
        let end = remaining.iter().position(|&b| b == 0).unwrap_or(remaining.len());
        unsafe { std::str::from_utf8_unchecked(&remaining[..end]) }
    }

    pub fn entry_deps(&self, entry: &PackageEntry) -> &[DepEdge] {
        let start = entry.dep_start as usize;
        let count = entry.dep_count as usize;
        unsafe { std::slice::from_raw_parts(self.dep_edges.add(start), count) }
    }

    /// Verify structural integrity: all string offsets valid, dep indices in range.
    pub fn verify(&self) -> Result<(), String> {
        let str_bytes = unsafe { std::slice::from_raw_parts(self.string_table, self.str_len) };
        for i in 0..self.pkg_count {
            let e = self.entry(i);
            if e.name_off as usize >= self.str_len {
                return Err(format!("pkg {}: name_off {} >= str_len {}", i, e.name_off, self.str_len));
            }
            let name_end = str_bytes[e.name_off as usize..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| format!("pkg {}: name not null-term", i))?;
            if name_end == 0 {
                return Err(format!("pkg {}: empty name", i));
            }
            if e.version_off as usize >= self.str_len {
                return Err(format!("pkg {}: version_off {} >= str_len {}", i, e.version_off, self.str_len));
            }
            let ver_end = str_bytes[e.version_off as usize..]
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| format!("pkg {}: version not null-term", i))?;
            if ver_end == 0 {
                return Err(format!("pkg {}: empty version", i));
            }
            if (e.dep_start as usize + e.dep_count as usize) > self.dep_count {
                return Err(format!("pkg {}: deps out of range", i));
            }
            for d in self.entry_deps(e) {
                if d.pkg_idx as usize >= self.pkg_count {
                    return Err(format!("pkg {}: dep idx {} >= {}", i, d.pkg_idx, self.pkg_count));
                }
            }
        }
        Ok(())
    }
}

// ── Builder ──────────────────────────────────────────────────────────────────────

pub struct CapsuleBuilder {
    entries: Vec<u8>,
    dep_edges: Vec<u8>,
    string_table: Vec<u8>,
    string_offsets: std::collections::HashMap<String, u32>,
    pkg_count: u32,
    dep_count: u32,
    total_unpacked: u64,
    manifest_hash: [u8; 8],
}

impl CapsuleBuilder {
    pub fn new(manifest_hash: [u8; 8]) -> Self {
        Self {
            entries: Vec::new(),
            dep_edges: Vec::new(),
            string_table: Vec::new(),
            string_offsets: std::collections::HashMap::new(),
            pkg_count: 0,
            dep_count: 0,
            total_unpacked: 0,
            manifest_hash,
        }
    }

    fn intern_string(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.string_offsets.get(s) {
            return off;
        }
        let off = self.string_table.len() as u32;
        self.string_table.extend_from_slice(s.as_bytes());
        self.string_table.push(0);
        self.string_offsets.insert(s.to_string(), off);
        off
    }

    pub fn add_package(
        &mut self,
        name: &str,
        version: &str,
        tarball_hash: &[u8; 32],
        unpacked_size: u32,
        deps: &[(u32, u16)],
        install_order: u16,
        flags: u16,
        tarball_url: &str,
    ) -> u32 {
        let name_off = self.intern_string(name);
        let version_off = self.intern_string(version);
        let url_off = if tarball_url.is_empty() { 0 } else { self.intern_string(tarball_url) };
        let dep_start = self.dep_count;

        for &(pkg_idx, edge_type) in deps {
            self.dep_edges.extend_from_slice(&pkg_idx.to_le_bytes());
            self.dep_edges.extend_from_slice(&edge_type.to_le_bytes());
            self.dep_edges.extend_from_slice(&[0u8; 2]);
            self.dep_count += 1;
        }

        let idx = self.pkg_count;
        self.entries.extend_from_slice(&name_off.to_le_bytes());
        self.entries.extend_from_slice(&version_off.to_le_bytes());
        self.entries.extend_from_slice(tarball_hash);
        self.entries.extend_from_slice(&unpacked_size.to_le_bytes());
        self.entries.extend_from_slice(&dep_start.to_le_bytes());
        self.entries.extend_from_slice(&(deps.len() as u16).to_le_bytes());
        self.entries.extend_from_slice(&flags.to_le_bytes());
        self.entries.extend_from_slice(&install_order.to_le_bytes());
        self.entries.extend_from_slice(&url_off.to_le_bytes());
        self.entries.extend_from_slice(&[0u8; 6]);

        self.pkg_count += 1;
        self.total_unpacked += unpacked_size as u64;
        idx
    }

    pub fn build(mut self) -> Vec<u8> {
        // 4-byte align string table
        while self.string_table.len() % 4 != 0 {
            self.string_table.push(0);
        }

        let mut buf = Vec::with_capacity(
            HEADER_SIZE + self.entries.len() + self.string_table.len() + self.dep_edges.len()
        );

        // Header
        buf.extend_from_slice(&MAGIC);
        buf.extend_from_slice(&CAPSULE_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.pkg_count.to_le_bytes());
        buf.extend_from_slice(&self.dep_count.to_le_bytes());
        buf.extend_from_slice(&(self.string_table.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.manifest_hash);
        buf.extend_from_slice(&self.total_unpacked.to_le_bytes());
        buf.extend_from_slice(&[0u8; 24]); // reserved

        // Entries
        buf.extend_from_slice(&self.entries);

        // String table
        buf.extend_from_slice(&self.string_table);

        // Dep edges
        buf.extend_from_slice(&self.dep_edges);

        buf
    }

    pub fn package_count(&self) -> usize {
        self.pkg_count as usize
    }
}

// ── Write / Read helpers ─────────────────────────────────────────────────────────

pub fn write_capsule(path: &Path, builder: CapsuleBuilder) -> Result<(), String> {
    let data = builder.build();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("capsule mkdir: {}", e))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("capsule create tmp: {}", e))?;
        f.write_all(&data).map_err(|e| format!("capsule write: {}", e))?;
        f.sync_all().map_err(|e| format!("capsule sync: {}", e))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("capsule rename: {}", e))?;
    Ok(())
}

pub fn read_capsule(path: &Path) -> Result<CapsuleView, String> {
    let view = CapsuleView::open(path)?;
    view.verify()?;
    Ok(view)
}

pub fn capsule_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".jhol.capsule")
}

pub fn daemon_capsule_path(cache_dir: &str) -> PathBuf {
    PathBuf::from(cache_dir).join("daemon-capsule.bin")
}

// ── Compute manifest hash ────────────────────────────────────────────────────────

/// Compute the manifest hash: xxh3 of (package.json || package-lock.json).
pub fn compute_manifest_hash(project_dir: &Path) -> Result<[u8; 8], String> {
    use xxhash_rust::xxh3::xxh3_64;
    let pj = fs::read(project_dir.join("package.json"))
        .map_err(|e| format!("read package.json: {}", e))?;
    let lf = fs::read(project_dir.join("package-lock.json"))
        .map_err(|e| format!("read package-lock.json: {}", e))?;
    let mut combined = pj;
    combined.extend_from_slice(&lf);
    Ok(xxh3_64(&combined).to_le_bytes())
}

// ── Install from capsule ─────────────────────────────────────────────────────────

/// Install all packages from a capsule into node_modules.
/// Uses the existing sync APIs: store_dir for tarballs, selective_extract for extraction.
pub fn install_from_capsule(view: &CapsuleView, project_dir: &Path) -> Result<(), String> {
    let node_modules = project_dir.join("node_modules");
    fs::create_dir_all(&node_modules).map_err(|e| format!("capsule: mkdir node_modules: {}", e))?;
    fs::create_dir_all(node_modules.join(".bin")).map_err(|e| format!("capsule: mkdir .bin: {}", e))?;

    let cache_dir = crate::utils::get_cache_dir();
    let store_dir = PathBuf::from(&cache_dir).join("store");
    let nm = &node_modules;
    let n = view.package_count();

    let mut install_any = false;

    for i in 0..n {
        let entry = view.entry(i);
        let name = view.entry_name(entry);
        let version = view.entry_version(entry);
        let tarball_hex = hex::encode(entry.tarball_hash);

        let pkg_dir = if name.starts_with('@') {
            let slash = name.find('/').ok_or_else(|| format!("invalid scoped name: {}", name))?;
            let scope_dir = nm.join(&name[..slash]);
            fs::create_dir_all(&scope_dir).map_err(|e| format!("capsule: mkdir scope: {}", e))?;
            scope_dir.join(&name[slash + 1..])
        } else {
            nm.join(name)
        };

        if pkg_dir.join("package.json").exists() {
            continue; // already installed
        }

        install_any = true;

        // Check store
        let tarball_file = store_dir.join(format!("{}.tgz", tarball_hex));
        if tarball_file.exists() {
            crate::selective_extract::extract_selective_to_path(&tarball_file, &pkg_dir)
                .map_err(|e| format!("capsule: extract {}: {}", name, e))?;
        } else {
            // Fetch from registry — use URL from capsule if available
            let url = if entry.url_off != 0 {
                view.entry_url(entry).to_string()
            } else {
                format!(
                    "https://registry.npmjs.org/{}/-/{}-{}.tgz",
                    name.replace('/', "%2F"),
                    name.split('/').last().unwrap_or(name),
                    version
                )
            };
            let bytes = crate::http_client::get_bytes_with_bearer(url.as_str(), None)
                .map_err(|e| format!("capsule: fetch {}@{}: {}", name, version, e))?;

            // Save to store
            let store_path = store_dir.join(format!("{}.tgz", tarball_hex));
            fs::create_dir_all(&store_dir).ok();
            {
                let mut f = fs::File::create(&store_path)
                    .map_err(|e| format!("capsule: store file: {}", e))?;
                f.write_all(&bytes).map_err(|e| format!("capsule: write store: {}", e))?;
            }

            // Extract
            crate::selective_extract::extract_selective_to_path(&store_path, &pkg_dir)
                .map_err(|e| format!("capsule: extract {}: {}", name, e))?;
        }

        // Update store index
        let mut index = crate::utils::read_store_index();
        index.insert(format!("{}@{}", name, version), tarball_hex.clone());
        crate::utils::write_store_index(&index).ok();
    }

    if install_any {
        crate::bin_links::rebuild_bin_links(nm).ok();
    }

    Ok(())
}

// ── Build capsule from project directory ────────────────────────────────────────

/// Build and write a capsule from the current project state.
/// Reads package-lock.json and store index to construct the capsule.
/// Stores each package name, version, tarball hash (from store index),
/// and install_order (topological depth).
/// Dependency edges are not stored in this simplified version — the linear
/// walk by install_order is sufficient for the install fast path.
pub fn write_capsule_from_project(project_dir: &Path) -> Result<(), String> {
    let lockfile_path = project_dir.join("package-lock.json");
    if !lockfile_path.exists() {
        return Ok(());
    }

    let manifest_hash = compute_manifest_hash(project_dir)?;
    let lockfile_bytes = std::fs::read(&lockfile_path)
        .map_err(|e| format!("read lockfile: {}", e))?;
    let lockfile: serde_json::Value = serde_json::from_slice(&lockfile_bytes)
        .map_err(|e| format!("parse lockfile: {}", e))?;

    let packages = match lockfile.get("packages").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(()),
    };

    // Filter out root package (key = "")
    let dep_keys: Vec<&String> = packages.keys().filter(|k| !k.is_empty()).collect();
    if dep_keys.is_empty() {
        return Ok(());
    }

    let store_index = crate::utils::read_store_index();
    let mut builder = CapsuleBuilder::new(manifest_hash);

    for key in &dep_keys {
        let pkg = &packages[*key];
        let name = key.strip_prefix("node_modules/").unwrap_or(key.as_str());
        let version = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0");
        let tarball_hash = resolve_tarball_hash(name, version, pkg, &store_index);
        let unpacked_size = pkg.get("integrity").map(|_| 0u32).unwrap_or(0);
        let flags = extract_flags(pkg);
        // install_order based on key depth — deeper = later
        let depth = key.matches('/').count() as u16;
        builder.add_package(name, version, &tarball_hash, unpacked_size, &[], depth, flags, "");
    }

    let cap_path = capsule_path(project_dir);
    write_capsule(&cap_path, builder)?;

    Ok(())
}

fn extract_flags(pkg: &serde_json::Value) -> u16 {
    let mut flags = 0u16;
    if pkg.get("dev").and_then(|v| v.as_bool()).unwrap_or(false) { flags |= 1; }
    if pkg.get("peer").and_then(|v| v.as_bool()).unwrap_or(false) { flags |= 2; }
    if pkg.get("optional").and_then(|v| v.as_bool()).unwrap_or(false) { flags |= 4; }
    flags
}

fn resolve_tarball_hash(
    name: &str,
    version: &str,
    pkg: &serde_json::Value,
    store_index: &std::collections::HashMap<String, String>,
) -> [u8; 32] {
    use base64::Engine;
    let index_key = format!("{}@{}", name, version);
    // Try store index first
    if let Some(hex_hash) = store_index.get(&index_key) {
        if let Ok(bytes) = hex::decode(hex_hash) {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                return arr;
            }
        }
    }
    // Try integrity field (sha256- or sha512- base64)
    if let Some(integrity) = pkg.get("integrity").and_then(|v| v.as_str()) {
        if let Some(b64) = integrity.strip_prefix("sha256-") {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64) {
                if decoded.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&decoded);
                    return arr;
                }
            }
        }
        if let Some(b64) = integrity.strip_prefix("sha512-") {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64) {
                // SHA512 is 64 bytes — take first 32
                let mut arr = [0u8; 32];
                let len = decoded.len().min(32);
                arr[..len].copy_from_slice(&decoded[..len]);
                return arr;
            }
        }
    }
    [0u8; 32]
}

// ── Tests ────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn make_tmp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let c = COUNTER.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("jhol-capsule-test-{}-{}", std::process::id(), c));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_build_and_read_roundtrip() {
        let dir = make_tmp_dir();
        let path = dir.join("test.jhlcpl");
        let mut builder = CapsuleBuilder::new([0xab; 8]);

        let idx0 = builder.add_package("left-pad", "1.0.0", &[0x01; 32], 1024, &[], 0, 0, "");
        let dep1 = [(idx0, 0)];
        let idx1 = builder.add_package("is-positive", "1.0.0", &[0x02; 32], 512, &dep1, 1, 0, "");
        let dep2 = [(idx1, 0)];
        let idx2 = builder.add_package("@scope/foo", "2.1.3", &[0x03; 32], 2048, &dep2, 2, 0, "");
        let dep3 = [(idx0, 0), (idx2, 0)];
        builder.add_package("bar", "0.5.0", &[0x04; 32], 256, &dep3, 3, 0, "");

        assert_eq!(builder.package_count(), 4);

        write_capsule(&path, builder).unwrap();

        let view = read_capsule(&path).unwrap();
        assert_eq!(view.package_count(), 4);
        assert_eq!(view.manifest_hash(), [0xab; 8]);

        let e0 = view.entry(0);
        assert_eq!(view.entry_name(e0), "left-pad");
        assert_eq!(view.entry_version(e0), "1.0.0");
        assert_eq!(e0.unpacked_size, 1024);
        assert_eq!(e0.install_order, 0);
        assert_eq!(e0.dep_count, 0);

        let e3 = view.entry(3);
        assert_eq!(view.entry_name(e3), "bar");
        assert_eq!(view.entry_version(e3), "0.5.0");
        assert_eq!(e3.dep_count, 2);
        assert_eq!(e3.unpacked_size, 256);
        let deps = view.entry_deps(e3);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].pkg_idx, 0);
        assert_eq!(deps[1].pkg_idx, 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_corrupt_capsule_rejected() {
        let dir = make_tmp_dir();
        let mut builder = CapsuleBuilder::new([0xab; 8]);
        builder.add_package("pkg", "1.0", &[0x01; 32], 100, &[], 0, 0, "");
        let mut data = builder.build();
        data[0] = 0xFF; // corrupt magic
        let path = dir.join("bad.jhlcpl");
        fs::write(&path, &data).unwrap();
        assert!(read_capsule(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_truncated_capsule_rejected() {
        let dir = make_tmp_dir();
        let builder = CapsuleBuilder::new([0x00; 8]);
        let data = builder.build();
        let truncated = &data[..data.len().min(16)];
        let path = dir.join("trunc.jhlcpl");
        fs::write(&path, truncated).unwrap();
        assert!(read_capsule(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_empty_capsule() {
        let dir = make_tmp_dir();
        let builder = CapsuleBuilder::new([0x00; 8]);
        let path = dir.join("empty.jhlcpl");
        write_capsule(&path, builder).unwrap();
        let view = read_capsule(&path).unwrap();
        assert_eq!(view.package_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_string_table_intern_shared() {
        let dir = make_tmp_dir();
        let mut builder = CapsuleBuilder::new([0x00; 8]);
        // Two packages with same name but different versions
        builder.add_package("lodash", "4.17.21", &[0x01; 32], 100, &[], 0, 0, "");
        builder.add_package("lodash", "4.17.20", &[0x02; 32], 100, &[], 1, 0, "");
        let path = dir.join("shared.jhlcpl");
        write_capsule(&path, builder).unwrap();
        let view = read_capsule(&path).unwrap();
        assert_eq!(view.package_count(), 2);
        // Both see same "lodash" string at same offset
        let e0 = view.entry(0);
        let e1 = view.entry(1);
        assert_eq!(e0.name_off, e1.name_off);
        assert_ne!(e0.version_off, e1.version_off);
        assert_eq!(view.entry_name(e0), "lodash");
        assert_eq!(view.entry_name(e1), "lodash");
        assert_eq!(view.entry_version(e0), "4.17.21");
        assert_eq!(view.entry_version(e1), "4.17.20");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manifest_hash_compute() {
        let dir = make_tmp_dir();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        fs::write("package.json", b"{}").unwrap();
        fs::write("package-lock.json", b"{}").unwrap();
        let hash = compute_manifest_hash(Path::new(".")).unwrap();
        let hash2 = compute_manifest_hash(Path::new(".")).unwrap();
        assert_eq!(hash, hash2);
        fs::write("package.json", b"{\"name\":\"x\"}").unwrap();
        let hash3 = compute_manifest_hash(Path::new(".")).unwrap();
        assert_ne!(hash, hash3);
        std::env::set_current_dir(&original_dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
