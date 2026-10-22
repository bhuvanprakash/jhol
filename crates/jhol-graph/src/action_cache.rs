//! Local action-cache metadata (blueprint Phase B).
//! Separate from npm package CAS under `store/` — keys are action digests only.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ActionCacheError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// `Hash(action_name || sorted path=digest pairs)`.
pub fn action_key(action: &str, inputs: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(&str, &str)> = inputs.to_vec();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = Sha256::new();
    h.update(action.as_bytes());
    h.update([0u8]);
    for (p, d) in pairs {
        h.update(p.as_bytes());
        h.update([0u8]);
        h.update(d.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

pub fn cache_dir(project_or_jhol_cache: &Path) -> PathBuf {
    project_or_jhol_cache.join("action-cache")
}

pub fn lookup(root: &Path, key: &str) -> Option<PathBuf> {
    let p = cache_dir(root).join(key);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

pub fn store(root: &Path, key: &str, body: &[u8]) -> Result<PathBuf, ActionCacheError> {
    let dir = cache_dir(root);
    fs::create_dir_all(&dir)?;
    let p = dir.join(key);
    fs::write(&p, body)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_after_store() {
        let dir = tempfile::tempdir().unwrap();
        let key = action_key("sync", &[("package-lock.json", "abc")]);
        assert!(lookup(dir.path(), &key).is_none());
        store(dir.path(), &key, b"receipt").unwrap();
        assert_eq!(
            fs::read(lookup(dir.path(), &key).unwrap()).unwrap(),
            b"receipt"
        );
    }
}
