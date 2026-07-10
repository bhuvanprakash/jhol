use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::errors::{RuntimeError, RuntimeResult};

const PACKAGE_JSON: &str = "package.json";
const NODE_MODULES: &str = "node_modules";

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub path: PathBuf,
    pub source: Arc<str>,
    pub format: ModuleFormat,
    pub is_external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleFormat {
    ESM,
    CJS,
    JSON,
    Wasm,
    Native,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistentEntry {
    key: u64,
    path: PathBuf,
    format: ModuleFormat,
    is_external: bool,
    mtime_secs: u64,
    mtime_nanos: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistentCacheFile {
    version: u32,
    entries: Vec<PersistentEntry>,
}

const CACHE_VERSION: u32 = 1;
const CACHE_FILE_NAME: &str = "module-resolver-cache-v2.bin";

/// Extension order by likelihood — .ts, .jsx before .json, .node
const EXT_ORDER: &[&str] = &[
    "", ".ts", ".tsx", ".jsx", ".js", ".mjs", ".cjs", ".json", ".node",
];
const INDEX_EXTS: &[&str] = &[
    "index.ts",
    "index.tsx",
    "index.jsx",
    "index.js",
    "index.mjs",
    "index.cjs",
    "index.json",
];

fn cache_file_path() -> PathBuf {
    let base = dirs::cache_dir()
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("jhol");
    let _ = fs::create_dir_all(&base);
    base.join(CACHE_FILE_NAME)
}

fn cache_key(specifier: &str, base: &Path) -> u64 {
    let canonical = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let mut input = Vec::with_capacity(specifier.len() + canonical.as_os_str().len() + 1);
    input.extend_from_slice(specifier.as_bytes());
    input.push(0);
    input.extend_from_slice(canonical.as_os_str().as_encoded_bytes());
    xxh3_64(&input)
}

fn load_persistent_cache() -> HashMap<u64, PersistentEntry> {
    let path = cache_file_path();
    if !path.exists() {
        return HashMap::new();
    }
    match fs::read(&path) {
        Ok(bytes) => match bincode::deserialize::<PersistentCacheFile>(&bytes) {
            Ok(file) if file.version == CACHE_VERSION => {
                file.entries.into_iter().map(|e| (e.key, e)).collect()
            }
            _ => HashMap::new(),
        },
        Err(_) => HashMap::new(),
    }
}

fn save_persistent_cache(entries: &HashMap<u64, PersistentEntry>) {
    let path = cache_file_path();
    let file = PersistentCacheFile {
        version: CACHE_VERSION,
        entries: entries.values().cloned().collect(),
    };
    if let Ok(bytes) = bincode::serialize(&file) {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, &bytes).is_ok() {
            // On Windows rename fails if target exists; remove it first.
            #[cfg(windows)]
            {
                let _ = fs::remove_file(&path);
            }
            let _ = fs::rename(&tmp, &path);
        }
    }
}

#[derive(Clone)]
pub struct ModuleResolver {
    memory: Arc<DashMap<u64, Arc<ResolvedModule>>>,
    persistent: Arc<RwLock<HashMap<u64, PersistentEntry>>>,
    global_paths: Vec<PathBuf>,
    dirty: Arc<AtomicBool>,
}

impl ModuleResolver {
    pub fn new() -> Self {
        let mut global_paths = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            let global = PathBuf::from(home)
                .join(".jhol")
                .join("global")
                .join(NODE_MODULES);
            if global.exists() {
                global_paths.push(global);
            }
        }
        let persistent = load_persistent_cache();
        Self {
            memory: Arc::new(DashMap::new()),
            persistent: Arc::new(RwLock::new(persistent)),
            global_paths,
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    fn try_path_with_extensions(base: &Path) -> Option<PathBuf> {
        let p_str = base.to_string_lossy().to_string();
        for ext in EXT_ORDER {
            let with_ext = if ext.is_empty() {
                p_str.clone()
            } else {
                format!("{}{}", p_str, ext)
            };
            let candidate = PathBuf::from(&with_ext);
            if candidate.exists() && candidate.is_file() {
                return Some(candidate);
            }
        }
        if base.is_dir() {
            for idx in INDEX_EXTS {
                let candidate = base.join(idx);
                if candidate.exists() && candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    fn resolve_package_internal(pkg_dir: &Path, subpath: &str) -> RuntimeResult<Option<PathBuf>> {
        let pkg_json_path = pkg_dir.join(PACKAGE_JSON);
        if !pkg_json_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&pkg_json_path).map_err(RuntimeError::IoError)?;
        let pkg: serde_json::Value = serde_json::from_str(&content).map_err(RuntimeError::JsonError)?;

        if let Some(exports) = pkg.get("exports") {
            let target = if subpath.is_empty() {
                if let Some(default) = exports.get(".") {
                    default
                } else if let Some(main) = exports.get("default") {
                    main
                } else if let Some(import) = exports.get("import") {
                    import
                } else if let Some(require) = exports.get("require") {
                    require
                } else {
                    exports
                }
            } else {
                let export_key = format!("./{}", subpath);
                if let Some(entry) = exports.get(&export_key) {
                    entry
                } else {
                    return Ok(None);
                }
            };

            match target {
                serde_json::Value::String(s) => {
                    let target_path = pkg_dir.join(s.trim_start_matches("./"));
                    return Ok(Self::try_path_with_extensions(&target_path));
                }
                serde_json::Value::Object(map) => {
                    let target_str = map
                        .get("default")
                        .or_else(|| map.get("import"))
                        .or_else(|| map.get("require"))
                        .and_then(|v| v.as_str());
                    if let Some(s) = target_str {
                        let target_path = pkg_dir.join(s.trim_start_matches("./"));
                        return Ok(Self::try_path_with_extensions(&target_path));
                    }
                }
                _ => {}
            }
        }

        if subpath.is_empty() {
            let main = pkg
                .get("main")
                .and_then(|v| v.as_str())
                .unwrap_or("index.js");
            let main_path = pkg_dir.join(main);
            return Ok(Self::try_path_with_extensions(&main_path));
        }

        Ok(None)
    }

    fn find_in_node_modules(&self, start_dir: &Path, specifier: &str) -> RuntimeResult<Option<PathBuf>> {
        let (package_name, subpath) = if specifier.starts_with('@') {
            let parts: Vec<&str> = specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                let pkg = format!("{}/{}", parts[0], parts[1]);
                let sub = if parts.len() > 2 { parts[2] } else { "" };
                (pkg, sub)
            } else {
                (specifier.to_string(), "")
            }
        } else {
            let parts: Vec<&str> = specifier.splitn(2, '/').collect();
            let pkg = parts[0].to_string();
            let sub = if parts.len() > 1 { parts[1] } else { "" };
            (pkg, sub)
        };

        let mut current = Some(start_dir.to_path_buf());
        while let Some(dir) = current {
            let nm = dir.join(NODE_MODULES);
            let pkg_dir = nm.join(&package_name);
            if pkg_dir.exists() && pkg_dir.is_dir() {
                if subpath.is_empty() {
                    if let Some(path) = Self::resolve_package_internal(&pkg_dir, "")? {
                        return Ok(Some(path));
                    }
                    // package.json missing or invalid — try directory index files.
                    if let Some(path) = Self::try_path_with_extensions(&pkg_dir) {
                        return Ok(Some(path));
                    }
                    return Ok(None);
                }
                let sub_path = pkg_dir.join(subpath);
                if let Some(path) = Self::try_path_with_extensions(&sub_path) {
                    return Ok(Some(path));
                }
            }
            current = dir.parent().map(|p| p.to_path_buf());
        }

        for global in &self.global_paths {
            let pkg_dir = global.join(&package_name);
            if pkg_dir.exists() && pkg_dir.is_dir() {
                if subpath.is_empty() {
                    if let Some(path) = Self::resolve_package_internal(&pkg_dir, "")? {
                        return Ok(Some(path));
                    }
                    if let Some(path) = Self::try_path_with_extensions(&pkg_dir) {
                        return Ok(Some(path));
                    }
                    return Ok(None);
                }
                let sub_path = pkg_dir.join(subpath);
                if let Some(path) = Self::try_path_with_extensions(&sub_path) {
                    return Ok(Some(path));
                }
            }
        }

        Ok(None)
    }

    fn format_for_path(path: &Path) -> ModuleFormat {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "mjs" | "mts" => ModuleFormat::ESM,
            "cjs" | "cts" => ModuleFormat::CJS,
            "json" => ModuleFormat::JSON,
            "wasm" => ModuleFormat::Wasm,
            "node" => ModuleFormat::Native,
            _ => ModuleFormat::CJS,
        }
    }

    fn load_module_at(
        path: &Path,
        format: ModuleFormat,
        is_external: bool,
    ) -> RuntimeResult<ResolvedModule> {
        let source = if format == ModuleFormat::JSON {
            fs::read_to_string(path).map_err(RuntimeError::IoError)?
        } else if format == ModuleFormat::Native || format == ModuleFormat::Wasm {
            String::new()
        } else {
            fs::read_to_string(path).map_err(RuntimeError::IoError)?
        };
        Ok(ResolvedModule {
            path: path.to_path_buf(),
            source: Arc::from(source),
            format,
            is_external,
        })
    }

    fn resolve_path(&self, specifier: &str, base: &Path) -> RuntimeResult<PathBuf> {
        let path = PathBuf::from(specifier);
        if path.is_absolute() {
            if let Some(found) = Self::try_path_with_extensions(&path) {
                return Ok(found);
            }
            return Err(RuntimeError::ModuleNotFound(format!(
                "Module not found: {}",
                specifier
            )));
        }

        if specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/')
        {
            let base_dir = if base.is_dir() {
                base.to_path_buf()
            } else {
                base.parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."))
            };
            let resolved = base_dir.join(&path);
            if let Some(found) = Self::try_path_with_extensions(&resolved) {
                return Ok(found);
            }
            return Err(RuntimeError::ModuleResolutionFailed(
                specifier.to_string(),
                vec![resolved],
            ));
        }

        let base_dir = if base.is_dir() {
            base.to_path_buf()
        } else {
            base.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        };

        if let Some(found) = self.find_in_node_modules(&base_dir, specifier)? {
            return Ok(found);
        }

        Err(RuntimeError::ModuleNotFound(format!(
            "Cannot find module '{}'",
            specifier
        )))
    }

    pub fn resolve(&self, specifier: &str, base: &Path) -> RuntimeResult<Arc<ResolvedModule>> {
        let key = cache_key(specifier, base);

        if let Some(cached) = self.memory.get(&key) {
            return Ok(Arc::clone(cached.value()));
        }

        if is_builtin(specifier) {
            let module = ResolvedModule {
                path: PathBuf::from(format!("node:{}", specifier)),
                source: Arc::from(""),
                format: ModuleFormat::Native,
                is_external: true,
            };
            let arc = Arc::new(module);
            self.memory.insert(key, Arc::clone(&arc));
            return Ok(arc);
        }

        {
            let persistent = self
                .persistent
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = persistent.get(&key) {
                if let Ok(meta) = fs::metadata(&entry.path) {
                    if let Ok(mtime) = meta.modified() {
                        let dur = mtime
                            .duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .unwrap_or_default();
                        if dur.as_secs() == entry.mtime_secs
                            && dur.subsec_nanos() == entry.mtime_nanos
                        {
                            if let Ok(module) =
                                Self::load_module_at(&entry.path, entry.format.clone(), entry.is_external)
                            {
                                let arc = Arc::new(module);
                                self.memory.insert(key, Arc::clone(&arc));
                                return Ok(arc);
                            }
                        }
                    }
                }
            }
        }

        let resolved_path = self.resolve_path(specifier, base)?;
        let module = Self::load_module_at(&resolved_path, Self::format_for_path(&resolved_path), false)?;
        let arc = Arc::new(module);
        self.memory.insert(key, Arc::clone(&arc));

        if let Ok(meta) = fs::metadata(&arc.path) {
            if let Ok(mtime) = meta.modified() {
                let dur = mtime
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                let entry = PersistentEntry {
                    key,
                    path: arc.path.clone(),
                    format: arc.format.clone(),
                    is_external: arc.is_external,
                    mtime_secs: dur.as_secs(),
                    mtime_nanos: dur.subsec_nanos(),
                };
                if let Ok(mut persistent) = self.persistent.write() {
                    persistent.insert(key, entry);
                    self.dirty.store(true, Ordering::Relaxed);
                }
            }
        }

        Ok(arc)
    }

    pub fn save(&self) {
        if self.dirty.load(Ordering::Relaxed) {
            // Take write lock to clone entries and reset dirty atomically,
            // preventing a concurrent resolve from setting dirty between
            // snapshot and reset.
            let entries = if let Ok(persistent) = self.persistent.write() {
                let cloned: HashMap<u64, PersistentEntry> = persistent.clone();
                self.dirty.store(false, Ordering::Relaxed);
                Some(cloned)
            } else {
                None
            };
            if let Some(entries) = entries {
                save_persistent_cache(&entries);
            }
        }
    }
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ModuleResolver {
    fn drop(&mut self) {
        self.save();
    }
}

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "assert"
            | "buffer"
            | "child_process"
            | "cluster"
            | "console"
            | "constants"
            | "crypto"
            | "dgram"
            | "dns"
            | "domain"
            | "events"
            | "fs"
            | "http"
            | "http2"
            | "https"
            | "module"
            | "net"
            | "os"
            | "path"
            | "perf_hooks"
            | "process"
            | "punycode"
            | "querystring"
            | "readline"
            | "repl"
            | "stream"
            | "string_decoder"
            | "timers"
            | "tls"
            | "tty"
            | "url"
            | "util"
            | "v8"
            | "vm"
            | "worker_threads"
            | "zlib"
    )
}

pub fn resolve_builtin_path(_name: &str) -> Option<PathBuf> {
    None
}
