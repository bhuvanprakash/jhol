//! Optional `jhol.project.lock` — references native lock digests only.
//! Native package locks stay authoritative (blueprint §5 / schema graph-lock-v1).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const PROJECT_LOCK_FORMAT: u32 = 1;
pub const PROJECT_LOCK_FILENAME: &str = "jhol.project.lock";

#[derive(Debug, Error)]
pub enum ProjectLockError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterRef {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeLockRef {
    pub path: String,
    pub kind: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectLock {
    pub format_version: u32,
    pub project_config_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformInfo>,
    pub adapters: Vec<AdapterRef>,
    pub native_locks: Vec<NativeLockRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permitted_outputs: Vec<String>,
}

/// SHA-256 hex of file bytes. Missing file → digest of empty (caller should skip).
pub fn digest_file(path: &Path) -> Result<String, ProjectLockError> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

pub fn digest_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// Known native lock filenames → (kind, relative name).
const NATIVE_LOCK_CANDIDATES: &[(&str, &str)] = &[
    ("package-lock.json", "npm"),
    ("bun.lock", "bun"),
    ("yarn.lock", "yarn"),
    ("pnpm-lock.yaml", "pnpm"),
    ("Cargo.lock", "cargo"),
    ("uv.lock", "uv"),
    ("requirements.txt", "pip"),
];

/// Scan project root for native locks; hash each present file.
pub fn collect_native_locks(root: &Path) -> Result<Vec<NativeLockRef>, ProjectLockError> {
    let mut out = Vec::new();
    for (name, kind) in NATIVE_LOCK_CANDIDATES {
        let p = root.join(name);
        if p.is_file() {
            out.push(NativeLockRef {
                path: name.to_string(),
                kind: kind.to_string(),
                digest: digest_file(&p)?,
            });
        }
    }
    Ok(out)
}

pub fn build_project_lock(
    root: &Path,
    adapters: Vec<AdapterRef>,
) -> Result<ProjectLock, ProjectLockError> {
    let config_path = root.join("jhol.toml");
    let project_config_digest = if config_path.is_file() {
        digest_file(&config_path)?
    } else {
        digest_bytes(b"")
    };
    Ok(ProjectLock {
        format_version: PROJECT_LOCK_FORMAT,
        project_config_digest,
        platform: Some(PlatformInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            libc: None,
        }),
        adapters,
        native_locks: collect_native_locks(root)?,
        artifacts: Vec::new(),
        permitted_outputs: Vec::new(),
    })
}

pub fn write_project_lock(root: &Path, lock: &ProjectLock) -> Result<PathBuf, ProjectLockError> {
    let path = root.join(PROJECT_LOCK_FILENAME);
    let body = serde_json::to_string_pretty(lock)?;
    fs::write(&path, body)?;
    Ok(path)
}

pub fn read_project_lock(root: &Path) -> Result<Option<ProjectLock>, ProjectLockError> {
    let path = root.join(PROJECT_LOCK_FILENAME);
    if !path.is_file() {
        return Ok(None);
    }
    let s = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&s)?))
}

/// True when every recorded native lock path still matches its digest.
pub fn native_locks_match(root: &Path, lock: &ProjectLock) -> Result<bool, ProjectLockError> {
    Ok(collect_native_locks(root)? == lock.native_locks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn writes_and_verifies_native_digest() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package-lock.json"),
            r#"{"lockfileVersion":3}"#,
        )
        .unwrap();
        fs::write(dir.path().join("jhol.toml"), "schema_version = 1\n").unwrap();
        let lock = build_project_lock(
            dir.path(),
            vec![AdapterRef {
                id: "jhol-adapter-js".into(),
                version: "0.1.0".into(),
            }],
        )
        .unwrap();
        assert_eq!(lock.native_locks.len(), 1);
        assert_eq!(lock.native_locks[0].kind, "npm");
        let path = write_project_lock(dir.path(), &lock).unwrap();
        assert!(path.ends_with(PROJECT_LOCK_FILENAME));
        assert!(native_locks_match(dir.path(), &lock).unwrap());
        fs::write(dir.path().join("package-lock.json"), "changed").unwrap();
        assert!(!native_locks_match(dir.path(), &lock).unwrap());
    }

    /// Property: project lock digests only native lock bytes — never store
    /// contents or action-cache keys (blueprint Phase A invariant).
    #[test]
    fn property_project_lock_digest_equals_file_bytes_only() {
        let dir = tempfile::tempdir().unwrap();
        let body = br#"{"lockfileVersion":3,"packages":{}}"#;
        fs::write(dir.path().join("package-lock.json"), body).unwrap();
        // decoy store / action cache must not affect digest
        let store = dir.path().join(".jhol").join("action-cache");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("decoy"), b"should-not-enter-digest").unwrap();
        let lock = build_project_lock(dir.path(), vec![]).unwrap();
        assert_eq!(lock.native_locks.len(), 1);
        assert_eq!(lock.native_locks[0].digest, digest_bytes(body));
        assert!(native_locks_match(dir.path(), &lock).unwrap());
    }

    #[test]
    fn corrupted_project_lock_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(PROJECT_LOCK_FILENAME), "{not json").unwrap();
        assert!(read_project_lock(dir.path()).is_err());
    }

    #[test]
    fn detects_added_and_removed_native_locks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package-lock.json"), "{} ").unwrap();
        let lock = build_project_lock(dir.path(), vec![]).unwrap();

        fs::write(dir.path().join("uv.lock"), "version = 1\n").unwrap();
        assert!(!native_locks_match(dir.path(), &lock).unwrap());

        fs::remove_file(dir.path().join("uv.lock")).unwrap();
        fs::remove_file(dir.path().join("package-lock.json")).unwrap();
        assert!(!native_locks_match(dir.path(), &lock).unwrap());
    }
}
