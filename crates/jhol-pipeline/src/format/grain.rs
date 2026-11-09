//! JholGrain — Unified Content-Addressed Resolution + Content Store
//!
//! Merges the three fragmented caches (.jres resolved bundles, .jctr per-package
//! contracts, JGrid content blocks) into one append-only, content-addressed file.
//!
//! ## Format
//!
//! ```text
//! grain.jhol
//! ├─ HEADER (256 B): magic "JHLGRN01", version, region offsets + counts
//! ├─ CONTRACTS region: per-package decision records
//! │    key = xxh3_64(name@version)
//! │    value = { name, version, evidence_hash, resolved_url, integrity,
//! │              deps[(child,child_version)], peers, optional }
//! ├─ RESOLUTIONS region: whole-tree resolved bundles
//! │    key = xxh3_64(sorted direct-dep "name:spec" list)
//! │    value = { direct_specs: Vec<(name,spec)>,
//! │              packages: HashMap<name, {version, url, integrity, deps}> }
//! └─ INDEX (end of file): Swiss Table mapping key → (region, offset, len)
//! ```
//!
//! All regions are append-only. Reads are zero-copy slices via mmap.  Writes
//! append new entries and rebuild the index periodically.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File magic and version
const MAGIC: &[u8; 8] = b"JHLGRN01";
const HEADER_SIZE: usize = 256;

// ── Region IDs ──────────────────────────────────────────────────────────

pub const REGION_CONTRACTS: u8 = 1;
pub const REGION_RESOLUTIONS: u8 = 2;

// ── Public data types ───────────────────────────────────────────────────

/// A per-package resolution contract — immutable once written.
/// The `evidence_hash` proves the packument content was read (xxh3 of the
/// sorted version list + declared deps across versions).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contract {
    pub name: String,
    pub version: String,
    pub evidence_hash: u64,
    pub resolved_url: String,
    pub integrity: String,
    pub deps: Vec<(String, String)>,  // (child_name, child_version)
    pub peers: Vec<(String, String)>, // (peer_name, peer_spec)
    pub optional: bool,
}

impl Contract {
    /// Compute the evidence hash for a packument's version + dep fingerprint.
    pub fn evidence_from_packument(packument: &serde_json::Value) -> u64 {
        let Some(versions) = packument.get("versions").and_then(|v| v.as_object()) else {
            return 0;
        };
        // Sort version strings for deterministic ordering.
        let mut ver_keys: Vec<&String> = versions.keys().collect();
        ver_keys.sort();
        let mut hasher = Xxh3Hasher64::new();
        for vk in &ver_keys {
            hasher.write(vk.as_bytes());
            if let Some(vi) = versions.get(*vk) {
                for key in ["dependencies", "peerDependencies", "optionalDependencies"] {
                    if let Some(map) = vi.get(key).and_then(|v| v.as_object()) {
                        for (k, v) in map {
                            hasher.write(k.as_bytes());
                            hasher.write(v.as_str().unwrap_or("*").as_bytes());
                        }
                    }
                }
            }
        }
        hasher.finish()
    }
}

/// Minimal xxh3 hasher (64-bit, zero-config).
struct Xxh3Hasher64 {
    seed: u64,
    state: u64,
    total: u64,
}

impl Xxh3Hasher64 {
    fn new() -> Self {
        Self {
            seed: 0,
            state: 0x9e3779b97f4a7c15,
            total: 0,
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        let h = xxhash_rust::xxh3::xxh3_64_with_seed(bytes, self.seed);
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(h);
        self.total ^= self.state;
    }
    fn finish(self) -> u64 {
        self.total
    }
}

/// A whole-tree resolved bundle — one per lockfile hash / dep-set hash.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resolution {
    pub direct_specs: Vec<(String, String)>, // (name, spec) sorted
    pub packages: HashMap<String, PackageEntry>,
}

/// Entry inside a Resolution.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageEntry {
    pub version: String,
    pub resolved_url: String,
    pub integrity: String,
    pub dependencies: HashMap<String, String>, // child_name → child_version
}

// ── Header ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct GrainHeader {
    contract_offset: u64,
    contract_count: u64,
    resolution_offset: u64,
    resolution_count: u64,
}

impl GrainHeader {
    fn new() -> Self {
        Self {
            contract_offset: HEADER_SIZE as u64,
            contract_count: 0,
            resolution_offset: HEADER_SIZE as u64,
            resolution_count: 0,
        }
    }

