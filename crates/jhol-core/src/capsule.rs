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
use std::mem::size_of;
use std::path::{Path, PathBuf};

const _: () = assert!(size_of::<CapsuleHeader>() == 64, "CapsuleHeader must be 64 bytes");
const _: () = assert!(size_of::<PackageEntry>() == 64, "PackageEntry must be 64 bytes");
const _: () = assert!(size_of::<DepEdge>() == 8, "DepEdge must be 8 bytes");

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
    pub spectral_gap: f64,
    pub predicted_beta1: u64,
    pub knot_fingerprint: u64,
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
    pub url_off: u32,
    pub install_order: u16,
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
    /// Optional PBF trailer (after dep edges).
    pbf: Option<u64>,
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

        // Optional 8-byte PBF trailer after dep edges (KPSL).
        let pbf = if mmap.len() >= deps_end + 8 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&mmap[deps_end..deps_end + 8]);
            Some(u64::from_le_bytes(b))
        } else {
            None
        };

        Ok(Self {
            _mmap: mmap,
            header,
            entries,
            string_table,
            dep_edges,
            pkg_count,
            str_len,
            dep_count,
            pbf,
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

    pub fn spectral_gap(&self) -> f64 {
        unsafe { (*self.header).spectral_gap }
    }

    pub fn predicted_beta1(&self) -> u64 {
        unsafe { (*self.header).predicted_beta1 }
    }

    pub fn knot_fingerprint(&self) -> u64 {
        unsafe { (*self.header).knot_fingerprint }
    }

    /// Persistence barcode fingerprint stored in capsule trailer (if present).
    pub fn pbf(&self) -> Option<u64> {
        self.pbf
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
    spectral_gap: f64,
    predicted_beta1: u64,
    knot_fingerprint: u64,
    /// Persistence barcode fingerprint (PBF) trailer — written after dep edges.
    pbf: u64,
    has_pbf: bool,
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
            spectral_gap: 1.0,
            predicted_beta1: 0,
            knot_fingerprint: 0,
            pbf: 0,
            has_pbf: false,
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
        self.entries.extend_from_slice(&url_off.to_le_bytes());
        self.entries.extend_from_slice(&install_order.to_le_bytes());
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
        buf.extend_from_slice(&self.spectral_gap.to_le_bytes());
        buf.extend_from_slice(&self.predicted_beta1.to_le_bytes());
        buf.extend_from_slice(&self.knot_fingerprint.to_le_bytes());

        // Entries
        buf.extend_from_slice(&self.entries);

        // String table
        buf.extend_from_slice(&self.string_table);

        // Dep edges
        buf.extend_from_slice(&self.dep_edges);

        // Optional PBF trailer (8 bytes). Old readers ignore trailing bytes.
        if self.has_pbf {
            buf.extend_from_slice(&self.pbf.to_le_bytes());
        }

        buf
    }

    pub fn set_spectral(&mut self, spectral_gap: f64, predicted_beta1: u64) {
        self.spectral_gap = spectral_gap;
        self.predicted_beta1 = predicted_beta1;
    }

    pub fn set_knot_fingerprint(&mut self, fp: u64) {
        self.knot_fingerprint = fp;
    }

    pub fn set_pbf(&mut self, pbf: u64) {
        self.pbf = pbf;
        self.has_pbf = true;
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
/// Soul Path: mmap → install_order waves → JAGR-S / store_unpacked project.
pub fn install_from_capsule(view: &CapsuleView, project_dir: &Path) -> Result<(), String> {
    // Prefer waveform path (parallel by install_order)
    if crate::soul_path::stream_install_enabled() || view.package_count() > 0 {
        return crate::soul_path::waveform_install_capsule(view, project_dir).or_else(|e| {
            crate::utils::log(&format!(
                "Soul Path waveform fallback to serial capsule: {}",
                e
            ));
            install_from_capsule_serial(view, project_dir)
        });
    }
    install_from_capsule_serial(view, project_dir)
}

fn install_from_capsule_serial(view: &CapsuleView, project_dir: &Path) -> Result<(), String> {
    crate::soul_path::ensure_jagrs_migrated();
    let node_modules = project_dir.join("node_modules");
    fs::create_dir_all(&node_modules).map_err(|e| format!("capsule: mkdir node_modules: {}", e))?;
    fs::create_dir_all(node_modules.join(".bin"))
        .map_err(|e| format!("capsule: mkdir .bin: {}", e))?;

    let cache_dir = PathBuf::from(crate::utils::get_cache_dir());
    let store_dir = cache_dir.join("store");
    let n = view.package_count();
    let mut install_any = false;
    let mut index_batch: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // Sort by install_order for serial correctness
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| view.entry(i).install_order);

    for i in order {
        let entry = view.entry(i);
        let name = view.entry_name(entry);
        let version = view.entry_version(entry);
        let tarball_hex = hex::encode(entry.tarball_hash);

        let pkg_dir = if name.starts_with('@') {
            let slash = name
                .find('/')
                .ok_or_else(|| format!("invalid scoped name: {}", name))?;
            let scope_dir = node_modules.join(&name[..slash]);
            fs::create_dir_all(&scope_dir).map_err(|e| format!("capsule: mkdir scope: {}", e))?;
            scope_dir.join(&name[slash + 1..])
        } else {
            node_modules.join(name)
        };

        if pkg_dir.join("package.json").exists() {
            continue;
        }
        install_any = true;

        if crate::soul_path::project_package(name, version, &tarball_hex, &node_modules, &cache_dir)
            .is_ok()
        {
            index_batch.insert(format!("{}@{}", name, version), tarball_hex);
            continue;
        }

        // Cold: fetch URL from capsule
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
        let pkg_key = format!("{}@{}", name, version);
        let hash: String = crate::jagrs::download_and_stream_to_store(&url, cache_dir.to_str().unwrap_or("."), &pkg_key)
            .or_else(|_| -> Result<String, String> {
                let bytes = crate::http_client::get_bytes_with_bearer(&url, None)?;
                fs::create_dir_all(&store_dir).ok();
                let store_path = store_dir.join(format!("{}.tgz", tarball_hex));
                fs::write(&store_path, &bytes).map_err(|e| e.to_string())?;
                let _ = crate::jagrs::store_tarball(&bytes);
                let _ = crate::registry::ensure_unpacked_in_store(&store_path, &cache_dir);
                Ok(tarball_hex.clone())
            })?;
        crate::soul_path::project_package(name, version, &hash, &node_modules, &cache_dir)
            .map_err(|e| format!("capsule: project {}: {}", name, e))?;
        index_batch.insert(format!("{}@{}", name, version), hash);
    }

    if !index_batch.is_empty() {
        let mut index = crate::utils::read_store_index();
        index.extend(index_batch);
        let _ = crate::utils::write_store_index(&index);
    }
    if install_any {
        crate::bin_links::rebuild_bin_links(&node_modules).ok();
    }
    Ok(())
}

// ── Build capsule from project directory ────────────────────────────────────────

/// Build and write a capsule from the current project state.
/// Real DepEdges + topo install_order (Soul Path graph plane).
pub fn write_capsule_from_project(project_dir: &Path) -> Result<(), String> {
    let lockfile_path = project_dir.join("package-lock.json");
    if !lockfile_path.exists() {
        return Ok(());
    }

    let manifest_hash = compute_manifest_hash(project_dir)?;
    let lockfile_bytes =
        std::fs::read(&lockfile_path).map_err(|e| format!("read lockfile: {}", e))?;
    let lockfile: serde_json::Value =
        serde_json::from_slice(&lockfile_bytes).map_err(|e| format!("parse lockfile: {}", e))?;

    let packages = match lockfile.get("packages").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(()),
    };

    // name → lockfile key (prefer top-level node_modules/<name>)
    let mut name_to_key: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut ordered_names: Vec<String> = Vec::new();
    for key in packages.keys() {
        if key.is_empty() {
            continue;
        }
        let name = lockfile_key_to_name(key);
        // Prefer shallowest key for a name
        let depth = key.matches("node_modules").count();
        match name_to_key.get(&name) {
            Some(existing) => {
                let ed = existing.matches("node_modules").count();
                if depth < ed {
                    name_to_key.insert(name.clone(), key.clone());
                }
            }
            None => {
                name_to_key.insert(name.clone(), key.clone());
                ordered_names.push(name);
            }
        }
    }
    if ordered_names.is_empty() {
        return Ok(());
    }

    // name → capsule index
    let mut name_idx: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (i, n) in ordered_names.iter().enumerate() {
        name_idx.insert(n.clone(), i as u32);
    }

    // Build adjacency for topo order
    let mut edges_by_name: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut indeg: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for n in &ordered_names {
        indeg.entry(n.clone()).or_insert(0);
    }
    for name in &ordered_names {
        let key = match name_to_key.get(name) {
            Some(k) => k,
            None => continue,
        };
        let pkg = &packages[key];
        let mut deps = Vec::new();
        if let Some(obj) = pkg.get("dependencies").and_then(|d| d.as_object()) {
            for dname in obj.keys() {
                if name_idx.contains_key(dname) {
                    deps.push(dname.clone());
                    *indeg.entry(dname.clone()).or_insert(0) += 1;
                }
            }
        }
        edges_by_name.insert(name.clone(), deps);
    }

    // Kahn topo → install_order (deps first = lower order)
    let mut order_map: std::collections::HashMap<String, u16> = std::collections::HashMap::new();
    let mut queue: std::collections::VecDeque<String> = indeg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    // Reverse edges: dep → dependents for Kahn on "deps first"
    // Actually indeg counts how many packages depend ON this? We incremented for deps of name,
    // so indeg[dep] = number of packages that list dep. Roots have indeg 0 = nothing depends... wait
    // We want deps installed BEFORE dependents. So edge name→dep means name needs dep first.
    // indeg[dep] was incremented when name depends on dep — that's wrong for "deps first".
    // Fix: indeg[name] = number of deps name still needs.
    let mut indeg2: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut rev: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for n in &ordered_names {
        indeg2.insert(n.clone(), 0);
    }
    for (name, deps) in &edges_by_name {
        indeg2.insert(name.clone(), deps.len() as u32);
        for d in deps {
            rev.entry(d.clone()).or_default().push(name.clone());
        }
    }
    queue = indeg2
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut wave: u16 = 0;
    let mut placed = 0usize;
    while !queue.is_empty() {
        let layer: Vec<String> = queue.drain(..).collect();
        for n in &layer {
            order_map.insert(n.clone(), wave);
            placed += 1;
            if let Some(dependents) = rev.get(n) {
                for depn in dependents {
                    if let Some(c) = indeg2.get_mut(depn) {
                        *c = c.saturating_sub(1);
                        if *c == 0 {
                            queue.push_back(depn.clone());
                        }
                    }
                }
            }
        }
        wave = wave.saturating_add(1);
    }
    // Cycles / leftovers
    for n in &ordered_names {
        order_map.entry(n.clone()).or_insert(wave);
        if !order_map.contains_key(n) {
            placed += 1;
        }
    }
    let _ = placed;

    let store_index = crate::utils::read_store_index();
    let mut builder = CapsuleBuilder::new(manifest_hash);

    // Set spectral gate fields from DSSC proxy on root deps
    if let Ok(pj_bytes) = std::fs::read(project_dir.join("package.json")) {
        if let Ok(pj) = serde_json::from_slice::<serde_json::Value>(&pj_bytes) {
            if let Some(deps_obj) = pj.get("dependencies").and_then(|d| d.as_object()) {
                let deps: std::collections::HashMap<String, String> = deps_obj
                    .iter()
                    .map(|(k, v)| {
                        (k.clone(), v.as_str().unwrap_or("*").to_string())
                    })
                    .collect();
                let gate = crate::soul_path::spectral_gate_from_deps(&deps);
                builder.set_spectral(gate.spectral_gap, gate.predicted_beta1 as u64);
            }
        }
    }

    // Knot fingerprint + PBF trailer for KPSL classifier
    if let Ok(pj_bytes) = std::fs::read(project_dir.join("package.json")) {
        if let Ok(pj) = serde_json::from_slice::<serde_json::Value>(&pj_bytes) {
            if let Some(deps_obj) = pj.get("dependencies").and_then(|d| d.as_object()) {
                let deps_map: std::collections::HashMap<String, String> = deps_obj
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("*").to_string()))
                    .collect();
                let fp = crate::resolution_mirror::fingerprint_deps(&deps_map);
                builder.set_knot_fingerprint(fp);
                let pbf = crate::jagr::resolution_differential::pbf_from_deps(&deps_map);
                builder.set_pbf(pbf);
            }
        }
    }

    // Add packages in ordered_names order so indices match name_idx
    for name in &ordered_names {
        let key = name_to_key.get(name).unwrap();
        let pkg = &packages[key];
        let version = pkg
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0");
        let tarball_hash = resolve_tarball_hash(name, version, pkg, &store_index);
        let unpacked_size = 0u32;
        let flags = extract_flags(pkg);
        let install_order = *order_map.get(name).unwrap_or(&0);
        let url = pkg
            .get("resolved")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let mut deps_idx: Vec<(u32, u16)> = Vec::new();
        if let Some(dlist) = edges_by_name.get(name) {
            for d in dlist {
                if let Some(&idx) = name_idx.get(d) {
                    deps_idx.push((idx, 0)); // 0 = runtime dep
                }
            }
        }
        builder.add_package(
            name,
            version,
            &tarball_hash,
            unpacked_size,
            &deps_idx,
            install_order,
            flags,
            url,
        );
    }

    let cap_path = capsule_path(project_dir);
    write_capsule(&cap_path, builder)?;

    // Write install markers for zero-binary-startup fast path
    crate::daemon_cache::write_nm_fingerprint(project_dir, manifest_hash);
    crate::daemon_cache::store_last_hash(manifest_hash);
    crate::daemon_cache::write_status(manifest_hash, project_dir);

    Ok(())
}

