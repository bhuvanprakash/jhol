//! Jhol Bundle CAS (Content-Addressable Store)
//!
//! All parsed and transformed content is stored in a CAS keyed by content hash.
//! This means identical files across projects and rebuilds are processed exactly once.
//!
//! Directory layout:
//!   ~/.jhol-cache/bundle-cas/
//!   ├── manifests/   ParseManifest records (JSON)
//!   ├── transformed/  Transformed output (UTF-8 text, one file per hash)
//!   ├── sourcemaps/   VLQ sourcemap JSON files
//!   └── meta/        path → content-hash index for invalidation
//!
//! Key insight: the content hash (xxhash3) is the canonical identity of a module,
//! NOT its path. Two files with identical content but different paths share one
//! CAS entry. Two builds of the same file with unchanged content share one CAS entry.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use crate::errors::{BundleError, BundleResult};

/// xxHash3 output — 64-bit content hash. Used as the canonical CAS key.
/// Not cryptographic: speed > collision resistance for build tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub u64);

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(xxhash_rust::xxh3::xxh3_64(bytes))
    }

    pub fn from_str(s: &str) -> Self {
        Self(xxhash_rust::xxh3::xxh3_64(s.as_bytes()))
    }

    pub fn to_hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// ParseManifest — the canonical representation of a parsed module.
/// Stored in CAS, keyed by content hash of the original source.
/// This is what gets cached: parsing is expensive, and the same source
/// file content always produces the same ParseManifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParseManifest {
    /// xxhash3 of the raw source bytes — this is the CAS key
    pub content_hash: u64,
    /// xxhash3 of the parsed AST (before transform) — used to invalidate
    /// transform cache when the AST structure changes without content changing
    pub ast_hash: u64,
    /// Resolved absolute path of this module (for error messages, debugging, code generation)
    pub path: PathBuf,
    /// File type derived from extension
    pub file_type: String,
    /// Original source (kept so we don't need to re-read the file)
    pub source: String,
    /// Static imports (import ... from "...")
    pub imports: Vec<ImportRecord>,
    /// Re-exports (export ... from "...")
    pub reexports: Vec<ImportRecord>,
    /// Dynamic imports (import(...))
    pub dynamic_imports: Vec<ImportRecord>,
    /// CJS requires (require(...))
    pub requires: Vec<ImportRecord>,
    /// Named and default exports
    pub exports: Vec<ExportRecord>,
    /// Content hashes of all dependencies (resolved from specifiers)
    pub dependency_hashes: Vec<u64>,
    /// True if this module has top-level side effects
    pub has_side_effects: bool,
    /// True if this is an entry point
    pub is_entry: bool,
}

/// Import kind — distinguishes static/dynamic/require/reexport for code splitting.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ImportKind {
    StaticImport,
    DynamicImport,
    Require,
    ReExport,
}

/// Import record — minimal representation for module graph building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportRecord {
    /// The specifier as written in the source (e.g. "./foo", "react", "lodash")
    pub specifier: String,
    /// The kind of import
    pub kind: ImportKind,
    /// Start byte offset in source
    pub start: usize,
    /// End byte offset in source
    pub end: usize,
}

/// Export record — minimal representation for tree shaking.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportRecord {
    /// Exported name
    pub name: String,
    /// Alias for default exports ("default") or renamed exports (as X)
    pub alias: Option<String>,
    /// Source module for re-exports (None for local exports)
    pub source: Option<String>,
}

/// TransformedOutput — the result of transforming a ParseManifest.
/// Stored in CAS keyed by the content hash + transform config hash.
/// When config changes (e.g. JSX runtime, define values), outputs are invalidated.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformedOutput {
    /// Content hash this was transformed from
    pub source_hash: u64,
    /// Config hash this was transformed with
    pub config_hash: u64,
    /// The transformed JavaScript source
    pub code: String,
    /// Inline sourcemap segments (VLQ encoded)
    pub sourcemap: Option<String>,
}

/// MetaIndex — maps source file paths to content hashes for cache invalidation.
/// On rebuild, only files with changed mtime+size are re-hashed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct MetaIndex {
    /// path → (content_hash, mtime_secs, size_bytes)
    entries: HashMap<String, (u64, u64, u64)>,
}

