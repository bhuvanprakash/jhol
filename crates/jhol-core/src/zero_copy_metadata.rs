//! Zero-Copy Metadata Storage with bincode + memmap2
//!
//! Stores package metadata as memory-mapped archives for instant access
//! without deserialization on the hot path. Multiple projects share the
//! same mmap'd pages (OS does the cache).
//!
//! Unlike the original rkyv-based implementation, this uses bincode for
//! serialization + memmap2 for zero-copy reads. The first access still
//! deserializes (bincode), but subsequent reads hit kernel page cache.

use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Package metadata for zero-copy access (serialized via bincode).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<(String, String)>,
    pub optional_dependencies: Vec<(String, String)>,
    pub peer_dependencies: Vec<(String, String)>,
    pub dev_dependencies: Vec<(String, String)>,
    pub dist: DistInfo,
    pub bin: Option<Vec<String>>,
    pub scripts: Vec<(String, String)>,
    pub integrity: Option<String>,
    pub resolved_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DistInfo {
    pub tarball_url: String,
    pub shasum: String,
    pub integrity: Option<String>,
    pub file_count: Option<u32>,
    pub unpacked_size: Option<u64>,
}

/// Index entry: (name, offset_in_file, length_in_bytes)
type StoreIndex = Vec<(String, u64, u64)>;

/// Memory-mapped metadata store.
/// Data file: `<cache>/zero-copy/store.meta`
/// Index file: `<cache>/zero-copy/store.idx` (bincode)
pub struct MmapMetadataStore {
    store_path: PathBuf,
    mmap: Option<Mmap>,
    index: HashMap<String, (u64, u64)>,
}

fn default_store_path() -> PathBuf {
    let cache = crate::utils::get_cache_dir();
    let p = PathBuf::from(&cache).join("zero-copy").join("store.meta");
    if let Some(parent) = p.parent() {
        if fs::create_dir_all(parent).is_err() {
            let tmp = std::env::temp_dir()
                .join("jhol-zero-copy")
                .join("store.meta");
            if let Some(tmp_parent) = tmp.parent() {
                let _ = fs::create_dir_all(tmp_parent);
            }
            return tmp;
        }
    }
    p
}