fn lockfile_key_to_name(key: &str) -> String {
    // node_modules/foo or node_modules/@s/p or nested .../node_modules/foo
    if let Some(idx) = key.rfind("node_modules/") {
        return key[idx + "node_modules/".len()..].to_string();
    }
    key.trim_start_matches("node_modules/").to_string()
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

/// Classifier: look up capsule by knot fingerprint AND manifest hash.
/// Returns Some(CapsuleView) if fingerprint matches, None otherwise.
pub fn classifier_lookup(
    project_dir: &Path,
    knot_fp: u64,
) -> Option<CapsuleView> {
    let cap_path = capsule_path(project_dir);
    if !cap_path.exists() {
        return None;
    }
    let view = CapsuleView::open(&cap_path).ok()?;
    if view.knot_fingerprint() != knot_fp {
        return None;
    }
    view.verify().ok()?;
    Some(view)
}

/// Write CGA capsule (git-committable) — includes knot fingerprint for classifier.
/// Writes to `.jhol/capsule.bin` path (git-friendly).
pub fn write_cga_capsule(
    project_dir: &Path,
    builder: CapsuleBuilder,
) -> Result<(), String> {
    let cap_path = cga_capsule_path(project_dir);
    if let Some(parent) = cap_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cga mkdir: {}", e))?;
    }
    write_capsule(&cap_path, builder)
}