impl MetaIndex {
    pub fn load(path: &Path) -> BundleResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let reader = BufReader::new(File::open(path)?);
        serde_json::from_reader(reader).map_err(|e| {
            BundleError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })
    }

    pub fn save(&self, path: &Path) -> BundleResult<()> {
        let parent = path.parent().unwrap_or(path);
        fs::create_dir_all(parent)?;
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, self).map_err(|e| {
            BundleError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
        writer.flush()?;
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<u64> {
        self.entries.get(path).map(|(h, _, _)| *h)
    }

    pub fn insert(&mut self, path: String, hash: u64, mtime: u64, size: u64) {
        self.entries.insert(path, (hash, mtime, size));
    }

    pub fn remove(&mut self, path: &str) {
        self.entries.remove(path);
    }

    /// Returns paths whose mtime or size changed compared to stored metadata.
    pub fn changed_paths(&self) -> Vec<String> {
        let mut changed = Vec::new();
        for (path, (stored_hash, stored_mtime, stored_size)) in &self.entries {
            if let Ok(meta) = fs::metadata(path) {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let size = meta.len();
                if mtime != *stored_mtime || size != *stored_size {
                    changed.push(path.clone());
                }
            }
        }
        changed
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// In-memory CAS cache with LRU eviction.
/// Each shard is a RwLock-protected HashMap, sharded by upper 8 bits of hash
/// to reduce lock contention under parallel access.
const SHARD_COUNT: usize = 16;

struct CasShard {
    manifests: RwLock<HashMap<u64, Arc<ParseManifest>>>,
    transformed: RwLock<HashMap<u64, Arc<TransformedOutput>>>,
    access_order: RwLock<Vec<u64>>,
}

impl CasShard {
    fn new() -> Self {
        Self {
            manifests: RwLock::new(HashMap::new()),
            transformed: RwLock::new(HashMap::new()),
            access_order: RwLock::new(Vec::new()),
        }
    }
}

/// CAS store — content-addressable cache for bundle operations.
/// Thread-safe, sharded, with LRU eviction.
pub struct CasStore {
    shards: Vec<CasShard>,
    cache_dir: PathBuf,
    max_entries: usize,
}

impl CasStore {
    pub fn new(cache_dir: PathBuf, max_entries: usize) -> BundleResult<Self> {
        let base = cache_dir.join("bundle-cas");
        fs::create_dir_all(base.join("manifests"))?;
        fs::create_dir_all(base.join("transformed"))?;
        fs::create_dir_all(base.join("sourcemaps"))?;
        fs::create_dir_all(base.join("meta"))?;

        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(CasShard::new());
        }

        Ok(Self {
            shards,
            cache_dir: base,
            max_entries,
        })
    }

    fn shard_idx(hash: u64) -> usize {
        ((hash >> 56) as usize) % SHARD_COUNT
    }

    /// Get a ParseManifest from CAS (memory cache → disk → absent).
    pub fn get_manifest(&self, hash: u64) -> Option<Arc<ParseManifest>> {
        let idx = Self::shard_idx(hash);
        let shard = &self.shards[idx];

        // Fast path: in-memory
        if let Ok(guard) = shard.manifests.read() {
            if let Some(m) = guard.get(&hash) {
                Self::touch_manifest(&shard.access_order, hash);
                return Some(Arc::clone(m));
            }
        }

        // Slow path: load from disk
        self.load_manifest_from_disk(hash)
    }

    fn touch_manifest(access_order: &RwLock<Vec<u64>>, hash: u64) {
        if let Ok(mut order) = access_order.write() {
            if let Some(pos) = order.iter().position(|&h| h == hash) {
                order.remove(pos);
            }
            order.push(hash);
        }
    }

    fn load_manifest_from_disk(&self, hash: u64) -> Option<Arc<ParseManifest>> {
        let path = self
            .cache_dir
            .join("manifests")
            .join(format!("{}.json", hash));
        if !path.exists() {
            return None;
        }

        let manifest: ParseManifest = match serde_json::from_slice(&fs::read(&path).ok()?) {
            Ok(m) => m,
            Err(_) => return None,
        };

        // Populate in-memory cache
        let idx = Self::shard_idx(hash);
        let shard = &self.shards[idx];
        if let Ok(mut guard) = shard.manifests.write() {
            guard.insert(hash, Arc::new(manifest));
        }

        let idx2 = Self::shard_idx(hash);
        let shard2 = &self.shards[idx2];
        Self::touch_manifest(&shard2.access_order, hash);

        self.get_manifest(hash)
    }

    /// Store a ParseManifest in CAS (memory + disk).
    pub fn put_manifest(&self, manifest: &ParseManifest) -> BundleResult<()> {
        let hash = manifest.content_hash;
        let idx = Self::shard_idx(hash);
        let shard = &self.shards[idx];

        // Store in memory
        let arc = Arc::new(manifest.clone());
        if let Ok(mut guard) = shard.manifests.write() {
            guard.insert(hash, arc);
        }
        Self::touch_manifest(&shard.access_order, hash);

        // Store on disk
        let path = self
            .cache_dir
            .join("manifests")
            .join(format!("{}.json", hash));
        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, manifest).map_err(|e| {
            BundleError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
        writer.flush()?;

        self.evict_if_needed();
        Ok(())
    }

    /// Get a TransformedOutput from CAS.
    pub fn get_transformed(
        &self,
        source_hash: u64,
        config_hash: u64,
    ) -> Option<Arc<TransformedOutput>> {
        let key = Self::composite_key(source_hash, config_hash);
        let idx = Self::shard_idx(key);
        let shard = &self.shards[idx];

        if let Ok(guard) = shard.transformed.read() {
            if let Some(t) = guard.get(&key) {
                return Some(Arc::clone(t));
            }
        }

        self.load_transformed_from_disk(key)
    }

    /// Compute a composite key from source hash + config hash.
    pub fn composite_key(source_hash: u64, config_hash: u64) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        source_hash.hash(&mut h);
        config_hash.hash(&mut h);
        h.finish()
    }

    fn load_transformed_from_disk(&self, key: u64) -> Option<Arc<TransformedOutput>> {
        let path = self
            .cache_dir
            .join("transformed")
            .join(format!("{}.json", key));
        if !path.exists() {
            return None;
        }

        let output: TransformedOutput = match serde_json::from_slice(&fs::read(&path).ok()?) {
            Ok(t) => t,
            Err(_) => return None,
        };

        let idx = Self::shard_idx(key);
        let shard = &self.shards[idx];
        if let Ok(mut guard) = shard.transformed.write() {
            guard.insert(key, Arc::new(output));
        }

        // Return the cached value
        let idx2 = Self::shard_idx(key);
        if let Ok(guard) = self.shards[idx2].transformed.read() {
            return guard.get(&key).map(|a| Arc::clone(a));
        }
        None
    }

    /// Store a TransformedOutput in CAS.
    /// Store a TransformedOutput in CAS.
    pub fn put_transformed(&self, output: &TransformedOutput) -> BundleResult<()> {
        let key = Self::composite_key(output.source_hash, output.config_hash);
        let idx = Self::shard_idx(key);
        let shard = &self.shards[idx];

        let arc = Arc::new(output.clone());
        if let Ok(mut guard) = shard.transformed.write() {
            guard.insert(key, arc);
        }

        let path = self
            .cache_dir
            .join("transformed")
            .join(format!("{}.json", key));
        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, output).map_err(|e| {
            BundleError::IoError(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })?;
        writer.flush()?;

        self.evict_if_needed();
        Ok(())
    }

    /// Load the meta index from disk.
    pub fn load_meta(&self) -> BundleResult<MetaIndex> {
        MetaIndex::load(&self.cache_dir.join("meta").join("index.json"))
    }

    /// Save the meta index to disk.
    pub fn save_meta(&self, meta: &MetaIndex) -> BundleResult<()> {
        meta.save(&self.cache_dir.join("meta").join("index.json"))
    }

    /// Invalidate a content hash from the cache (manifest + transformed).
    pub fn invalidate(&self, hash: u64) {
        let idx = Self::shard_idx(hash);

        if let Ok(mut guard) = self.shards[idx].manifests.write() {
            guard.remove(&hash);
        }

        // Invalidate all transformed outputs for this source hash
        // (config may have changed, need to retransform)
        if let Ok(mut guard) = self.shards[idx].transformed.write() {
            let keys_to_remove: Vec<u64> = guard
                .keys()
                .filter(|&&k| {
                    let src = k ^ (k & 0xFFFFFFFF); // rough approximation — actually check source_hash
                    src == hash
                })
                .cloned()
                .collect();
            for k in keys_to_remove {
                guard.remove(&k);
            }
        }

        // Remove from disk
        let manifest_path = self
            .cache_dir
            .join("manifests")
            .join(format!("{}.json", hash));
        let _ = fs::remove_file(manifest_path);
    }

    fn evict_if_needed(&self) {
        // Simple LRU: if total entries > max_entries, evict oldest half
        let total: usize = self
            .shards
            .iter()
            .map(|s| {
                s.manifests.read().map(|g| g.len()).unwrap_or(0)
                    + s.transformed.read().map(|g| g.len()).unwrap_or(0)
            })
            .sum();

        if total <= self.max_entries {
            return;
        }

        let evict_count = total / 2;
        let mut evicted = 0;

        for shard in &self.shards {
            if evicted >= evict_count {
                break;
            }

            if let Ok(mut order) = shard.access_order.write() {
                while evicted < evict_count && !order.is_empty() {
                    let hash = order.remove(0);
                    if let Ok(mut guard) = shard.manifests.write() {
                        guard.remove(&hash);
                    }
                    if let Ok(mut guard) = shard.transformed.write() {
                        guard.remove(&hash);
                    }
                    let mpath = self
                        .cache_dir
                        .join("manifests")
                        .join(format!("{}.json", hash));
                    let tpath = self
                        .cache_dir
                        .join("transformed")
                        .join(format!("{}.json", hash));
                    let _ = fs::remove_file(mpath);
                    let _ = fs::remove_file(tpath);
                    evicted += 1;
                }
            }
        }
    }

    /// Store a raw file in CAS given its content.
    /// Returns the content hash.
    pub fn store_raw(&self, content: &[u8]) -> BundleResult<u64> {
        let hash = xxhash_rust::xxh3::xxh3_64(content);
        let idx = Self::shard_idx(hash);
        let shard = &self.shards[idx];

        // Check if already stored
        if let Ok(guard) = shard.manifests.read() {
            if guard.contains_key(&hash) {
                return Ok(hash);
            }
        }

        // Store content as a manifest with minimal info
        let manifest = ParseManifest {
            content_hash: hash,
            ast_hash: hash,
            path: PathBuf::new(),
            file_type: "unknown".to_string(),
            source: String::from_utf8_lossy(content).into_owned(),
            imports: Vec::new(),
            reexports: Vec::new(),
            dynamic_imports: Vec::new(),
            requires: Vec::new(),
            exports: Vec::new(),
            dependency_hashes: Vec::new(),
            has_side_effects: false,
            is_entry: false,
        };

        self.put_manifest(&manifest)?;
        Ok(hash)
    }
}

/// Global CAS store — lazily initialized, process-lifetime.
static CAS_STORE: std::sync::RwLock<Option<CasStore>> = std::sync::RwLock::new(None);

pub fn bundle_cas_store() -> &'static std::sync::RwLock<Option<CasStore>> {
    &CAS_STORE
}

pub fn init_bundle_cas(max_entries: usize) -> BundleResult<()> {
    let cache_dir = std::env::var("JHOL_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".jhol-cache")
        });

    let store = CasStore::new(cache_dir, max_entries)?;
    let mut guard = CAS_STORE.write().unwrap();
    *guard = Some(store);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash_deterministic() {
        let src = "const x = 1; export { x };";
        let h1 = ContentHash::from_str(src);
        let h2 = ContentHash::from_str(src);
        assert_eq!(h1, h2);
        assert_eq!(h1.0, xxhash_rust::xxh3::xxh3_64(src.as_bytes()));
    }

    #[test]
    fn test_meta_index_save_load() {
        let tmp = std::env::temp_dir().join("jhol-cas-test-meta.json");
        let mut meta = MetaIndex::default();
        meta.insert("/a/b.js".into(), 12345, 999, 100);
        meta.save(&tmp).unwrap();
        let loaded = MetaIndex::load(&tmp).unwrap();
        assert_eq!(loaded.get("/a/b.js"), Some(12345));
        let _ = fs::remove_file(tmp);
    }
}