impl MmapMetadataStore {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let actual_path = if let Some(parent) = store_path.parent() {
            if fs::create_dir_all(parent).is_err() {
                let tmp = std::env::temp_dir()
                    .join("jhol-zero-copy")
                    .join("store.meta");
                if let Some(tmp_parent) = tmp.parent() {
                    fs::create_dir_all(tmp_parent)
                        .map_err(|e| format!("Failed to create store dir: {}", e))?;
                }
                tmp
            } else {
                store_path
            }
        } else {
            store_path
        };
        let index = Self::load_index(&actual_path);
        let mmap = if actual_path.exists()
            && fs::metadata(&actual_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            let file =
                File::open(&actual_path).map_err(|e| format!("Failed to open store: {}", e))?;
            let mmap =
                unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to mmap store: {}", e))?;
            Some(mmap)
        } else {
            File::create(&actual_path).ok();
            None
        };
        Ok(Self {
            store_path: actual_path,
            mmap,
            index,
        })
    }

    pub fn global() -> parking_lot::MutexGuard<'static, MmapMetadataStore> {
        static INST: std::sync::OnceLock<parking_lot::Mutex<MmapMetadataStore>> =
            std::sync::OnceLock::new();
        INST.get_or_init(|| {
            parking_lot::Mutex::new(MmapMetadataStore::new(default_store_path()).unwrap_or_else(
                |_| MmapMetadataStore {
                    store_path: default_store_path(),
                    mmap: None,
                    index: HashMap::new(),
                },
            ))
        })
        .lock()
    }

    fn load_index(store_path: &PathBuf) -> HashMap<String, (u64, u64)> {
        let index_path = store_path.with_extension("idx");
        if index_path.exists() {
            if let Ok(data) = fs::read(&index_path) {
                if let Ok(index_vec) = bincode::deserialize::<StoreIndex>(&data) {
                    return index_vec
                        .into_iter()
                        .map(|(k, v1, v2)| (k, (v1, v2)))
                        .collect();
                }
            }
        }
        HashMap::new()
    }

    /// Insert package metadata into store (append-only).
    pub fn insert(&mut self, name: &str, metadata: &PackageMetadata) -> Result<(), String> {
        let serialized = bincode::serialize(metadata)
            .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
        if let Some(parent) = self.store_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut file = match OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.store_path)
        {
            Ok(f) => f,
            Err(_) => {
                let tmp = std::env::temp_dir()
                    .join("jhol-zero-copy")
                    .join("store.meta");
                if let Some(parent) = tmp.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                self.store_path = tmp;
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&self.store_path)
                    .map_err(|e| format!("Failed to open store: {}", e))?
            }
        };
        let offset = file.metadata().map(|m| m.len()).unwrap_or(0);
        file.write_all(&serialized)
            .map_err(|e| format!("Failed to write: {}", e))?;
        self.index
            .insert(name.to_string(), (offset, serialized.len() as u64));
        self.save_index()?;
        self.remap()?;
        Ok(())
    }

    /// Get raw bytes by name (zero-copy from mmap).
    pub fn get_raw(&self, name: &str) -> Option<Vec<u8>> {
        let (offset, length) = self.index.get(name)?;
        let mmap = self.mmap.as_ref()?;
        let start = *offset as usize;
        let end = start + (*length as usize);
        if end > mmap.len() {
            return None;
        }
        Some(mmap[start..end].to_vec())
    }

    /// Deserialize metadata (first access pays deserialize cost; subsequent
    /// accesses hit kernel page cache on the mmap'd file).
    pub fn get(&self, name: &str) -> Option<PackageMetadata> {
        let raw = self.get_raw(name)?;
        bincode::deserialize(&raw).ok()
    }

    /// Store packument metadata after fetching from registry.
    /// Called by registry after successful packument fetch.
    pub fn store_packument(
        name: &str,
        version: &str,
        deps: &[(String, String)],
        opt_deps: &[(String, String)],
        peer_deps: &[(String, String)],
        dev_deps: &[(String, String)],
        tarball_url: &str,
        integrity: Option<&str>,
        bin: Option<&[String]>,
        scripts: &[(String, String)],
    ) {
        Self::store_packument_for_key(
            name,
            name,
            version,
            deps,
            opt_deps,
            peer_deps,
            dev_deps,
            tarball_url,
            integrity,
            bin,
            scripts,
        );
    }

    pub fn store_packument_for_key(
        key: &str,
        name: &str,
        version: &str,
        deps: &[(String, String)],
        opt_deps: &[(String, String)],
        peer_deps: &[(String, String)],
        dev_deps: &[(String, String)],
        tarball_url: &str,
        integrity: Option<&str>,
        bin: Option<&[String]>,
        scripts: &[(String, String)],
    ) {
        let mut store = Self::global();
        let meta = PackageMetadata {
            name: name.to_string(),
            version: version.to_string(),
            dependencies: deps.to_vec(),
            optional_dependencies: opt_deps.to_vec(),
            peer_dependencies: peer_deps.to_vec(),
            dev_dependencies: dev_deps.to_vec(),
            dist: DistInfo {
                tarball_url: tarball_url.to_string(),
                shasum: String::new(),
                integrity: integrity.map(|s| s.to_string()),
                file_count: None,
                unpacked_size: None,
            },
            bin: bin.map(|b| b.to_vec()),
            scripts: scripts.to_vec(),
            integrity: integrity.map(|s| s.to_string()),
            resolved_url: Some(tarball_url.to_string()),
        };
        if let Err(e) = store.insert(key, &meta) {
            eprintln!("store_packument insert error: {}", e);
        }
    }

    /// Quick lookup: get cached packument deps from mmap.
    pub fn lookup_packument(name: &str) -> Option<PackageMetadata> {
        let store = Self::global();
        store.get(name)
    }

    pub fn lookup_packument_for_key(key: &str) -> Option<PackageMetadata> {
        let store = Self::global();
        store.get(key)
    }

    pub fn clear(&mut self) -> Result<(), String> {
        File::create(&self.store_path).map_err(|e| format!("Failed to truncate store: {}", e))?;
        self.index.clear();
        self.mmap = None;
        let index_path = self.store_path.with_extension("idx");
        fs::remove_file(index_path).ok();
        Ok(())
    }

    pub fn stats(&self) -> MetadataStoreStats {
        let file_size = fs::metadata(&self.store_path).map(|m| m.len()).unwrap_or(0);
        MetadataStoreStats {
            package_count: self.index.len(),
            total_size_bytes: file_size,
            is_mmap_active: self.mmap.is_some(),
        }
    }

    fn save_index(&self) -> Result<(), String> {
        let index_path = self.store_path.with_extension("idx");
        let index_vec: StoreIndex = self
            .index
            .iter()
            .map(|(k, (o, l))| (k.clone(), *o, *l))
            .collect();
        let serialized = bincode::serialize(&index_vec)
            .map_err(|e| format!("Failed to serialize index: {}", e))?;
        fs::write(&index_path, &serialized).map_err(|e| format!("Failed to write index: {}", e))?;
        Ok(())
    }

    fn remap(&mut self) -> Result<(), String> {
        self.mmap = None;
        let file =
            File::open(&self.store_path).map_err(|e| format!("Failed to open store: {}", e))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("Failed to stat store: {}", e))?;
        if metadata.len() > 0 {
            let mmap =
                unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to mmap store: {}", e))?;
            self.mmap = Some(mmap);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MetadataStoreStats {
    pub package_count: usize,
    pub total_size_bytes: u64,
    pub is_mmap_active: bool,
}

/// Cache packument metadata from registry JSON in zero-copy store.
pub fn cache_packument_metadata(name: &str, version: &str, data: &serde_json::Value) {
    cache_packument_metadata_for_key(name, name, version, data);
}

pub fn cache_packument_metadata_for_key(
    key: &str,
    name: &str,
    version: &str,
    data: &serde_json::Value,
) {
    let mut deps = Vec::new();
    if let Some(d) = data.get("dependencies").and_then(|v| v.as_object()) {
        for (k, v) in d {
            deps.push((k.clone(), v.as_str().unwrap_or("*").to_string()));
        }
    }
    let mut opt_deps = Vec::new();
    if let Some(d) = data.get("optionalDependencies").and_then(|v| v.as_object()) {
        for (k, v) in d {
            opt_deps.push((k.clone(), v.as_str().unwrap_or("*").to_string()));
        }
    }
    let mut peer_deps = Vec::new();
    if let Some(d) = data.get("peerDependencies").and_then(|v| v.as_object()) {
        for (k, v) in d {
            peer_deps.push((k.clone(), v.as_str().unwrap_or("*").to_string()));
        }
    }
    let mut dev_deps = Vec::new();
    if let Some(d) = data.get("devDependencies").and_then(|v| v.as_object()) {
        for (k, v) in d {
            dev_deps.push((k.clone(), v.as_str().unwrap_or("*").to_string()));
        }
    }
    let tarball_url = data
        .get("dist")
        .and_then(|d| d.get("tarball").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_string();
    let integrity = data
        .get("dist")
        .and_then(|d| d.get("integrity").and_then(|i| i.as_str()));
    let bin: Option<Vec<String>> = data.get("bin").map(|b| {
        if let Some(obj) = b.as_object() {
            obj.values()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        } else if let Some(s) = b.as_str() {
            vec![s.to_string()]
        } else {
            Vec::new()
        }
    });
    let mut scripts = Vec::new();
    if let Some(s) = data.get("scripts").and_then(|v| v.as_object()) {
        for (k, v) in s {
            scripts.push((k.clone(), v.as_str().unwrap_or("").to_string()));
        }
    }
    MmapMetadataStore::store_packument_for_key(
        key,
        name,
        version,
        &deps,
        &opt_deps,
        &peer_deps,
        &dev_deps,
        &tarball_url,
        integrity,
        bin.as_deref(),
        &scripts,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bincode_mmap_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store_path = temp_dir.path().join("metadata.store");
        let mut store = MmapMetadataStore::new(store_path.clone()).unwrap();
        let metadata = PackageMetadata {
            name: "test-pkg".to_string(),
            version: "1.0.0".to_string(),
            dependencies: vec![("lodash".to_string(), "^4.0.0".to_string())],
            optional_dependencies: Vec::new(),
            peer_dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            dist: DistInfo {
                tarball_url: "https://example.com/pkg.tgz".to_string(),
                shasum: "abc123".to_string(),
                integrity: Some("sha512-xyz".to_string()),
                file_count: Some(10),
                unpacked_size: Some(1024),
            },
            bin: None,
            scripts: vec![("test".to_string(), "echo hi".to_string())],
            integrity: None,
            resolved_url: None,
        };
        store.insert("test-pkg", &metadata).unwrap();
        let data = store.get_raw("test-pkg").unwrap();
        assert!(!data.is_empty());
        let stats = store.stats();
        assert_eq!(stats.package_count, 1);
        assert!(stats.is_mmap_active);
        let retrieved = store.get("test-pkg").unwrap();
        assert_eq!(retrieved.name, "test-pkg");
        assert_eq!(retrieved.version, "1.0.0");
        assert_eq!(retrieved.dependencies[0].0, "lodash");
    }

    #[test]
    fn test_cache_packument() {
        let json: serde_json::Value = serde_json::from_str(r#"{
            "dependencies": {"left-pad": "^1.0.0"},
            "optionalDependencies": {"fsevents": "*"},
            "peerDependencies": {"react": "^18.0.0"},
            "devDependencies": {"typescript": "^5.0.0"},
            "dist": {"tarball": "https://registry.npmjs.org/pkg/-/pkg-1.0.0.tgz", "integrity": "sha512-abc"},
            "bin": {"pkg": "./bin/pkg.js"},
            "scripts": {"postinstall": "node setup.js"}
        }"#).unwrap();
        cache_packument_metadata("test-pkg", "1.0.0", &json);
        let retrieved = MmapMetadataStore::lookup_packument("test-pkg").unwrap();
        assert_eq!(retrieved.dependencies[0].0, "left-pad");
        assert_eq!(retrieved.optional_dependencies[0].0, "fsevents");
        assert_eq!(retrieved.peer_dependencies[0].0, "react");
        assert_eq!(retrieved.dev_dependencies[0].0, "typescript");
        assert_eq!(
            retrieved.dist.tarball_url,
            "https://registry.npmjs.org/pkg/-/pkg-1.0.0.tgz"
        );
    }

    #[test]
    fn identity_packument_entries_do_not_cross_registry_keys() {
        let data: serde_json::Value = serde_json::from_str(
            r#"{"dependencies":{"left-pad":"^1.0.0"},"dist":{"tarball":"https://registry.example/pkg.tgz","integrity":"sha512-abc"}}"#,
        )
        .unwrap();
        let key_a = "metadata-v2:registry-a-private-package";
        let key_b = "metadata-v2:registry-b-private-package";
        cache_packument_metadata_for_key(key_a, "@private/pkg", "1.0.0", &data);

        let hit = MmapMetadataStore::lookup_packument_for_key(key_a).unwrap();
        assert_eq!(hit.name, "@private/pkg");
        assert!(MmapMetadataStore::lookup_packument_for_key(key_b).is_none());
    }
}

#[cfg(test)]
mod zero_copy_live_proof {
    use super::*;

    #[test]
    fn store_packument_then_lookup_roundtrips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("zero-copy").join("store.meta");
        // Mimic the write path used by registry's fetch (store_packument into
        // the GLOBAL store) but against a known path, then read it back fresh.
        let mut store = MmapMetadataStore::new(store_path.clone()).expect("new");
        let meta = PackageMetadata {
            name: "left-pad".to_string(),
            version: "1.3.0".to_string(),
            dependencies: vec![],
            optional_dependencies: vec![],
            peer_dependencies: vec![],
            dev_dependencies: vec![],
            dist: DistInfo {
                tarball_url: "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz".to_string(),
                shasum: String::new(),
                integrity: Some("sha512-x".to_string()),
                file_count: None,
                unpacked_size: None,
            },
            bin: None,
            scripts: vec![],
            integrity: Some("sha512-x".to_string()),
            resolved_url: None,
        };
        store.insert("left-pad", &meta).expect("insert");
        store.get("left-pad").expect("read-back");
        drop(store);
        // Fresh process view: reload from disk (the .idx + .meta pair).
        let reloaded = MmapMetadataStore::new(store_path.clone()).expect("reload");
        let found = reloaded.get("left-pad");
        assert!(
            found.is_some(),
            "store must survive reload — write-only otherwise"
        );
        assert_eq!(found.unwrap().version, "1.3.0");
        assert!(store_path.exists(), "store.meta must exist on disk");
    }
}
