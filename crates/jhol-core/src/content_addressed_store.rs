//! Content-Addressed Store (CAS) — global, cross-install, cross-machine.
//! Maps content hash → store path. Same content → same hash → reused.
//! Phase 7: zero-copy mmap, hardlink if same hash exists anywhere on disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

const CAS_DIR: &str = "cas";
const CAS_INDEX: &str = "cas_index.json";

#[derive(Debug, Default)]
pub struct CasStore {
    /// hash → path mapping
    index: HashMap<String, PathBuf>,
    /// cache directory (e.g. ~/.jhol)
    cache_dir: PathBuf,
}

static GLOBAL_CAS: OnceLock<Arc<RwLock<CasStore>>> = OnceLock::new();

pub fn global_cas() -> Arc<RwLock<CasStore>> {
    GLOBAL_CAS
        .get_or_init(|| -> Arc<RwLock<CasStore>> {
            let cache_dir = std::path::PathBuf::from(crate::utils::get_cache_dir());
            let store = CasStore::load(&cache_dir).unwrap_or_else(|_| CasStore {
                index: HashMap::new(),
                cache_dir,
            });
            Arc::new(RwLock::new(store))
        })
        .clone()
}

fn _dummy() {}

impl CasStore {
    fn cas_dir(&self) -> PathBuf {
        self.cache_dir.join(CAS_DIR)
    }

    fn index_path(&self) -> PathBuf {
        self.cache_dir.join(CAS_INDEX)
    }

    fn load(cache_dir: &PathBuf) -> Result<Self, String> {
        let ip = cache_dir.join(CAS_INDEX);
        if !ip.exists() {
            return Ok(Self {
                index: HashMap::new(),
                cache_dir: cache_dir.clone(),
            });
        }
        let raw = std::fs::read_to_string(&ip).map_err(|e| e.to_string())?;
        let map: HashMap<String, String> = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let index = map
            .into_iter()
            .map(|(h, p)| (h, PathBuf::from(p)))
            .collect();
        Ok(Self {
            index,
            cache_dir: cache_dir.clone(),
        })
    }

    /// Look up existing store path for a content hash. If exists on disk, return it.
    pub fn lookup(&self, hash: &str) -> Option<PathBuf> {
        self.index.get(hash).cloned().filter(|p| p.exists())
    }

    /// Register a content hash → store path mapping. Idempotent.
    pub fn register(&mut self, hash: &str, path: PathBuf) {
        self.index.insert(hash.to_string(), path);
    }

    /// Persist index to disk. Called at end of install.
    pub fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.cache_dir).map_err(|e| e.to_string())?;
        let map: HashMap<String, String> = self
            .index
            .iter()
            .map(|(h, p)| (h.clone(), p.to_string_lossy().to_string()))
            .collect();
        let json = serde_json::to_string(&map).map_err(|e| e.to_string())?;
        let ip = self.index_path();
        let tmp = ip.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &ip).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Hardlink source → dest if source exists. Returns true on success.
    /// Falls back to copy if hardlink fails (cross-device).
    pub fn link_or_copy(
        &self,
        src: &std::path::Path,
        dest: &std::path::Path,
    ) -> std::io::Result<()> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if dest.exists() {
            return Ok(());
        }
        if let Err(_) = std::fs::hard_link(src, dest) {
            std::fs::copy(src, dest)?;
        }
        Ok(())
    }
}

/// Compute xxh3 hash of a file. Returns hex string.
pub fn xxh3_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    use xxhash_rust::xxh3::Xxh3;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Xxh3::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:016x}", h.digest()))
}