/// Build and write a CGA capsule from the current project state.
/// Convenience wrapper around write_capsule_from_project + .jhol/capsule.bin.
pub fn write_cga_capsule_from_project(project_dir: &Path) -> Result<(), String> {
    // Build regular capsule first
    write_capsule_from_project(project_dir)?;
    // Copy to CGA path if it doesn't exist already
    let src = capsule_path(project_dir);
    let dst = cga_capsule_path(project_dir);
    if src != dst && src.exists() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cga mkdir: {}", e))?;
        }
        fs::copy(&src, &dst).map_err(|e| format!("cga copy: {}", e))?;
    }
    Ok(())
}

/// Path to the CGA capsule artifact (`.jhol/capsule.bin`).
pub fn cga_capsule_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".jhol").join("capsule.bin")
}

/// Try to install from CGA capsule at `.jhol/capsule.bin`.
/// Validates knot fingerprint + manifest hash before installing.
pub fn try_git_capsule_install(
    project_dir: &Path,
    knot_fp: u64,
    manifest_hash: [u8; 8],
    quiet: bool,
) -> Result<bool, String> {
    let cap_path = cga_capsule_path(project_dir);
    if !cap_path.exists() {
        return Ok(false);
    }
    let view = CapsuleView::open(&cap_path)?;
    // Check manifest hash (fast)
    if view.manifest_hash() != manifest_hash {
        return Ok(false);
    }
    // Check knot fingerprint (cohomology match)
    if view.knot_fingerprint() != knot_fp {
        return Ok(false);
    }
    view.verify()?;
    if !quiet {
        crate::utils::log("KCC³: CGA capsule hit — installing from .jhol/capsule.bin");
    }
    install_from_capsule(&view, project_dir)?;
    Ok(true)
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