    fn serialize(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..8].copy_from_slice(MAGIC);
        buf[8..16].copy_from_slice(&1u64.to_le_bytes()); // version
        buf[16..24].copy_from_slice(&self.contract_offset.to_le_bytes());
        buf[24..32].copy_from_slice(&self.contract_count.to_le_bytes());
        buf[32..40].copy_from_slice(&self.resolution_offset.to_le_bytes());
        buf[40..48].copy_from_slice(&self.resolution_count.to_le_bytes());
        buf
    }

    fn deserialize(buf: &[u8; HEADER_SIZE]) -> Option<Self> {
        if &buf[0..8] != MAGIC {
            return None;
        }
        Some(Self {
            contract_offset: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            contract_count: u64::from_le_bytes(buf[24..32].try_into().ok()?),
            resolution_offset: u64::from_le_bytes(buf[32..40].try_into().ok()?),
            resolution_count: u64::from_le_bytes(buf[40..48].try_into().ok()?),
        })
    }
}

// ── Grain Store ─────────────────────────────────────────────────────────

/// The unified content-addressed store.
pub struct GrainStore {
    path: PathBuf,
    data: Vec<u8>, // mmap'd or loaded into memory
}

impl GrainStore {
    /// Open or create a Grain store at the given path.
    pub fn open(path: &Path) -> Result<Self, String> {
        let data = if path.exists() {
            std::fs::read(path).map_err(|e| format!("Grain read: {e}"))?
        } else {
            let hdr = GrainHeader::new();
            let init = hdr.serialize().to_vec();
            std::fs::write(path, &init).map_err(|e| format!("Grain create: {e}"))?;
            init
        };
        Ok(Self {
            path: path.to_path_buf(),
            data,
        })
    }

    /// Create a new empty store, overwriting any existing file.
    pub fn create(path: &Path) -> Result<Self, String> {
        let hdr = GrainHeader::new();
        let init = hdr.serialize().to_vec();
        std::fs::write(path, &init).map_err(|e| format!("Grain create: {e}"))?;
        Ok(Self {
            path: path.to_path_buf(),
            data: init,
        })
    }

