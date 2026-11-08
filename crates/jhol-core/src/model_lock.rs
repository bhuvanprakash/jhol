//! Models as part of the project's environment proof.
//!
//! A model pin lives in `.jhol/models.lock.json` rather than inside
//! `package-lock.json`, because that file belongs to npm and must stay readable by
//! npm. The *manifest* declares intent, this lock records the resolved commit and
//! per-file digests, and `jhol verify` checks both against the disk — one command,
//! one proof, npm compatibility intact.
//!
//! The verifying logic is deliberately the same shape as the tree verifier: ask the
//! disk what it has, compare with what the lock authorises, and name the difference.
//! A model that was edited, truncated or replaced must never be reported as fine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MODEL_LOCK_PATH: &str = ".jhol/models.lock.json";

/// SHA-256 of a file, streamed so a multi-gigabyte checkpoint does not have to fit
/// in memory.
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelLock {
    #[serde(default = "one")]
    pub schema: u32,
    /// name -> artifact, ordered for stable diffs.
    #[serde(default)]
    pub models: BTreeMap<String, ModelEntry>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub source: String,
    pub revision: String,
    /// Where the files live, relative to the project root.
    pub dir: String,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    Match,
    Missing { name: String, path: String },
    Wrong { name: String, path: String, expected: String, got: String },
    NoLock,
}

impl ModelState {
    pub fn is_match(&self) -> bool {
        matches!(self, ModelState::Match)
    }

    pub fn describe(&self) -> String {
        match self {
            ModelState::Match => "models match the lock".to_string(),
            ModelState::Missing { name, path } => format!("{name}: {path} is missing"),
            ModelState::Wrong {
                name,
                path,
                expected,
                got,
            } => format!(
                "{name}: {path} is {} but the lock pins {}",
                &got[..12.min(got.len())],
                &expected[..12.min(expected.len())]
            ),
            ModelState::NoLock => "no model lock".to_string(),
        }
    }
}

impl ModelLock {
    pub fn path(root: &Path) -> PathBuf {
        root.join(MODEL_LOCK_PATH)
    }

    pub fn load(root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(root)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write atomically: a half-written lock is worse than none.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn insert(&mut self, name: &str, entry: ModelEntry) {
        self.models.insert(name.to_string(), entry);
    }

    /// Verify every pinned file against the project. First mismatch wins, so the
    /// report names one concrete problem rather than a wall.
    pub fn verify(&self, root: &Path) -> ModelState {
        for (name, entry) in &self.models {
            for file in &entry.files {
                let p = root.join(&entry.dir).join(&file.path);
                let Ok(meta) = std::fs::metadata(&p) else {
                    return ModelState::Missing {
                        name: name.clone(),
                        path: p.display().to_string(),
                    };
                };
                if meta.len() != file.size {
                    return ModelState::Wrong {
                        name: name.clone(),
                        path: p.display().to_string(),
                        expected: file.sha256.clone(),
                        got: format!("size {} != {}", meta.len(), file.size),
                    };
                }
                match sha256_file(&p) {
                    Ok(got) if got == file.sha256 => {}
                    Ok(got) => {
                        return ModelState::Wrong {
                            name: name.clone(),
                            path: p.display().to_string(),
                            expected: file.sha256.clone(),
                            got,
                        }
                    }
                    Err(e) => {
                        return ModelState::Missing {
                            name: name.clone(),
                            path: format!("{} ({e})", p.display()),
                        }
                    }
                }
            }
        }
        ModelState::Match
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &str, path: &str, sha: &str, size: u64) -> ModelEntry {
        ModelEntry {
            source: "hf://a/b".into(),
            revision: "0123456789012345678901234567890123456789".into(),
            dir: dir.into(),
            files: vec![ModelFile {
                path: path.into(),
                sha256: sha.into(),
                size,
            }],
        }
    }

    #[test]
    fn round_trips_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let file = root.join("models/demo/w.bin");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"weights").unwrap();
        let sha = sha256_file(&file).unwrap();

        let mut lock = ModelLock::default();
        lock.insert("demo", entry("models/demo", "w.bin", &sha, 7));
        lock.save(root).unwrap();

        let loaded = ModelLock::load(root).unwrap();
        assert_eq!(loaded, lock);
        assert_eq!(loaded.verify(root), ModelState::Match);
    }

    #[test]
    fn tampered_and_missing_files_are_named() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let file = root.join("models/demo/w.bin");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"weights").unwrap();
        let sha = sha256_file(&file).unwrap();
        let mut lock = ModelLock::default();
        lock.insert("demo", entry("models/demo", "w.bin", &sha, 7));

        std::fs::write(&file, b"weightZ").unwrap(); // same size, different bytes
        match lock.verify(root) {
            ModelState::Wrong { name, .. } => assert_eq!(name, "demo"),
            other => panic!("expected Wrong, got {other:?}"),
        }

        std::fs::remove_file(&file).unwrap();
        assert!(matches!(lock.verify(root), ModelState::Missing { .. }));
    }
}
