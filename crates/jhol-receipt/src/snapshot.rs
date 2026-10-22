//! Portable project snapshots (blueprint Phase C).
//!
//! A snapshot captures `jhol.project.lock` JSON + an execution receipt.
//! Restore never rolls back production databases; limitations are recorded.

use crate::{ExecutionReceipt, ReceiptError, ReceiptStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

pub const SNAPSHOT_FORMAT: u32 = 1;
pub const SNAPSHOTS_DIR: &str = "snapshots";

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("snapshot not found: {0}")]
    NotFound(String),
    #[error("restore blocked: {0}")]
    Blocked(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub format_version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    /// Raw `jhol.project.lock` object (native locks stay authoritative).
    pub project_lock: serde_json::Value,
    pub receipt: ExecutionReceipt,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePlan {
    pub snapshot_id: String,
    pub would_write: Vec<String>,
    pub would_skip: Vec<String>,
    pub blocked: Vec<String>,
    pub limitations: Vec<String>,
}

fn snapshots_root(project: &Path) -> PathBuf {
    project.join(".jhol").join(SNAPSHOTS_DIR)
}

impl Snapshot {
    pub fn create(project_lock: serde_json::Value, mut receipt: ExecutionReceipt) -> Self {
        receipt.command = receipt.command.or(Some("snapshot".into()));
        Self {
            format_version: SNAPSHOT_FORMAT,
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            project_lock,
            receipt,
            limitations: vec![
                "restore does not mutate databases".into(),
                "native package trees require jhol sync --apply / install".into(),
            ],
        }
    }

    pub fn save(&self, project: &Path) -> Result<PathBuf, SnapshotError> {
        let dir = snapshots_root(project);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.id));
        fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(path)
    }

    pub fn load(project: &Path, id: &str) -> Result<Self, SnapshotError> {
        let path = snapshots_root(project).join(format!("{id}.json"));
        if !path.is_file() {
            return Err(SnapshotError::NotFound(id.to_string()));
        }
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }

    pub fn list(project: &Path) -> Result<Vec<(String, PathBuf)>, SnapshotError> {
        let dir = snapshots_root(project);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for e in fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push((stem.to_string(), p));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Plan restoring `jhol.project.lock` only — never DB, never silent.
    pub fn restore_plan(&self) -> RestorePlan {
        RestorePlan {
            snapshot_id: self.id.clone(),
            would_write: vec!["jhol.project.lock".into()],
            would_skip: vec![
                "node_modules (use jhol sync --apply)".into(),
                "database state (never auto-restored)".into(),
            ],
            blocked: Vec::new(),
            limitations: self.limitations.clone(),
        }
    }

    /// Apply: write project.lock back. Refuse if caller asks for DB (flag).
    pub fn restore_apply(
        &self,
        project: &Path,
        include_database: bool,
    ) -> Result<RestorePlan, SnapshotError> {
        if include_database {
            return Err(SnapshotError::Blocked(
                "database restore is not supported; refuse rather than guess".into(),
            ));
        }
        let plan = self.restore_plan();
        let lock_path = project.join("jhol.project.lock");
        fs::write(
            &lock_path,
            serde_json::to_string_pretty(&self.project_lock)?,
        )?;
        Ok(plan)
    }
}

pub fn empty_ok_receipt(project: &Path) -> ExecutionReceipt {
    let mut r = ExecutionReceipt::new(ReceiptStatus::Ok);
    r.project_root = Some(project.display().to_string());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let lock = serde_json::json!({"format_version": 1, "native_locks": []});
        let snap = Snapshot::create(lock, empty_ok_receipt(dir.path()));
        let path = snap.save(dir.path()).unwrap();
        assert!(path.is_file());
        let loaded = Snapshot::load(dir.path(), &snap.id).unwrap();
        assert_eq!(loaded.id, snap.id);
        let plan = loaded.restore_plan();
        assert!(plan.would_write.contains(&"jhol.project.lock".into()));
        loaded.restore_apply(dir.path(), false).unwrap();
        assert!(dir.path().join("jhol.project.lock").is_file());
        assert!(matches!(
            loaded.restore_apply(dir.path(), true),
            Err(SnapshotError::Blocked(_))
        ));
    }
}
