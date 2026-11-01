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
    /// Exact `package.json` text at capture time. Restore refuses to write lock
    /// files when the project's manifest no longer matches this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Exact `package-lock.json` text at capture time (native JS tree identity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_lock: Option<String>,
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
            manifest: None,
            package_lock: None,
        }
    }

    /// Attach the project's JS manifest and npm lock text captured by the CLI.
    pub fn with_project_files(
        mut self,
        manifest: Option<String>,
        package_lock: Option<String>,
    ) -> Self {
        self.manifest = manifest;
        self.package_lock = package_lock;
        self
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

    /// Plan a restore against a concrete project. A restore is blocked when the
    /// project's `package.json` is not byte-identical to the snapshot's, because
    /// the captured `package-lock.json` describes a different manifest then.
    pub fn restore_plan_in(&self, project: &Path) -> RestorePlan {
        let mut plan = self.restore_plan();
        match &self.manifest {
            None => plan.blocked.push(
                "snapshot predates manifest capture; it cannot be verified against package.json"
                    .into(),
            ),
            Some(captured) => {
                let current = fs::read_to_string(project.join("package.json")).ok();
                if current.as_deref() != Some(captured.as_str()) {
                    plan.blocked.push(
                        "package.json differs from the snapshot; restore would pin a tree for another manifest"
                            .into(),
                    );
                }
            }
        }
        if self.package_lock.is_some() {
            plan.would_write.push("package-lock.json".into());
        }
        plan
    }

    /// Apply: write `jhol.project.lock` and `package-lock.json` back. Never touches
    /// `node_modules` (run `jhol sync --apply`), and refuses database restore.
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
        let plan = self.restore_plan_in(project);
        if !plan.blocked.is_empty() {
            return Err(SnapshotError::Blocked(plan.blocked.join("; ")));
        }
        fs::write(
            project.join("jhol.project.lock"),
            serde_json::to_string_pretty(&self.project_lock)?,
        )?;
        if let Some(lock) = &self.package_lock {
            fs::write(project.join("package-lock.json"), lock)?;
        }
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
        // No manifest captured: restore must refuse rather than guess.
        assert!(matches!(
            loaded.restore_apply(dir.path(), false),
            Err(SnapshotError::Blocked(_))
        ));
        assert!(matches!(
            loaded.restore_apply(dir.path(), true),
            Err(SnapshotError::Blocked(_))
        ));
    }

    #[test]
    fn restore_writes_lock_only_when_manifest_matches() {
        let dir = tempfile::tempdir().unwrap();
        let pj = "{\"dependencies\":{\"left-pad\":\"1.3.0\"}}";
        fs::write(dir.path().join("package.json"), pj).unwrap();
        let lock = "{\"lockfileVersion\":3}";
        let snap = Snapshot::create(
            serde_json::json!({"native_locks": []}),
            empty_ok_receipt(dir.path()),
        )
        .with_project_files(Some(pj.into()), Some(lock.into()));

        // Same manifest: lock files are written.
        let plan = snap.restore_plan_in(dir.path());
        assert!(plan.blocked.is_empty(), "{:?}", plan.blocked);
        assert!(plan.would_write.contains(&"package-lock.json".into()));
        snap.restore_apply(dir.path(), false).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("package-lock.json")).unwrap(),
            lock
        );
        assert!(
            !dir.path().join("node_modules").exists(),
            "restore must not create node_modules"
        );

        // Edited manifest: refused, and the lock on disk is left alone.
        fs::write(dir.path().join("package.json"), "{\"dependencies\":{}}").unwrap();
        assert!(matches!(
            snap.restore_apply(dir.path(), false),
            Err(SnapshotError::Blocked(m)) if m.contains("package.json differs")
        ));
        assert_eq!(
            fs::read_to_string(dir.path().join("package-lock.json")).unwrap(),
            lock
        );
    }

    #[test]
    fn snapshots_without_project_files_still_load() {
        // A snapshot written before manifest capture has no `manifest` key.
        let dir = tempfile::tempdir().unwrap();
        let old = Snapshot::create(serde_json::json!({}), empty_ok_receipt(dir.path()));
        let mut v: serde_json::Value = serde_json::to_value(&old).unwrap();
        v.as_object_mut().unwrap().remove("manifest");
        let loaded: Snapshot = serde_json::from_value(v).unwrap();
        assert!(loaded.manifest.is_none());
    }
}
