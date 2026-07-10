use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use xxhash_rust::xxh3::xxh3_64;

use crate::errors::{RuntimeError, RuntimeResult};

const CACHE_VERSION: u32 = 1;
const CACHE_DIR_NAME: &str = "runtime-bytecode-v1";

fn cache_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = dirs::cache_dir()
            .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir)
            .join("jhol")
            .join(CACHE_DIR_NAME);
        let _ = fs::create_dir_all(&base);
        base
    })
    .clone()
}

fn cache_path(source_hash: u64) -> PathBuf {
    cache_dir().join(format!("{:016x}.qbc", source_hash))
}

pub fn key(source: &str) -> u64 {
    let mut input = Vec::with_capacity(source.len() + 4);
    input.extend_from_slice(CACHE_VERSION.to_le_bytes().as_slice());
    input.extend_from_slice(source.as_bytes());
    xxh3_64(&input)
}

pub fn load(source_hash: u64) -> Option<Vec<u8>> {
    fs::read(cache_path(source_hash)).ok()
}

pub fn save(source_hash: u64, bytecode: &[u8]) -> RuntimeResult<()> {
    let path = cache_path(source_hash);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytecode).map_err(RuntimeError::IoError)?;
    fs::rename(&tmp, &path).map_err(RuntimeError::IoError)?;
    Ok(())
}