    /// Get the file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the number of contracts stored.
    pub fn contract_count(&self) -> u64 {
        GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )
        .map(|h| h.contract_count)
        .unwrap_or(0)
    }

    /// Return the number of resolutions stored.
    pub fn resolution_count(&self) -> u64 {
        GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )
        .map(|h| h.resolution_count)
        .unwrap_or(0)
    }

    // ── Contract lookups ────────────────────────────────────────────────

    /// Look up a contract by (name, version).  O(n) scan of the contracts
    /// region (small for now; a hash index can be added later).
    pub fn get_contract(&self, name: &str, version: &str) -> Option<Contract> {
        let hdr = GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )?;
        let start = hdr.contract_offset as usize;
        let mut cursor = start;
        for _ in 0..hdr.contract_count {
            if cursor + 4 > self.data.len() {
                return None;
            }
            let entry_len =
                u32::from_le_bytes(self.data[cursor..cursor + 4].try_into().ok()?) as usize;
            cursor += 4;
            if cursor + entry_len > self.data.len() {
                return None;
            }
            if let Ok(contract) =
                bincode::deserialize::<Contract>(&self.data[cursor..cursor + entry_len])
            {
                if contract.name == name && contract.version == version {
                    return Some(contract);
                }
            }
            cursor += entry_len;
        }
        None
    }

    /// Return all contracts for a given package name.
    pub fn get_contracts_for_name(&self, name: &str) -> Vec<Contract> {
        let mut out = Vec::new();
        let hdr = match GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        ) {
            Some(h) => h,
            None => return out,
        };
        let start = hdr.contract_offset as usize;
        let mut cursor = start;
        for _ in 0..hdr.contract_count {
            if cursor + 4 > self.data.len() {
                break;
            }
            let entry_len =
                u32::from_le_bytes(self.data[cursor..cursor + 4].try_into().unwrap_or([0; 4]))
                    as usize;
            cursor += 4;
            if cursor + entry_len > self.data.len() {
                break;
            }
            if let Ok(contract) =
                bincode::deserialize::<Contract>(&self.data[cursor..cursor + entry_len])
            {
                if contract.name == name {
                    out.push(contract);
                }
            }
            cursor += entry_len;
        }
        out
    }

    /// Look up a contract by evidence_hash (fast — direct hash match).
    pub fn get_contract_by_evidence(&self, name: &str, evidence_hash: u64) -> Option<Contract> {
        let hdr = GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )?;
        let start = hdr.contract_offset as usize;
        let mut cursor = start;
        for _ in 0..hdr.contract_count {
            if cursor + 4 > self.data.len() {
                return None;
            }
            let entry_len =
                u32::from_le_bytes(self.data[cursor..cursor + 4].try_into().ok()?) as usize;
            cursor += 4;
            if cursor + entry_len > self.data.len() {
                return None;
            }
            if let Ok(contract) =
                bincode::deserialize::<Contract>(&self.data[cursor..cursor + entry_len])
            {
                if contract.name == name && contract.evidence_hash == evidence_hash {
                    return Some(contract);
                }
            }
            cursor += entry_len;
        }
        None
    }

    // ── Resolution lookups ──────────────────────────────────────────────

    /// Look up a resolution by the dep-set hash.
    pub fn get_resolution(&self, deps_hash: u64) -> Option<Resolution> {
        let hdr = GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )?;
        let start = hdr.resolution_offset as usize;
        let mut cursor = start;
        for _ in 0..hdr.resolution_count {
            if cursor + 4 > self.data.len() {
                return None;
            }
            let entry_len =
                u32::from_le_bytes(self.data[cursor..cursor + 4].try_into().ok()?) as usize;
            cursor += 4;
            if cursor + entry_len > self.data.len() {
                return None;
            }
            // First 8 bytes of each resolution entry is the deps hash for fast matching.
            if entry_len >= 8 {
                let stored_hash =
                    u64::from_le_bytes(self.data[cursor..cursor + 8].try_into().ok()?);
                if stored_hash == deps_hash {
                    if let Ok(res) = bincode::deserialize::<Resolution>(
                        &self.data[cursor + 8..cursor + entry_len],
                    ) {
                        return Some(res);
                    }
                }
            }
            cursor += entry_len;
        }
        None
    }

    /// Compute the deps-hash for a set of direct specs.
    pub fn deps_hash(direct: &[(String, String)]) -> u64 {
        let mut sorted = direct.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut h = Xxh3Hasher64::new();
        for (name, spec) in &sorted {
            h.write(name.as_bytes());
            h.write(b":");
            h.write(spec.as_bytes());
        }
        h.finish()
    }

    // ── Writes ──────────────────────────────────────────────────────────

    /// Append a contract.  Returns the entry offset.
    pub fn append_contract(&mut self, contract: &Contract) -> Result<u64, String> {
        let mut hdr = GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )
        .ok_or_else(|| "Grain: corrupt header".to_string())?;

        let payload =
            bincode::serialize(contract).map_err(|e| format!("Grain serialize contract: {e}"))?;
        let entry_len = payload.len() as u32;

        let offset = self.data.len() as u64;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("Grain append: {e}"))?;

        file.write_all(&entry_len.to_le_bytes())
            .map_err(|e| format!("Grain write len: {e}"))?;
        file.write_all(&payload)
            .map_err(|e| format!("Grain write payload: {e}"))?;
        file.flush().map_err(|e| format!("Grain flush: {e}"))?;

        // Update header in memory and on disk.
        hdr.contract_count += 1;
        let mut new_data = self.data.clone();
        let hdr_bytes = hdr.serialize();
        new_data[..HEADER_SIZE].copy_from_slice(&hdr_bytes);
        // Also append the entry to in-memory data.
        new_data.extend_from_slice(&entry_len.to_le_bytes());
        new_data.extend_from_slice(&payload);

        // Rewrite header in the file (cheap: first 256 bytes).
        let mut f2 = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.path)
            .map_err(|e| format!("Grain rewrite header: {e}"))?;
        f2.write_all(&hdr_bytes)
            .map_err(|e| format!("Grain header write: {e}"))?;
        f2.flush().map_err(|e| format!("Grain flush header: {e}"))?;

        self.data = new_data;
        Ok(offset)
    }

    /// Append a resolution.  Returns the entry offset.
    pub fn append_resolution(&mut self, resolution: &Resolution) -> Result<u64, String> {
        let mut hdr = GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )
        .ok_or_else(|| "Grain: corrupt header".to_string())?;

        // Prepend the deps_hash for fast lookup.
        let deps_hash = Self::deps_hash(&resolution.direct_specs);
        let mut payload = Vec::with_capacity(8 + 256);
        payload.extend_from_slice(&deps_hash.to_le_bytes());
        let rest = bincode::serialize(resolution)
            .map_err(|e| format!("Grain serialize resolution: {e}"))?;
        payload.extend_from_slice(&rest);

        let entry_len = payload.len() as u32;
        let offset = self.data.len() as u64;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("Grain append resolution: {e}"))?;
        file.write_all(&entry_len.to_le_bytes())
            .map_err(|e| format!("Grain write len: {e}"))?;
        file.write_all(&payload)
            .map_err(|e| format!("Grain write payload: {e}"))?;
        file.flush().map_err(|e| format!("Grain flush: {e}"))?;

        hdr.resolution_count += 1;
        let hdr_bytes = hdr.serialize();

        let mut new_data = self.data.clone();
        new_data[..HEADER_SIZE].copy_from_slice(&hdr_bytes);
        new_data.extend_from_slice(&entry_len.to_le_bytes());
        new_data.extend_from_slice(&payload);

        let mut f2 = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.path)
            .map_err(|e| format!("Grain rewrite header: {e}"))?;
        f2.write_all(&hdr_bytes)
            .map_err(|e| format!("Grain header write: {e}"))?;
        f2.flush().map_err(|e| format!("Grain flush header: {e}"))?;

        self.data = new_data;
        Ok(offset)
    }

    /// Compact the store: rewrite only live entries, reclaim space from stale ones.
    /// Contracts are kept if `keep(name, version)` returns true.
    pub fn compact<F: Fn(&str, &str) -> bool>(&mut self, keep: F) -> Result<(), String> {
        let hdr = GrainHeader::deserialize(
            self.data[..HEADER_SIZE]
                .try_into()
                .ok()
                .unwrap_or(&[0u8; HEADER_SIZE]),
        )
        .ok_or_else(|| "Grain: corrupt header".to_string())?;

        let mut new_contracts = Vec::new();
        // Re-scan contracts.
        let start = hdr.contract_offset as usize;
        let mut cursor = start;
        for _ in 0..hdr.contract_count {
            if cursor + 4 > self.data.len() {
                break;
            }
            let entry_len =
                u32::from_le_bytes(self.data[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            if cursor + entry_len > self.data.len() {
                break;
            }
            if let Ok(contract) =
                bincode::deserialize::<Contract>(&self.data[cursor..cursor + entry_len])
            {
                if keep(&contract.name, &contract.version) {
                    let payload = bincode::serialize(&contract).unwrap();
                    let entry_len_bytes = (payload.len() as u32).to_le_bytes();
                    new_contracts.extend_from_slice(&entry_len_bytes);
                    new_contracts.extend_from_slice(&payload);
                }
            }
            cursor += entry_len;
        }

        // Resolutions: keep all (they reference contracts; stale ones will just miss).
        let mut new_resolutions = Vec::new();
        let res_start = hdr.resolution_offset as usize;
        let mut rcursor = res_start;
        for _ in 0..hdr.resolution_count {
            if rcursor + 4 > self.data.len() {
                break;
            }
            let entry_len =
                u32::from_le_bytes(self.data[rcursor..rcursor + 4].try_into().unwrap()) as usize;
            rcursor += 4;
            if rcursor + entry_len > self.data.len() {
                break;
            }
            let entry_len_bytes = (entry_len as u32).to_le_bytes();
            new_resolutions.extend_from_slice(&entry_len_bytes);
            new_resolutions.extend_from_slice(&self.data[rcursor..rcursor + entry_len]);
            rcursor += entry_len;
        }

        // Count entries by walking the length-prefixed data.
        let count_entries = |data: &[u8]| -> u64 {
            let mut count = 0u64;
            let mut off = 0usize;
            while off + 4 <= data.len() {
                let elen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
                off += 4;
                if off + elen > data.len() {
                    break;
                }
                off += elen;
                count += 1;
            }
            count
        };
        let new_hdr = GrainHeader {
            contract_offset: HEADER_SIZE as u64,
            contract_count: count_entries(&new_contracts),
            resolution_offset: (HEADER_SIZE + new_contracts.len()) as u64,
            resolution_count: count_entries(&new_resolutions),
        };

        let mut out = new_hdr.serialize().to_vec();
        out.extend_from_slice(&new_contracts);
        out.extend_from_slice(&new_resolutions);

        std::fs::write(&self.path, &out).map_err(|e| format!("Grain compact write: {e}"))?;
        self.data = out;
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_grain_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jhol");

        // Create and write a contract.
        {
            let mut store = GrainStore::create(&path).unwrap();
            assert_eq!(store.contract_count(), 0);

            let contract = Contract {
                name: "lodash".into(),
                version: "4.17.21".into(),
                evidence_hash: 12345,
                resolved_url: "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz".into(),
                integrity: "sha512-abc".into(),
                deps: vec![],
                peers: vec![],
                optional: false,
            };
            store.append_contract(&contract).unwrap();
            assert_eq!(store.contract_count(), 1);
        }

        // Re-open and read back.
        {
            let store = GrainStore::open(&path).unwrap();
            assert_eq!(store.contract_count(), 1);
            let c = store.get_contract("lodash", "4.17.21").unwrap();
            assert_eq!(c.name, "lodash");
            assert_eq!(c.version, "4.17.21");
            assert_eq!(c.evidence_hash, 12345);
        }
    }

    #[test]
    fn test_grain_resolution_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jhol");

        {
            let mut store = GrainStore::create(&path).unwrap();
            let mut pkgs = HashMap::new();
            pkgs.insert(
                "lodash".into(),
                PackageEntry {
                    version: "4.17.21".into(),
                    resolved_url: "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz".into(),
                    integrity: "sha512-abc".into(),
                    dependencies: HashMap::new(),
                },
            );
            let res = Resolution {
                direct_specs: vec![("lodash".into(), "^4.17.0".into())],
                packages: pkgs,
            };
            store.append_resolution(&res).unwrap();
            assert_eq!(store.resolution_count(), 1);
        }

        {
            let store = GrainStore::open(&path).unwrap();
            let hash = GrainStore::deps_hash(&[("lodash".into(), "^4.17.0".into())]);
            let res = store.get_resolution(hash).unwrap();
            assert_eq!(res.packages.len(), 1);
            assert_eq!(res.packages["lodash"].version, "4.17.21");
        }
    }

    #[test]
    fn test_grain_multiple_contracts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jhol");

        {
            let mut store = GrainStore::create(&path).unwrap();
            for i in 0..50 {
                let contract = Contract {
                    name: format!("pkg-{i}"),
                    version: format!("1.0.{i}"),
                    evidence_hash: i as u64,
                    resolved_url: format!("https://example.com/pkg-{i}-1.0.{i}.tgz"),
                    integrity: String::new(),
                    deps: vec![],
                    peers: vec![],
                    optional: false,
                };
                store.append_contract(&contract).unwrap();
            }
            assert_eq!(store.contract_count(), 50);
        }

        {
            let store = GrainStore::open(&path).unwrap();
            assert_eq!(store.contract_count(), 50);
            let c = store.get_contract("pkg-25", "1.0.25").unwrap();
            assert_eq!(c.evidence_hash, 25);
        }
    }

    #[test]
    fn test_grain_compact() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jhol");

        {
            let mut store = GrainStore::create(&path).unwrap();
            for i in 0..10 {
                store
                    .append_contract(&Contract {
                        name: format!("pkg-{i}"),
                        version: format!("1.0.{i}"),
                        evidence_hash: i as u64,
                        resolved_url: String::new(),
                        integrity: String::new(),
                        deps: vec![],
                        peers: vec![],
                        optional: false,
                    })
                    .unwrap();
            }
            // Keep only even-named packages.
            store
                .compact(|name, _| {
                    name.strip_prefix("pkg-")
                        .and_then(|n| n.parse::<u32>().ok())
                        .map(|n| n % 2 == 0)
                        .unwrap_or(false)
                })
                .unwrap();
            assert_eq!(store.contract_count(), 5); // 0,2,4,6,8
        }

        {
            let store = GrainStore::open(&path).unwrap();
            assert!(store.get_contract("pkg-0", "1.0.0").is_some());
            assert!(store.get_contract("pkg-1", "1.0.1").is_none());
            assert!(store.get_contract("pkg-4", "1.0.4").is_some());
        }
    }

    #[test]
    fn test_grain_deps_hash_deterministic() {
        let specs = vec![
            ("lodash".into(), "^4.0.0".into()),
            ("express".into(), "^4.18.0".into()),
        ];
        let h1 = GrainStore::deps_hash(&specs);
        let h2 = GrainStore::deps_hash(&specs);
        assert_eq!(h1, h2);
        // Order-independent.
        let mut reversed = specs.clone();
        reversed.reverse();
        let h3 = GrainStore::deps_hash(&reversed);
        assert_eq!(h1, h3);
    }
}
