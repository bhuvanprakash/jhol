//! Execution receipts (blueprint §5.3 / schemas/execution-receipt-v1).
//!
//! A receipt records what was planned and what actually ran. It must never
//! contain credentials or plaintext secrets.

#![forbid(unsafe_code)]

pub mod snapshot;

pub use snapshot::{
    empty_ok_receipt, RestorePlan, Snapshot, SnapshotError, SNAPSHOTS_DIR, SNAPSHOT_FORMAT,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Ok,
    Failed,
    Partial,
    Cancelled,
    PolicyDenied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    FilesystemWrite,
    NetworkFetch,
    ProcessLaunch,
    LifecycleScript,
    DatabaseMutation,
    RemoteUpload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestedPath {
    pub path: String,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub format_version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub status: ReceiptStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<DigestedPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<DigestedPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_effects: Vec<SideEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adapter_receipts: Vec<serde_json::Value>,
}

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("receipt format version {found} is not supported (this build writes {supported})")]
    UnsupportedFormat { found: u32, supported: u32 },
}

impl ExecutionReceipt {
    pub fn new(status: ReceiptStatus) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            status,
            command: None,
            project_root: None,
            plan_digest: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            warnings: Vec::new(),
            limitations: Vec::new(),
            adapter_receipts: Vec::new(),
        }
    }

    pub fn to_json_pretty(&self) -> Result<String, ReceiptError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self, ReceiptError> {
        let r: Self = serde_json::from_str(s)?;
        if r.format_version != FORMAT_VERSION {
            return Err(ReceiptError::UnsupportedFormat {
                found: r.format_version,
                supported: FORMAT_VERSION,
            });
        }
        Ok(r)
    }
}

impl ReceiptStatus {
    /// `Ok` is the only fully successful outcome; `Partial` did some work.
    pub fn is_success(self) -> bool {
        matches!(self, ReceiptStatus::Ok | ReceiptStatus::Partial)
    }
}

/// Directory under the project that holds saved execution receipts.
pub const RECEIPTS_DIR: &str = "receipts";
/// Name of the most recent receipt inside [`RECEIPTS_DIR`].
pub const LATEST_RECEIPT: &str = "latest.json";

/// Persist a receipt under `<project>/.jhol/receipts/` and update `latest.json`.
/// Returns the path of the id-named file. Snapshots and `jhol diff` read these
/// files, so a `sync` that is not saved cannot be referenced later.
pub fn save_receipt(
    project: &std::path::Path,
    receipt: &ExecutionReceipt,
) -> Result<std::path::PathBuf, ReceiptError> {
    let dir = project.join(".jhol").join(RECEIPTS_DIR);
    std::fs::create_dir_all(&dir)?;
    let body = receipt.to_json_pretty()?;
    // Write to a temp file and rename into place: a crash mid-write must not
    // leave a truncated `latest.json` (snapshots and `jhol diff` read it).
    let tmp = dir.join(format!(".{}.tmp", receipt.id));
    std::fs::write(&tmp, &body)?;
    let path = dir.join(format!("{}.json", receipt.id));
    std::fs::rename(&tmp, &path)?;
    let tmp_latest = dir.join(format!(".{LATEST_RECEIPT}.tmp"));
    std::fs::write(&tmp_latest, &body)?;
    std::fs::rename(&tmp_latest, dir.join(LATEST_RECEIPT))?;
    Ok(path)
}

/// Load the most recently saved receipt, if any.
pub fn load_latest_receipt(project: &std::path::Path) -> Option<ExecutionReceipt> {
    load_latest_receipt_result(project).ok().flatten()
}

/// Like [`load_latest_receipt`], but distinguishes "no receipt yet" from
/// "the receipt is there and unreadable" (corrupt JSON or a future format).
pub fn load_latest_receipt_result(
    project: &std::path::Path,
) -> Result<Option<ExecutionReceipt>, ReceiptError> {
    let raw = match std::fs::read_to_string(
        project
            .join(".jhol")
            .join(RECEIPTS_DIR)
            .join(LATEST_RECEIPT),
    ) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ReceiptError::Io(e)),
    };
    // A missing file is "no receipt"; anything readable must parse.
    Ok(Some(ExecutionReceipt::from_json(&raw)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_receipt_is_found_as_latest() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_latest_receipt(dir.path()).is_none());
        let mut first = ExecutionReceipt::new(ReceiptStatus::Ok);
        first.command = Some("sync".into());
        save_receipt(dir.path(), &first).unwrap();
        let mut second = ExecutionReceipt::new(ReceiptStatus::Partial);
        second.command = Some("sync".into());
        let path = save_receipt(dir.path(), &second).unwrap();
        assert!(path.is_file());
        let latest = load_latest_receipt(dir.path()).expect("latest saved");
        assert_eq!(latest.id, second.id, "latest must be the last save");
        assert_eq!(latest.status, ReceiptStatus::Partial);
        assert!(dir
            .path()
            .join(".jhol/receipts")
            .join(format!("{}.json", first.id))
            .is_file());
    }

    #[test]
    fn latest_load_distinguishes_missing_from_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_latest_receipt_result(dir.path()).unwrap().is_none());
        let r = ExecutionReceipt::new(ReceiptStatus::Ok);
        save_receipt(dir.path(), &r).unwrap();
        assert!(load_latest_receipt_result(dir.path()).unwrap().is_some());
        // Corrupt latest.json is an error, not a silent None.
        std::fs::write(dir.path().join(".jhol/receipts/latest.json"), "{broken").unwrap();
        assert!(load_latest_receipt_result(dir.path()).is_err());
        // A future format version is rejected with a clear message.
        let mut future = ExecutionReceipt::new(ReceiptStatus::Ok);
        future.format_version = FORMAT_VERSION + 1;
        std::fs::write(
            dir.path().join(".jhol/receipts/latest.json"),
            future.to_json_pretty().unwrap(),
        )
        .unwrap();
        match load_latest_receipt_result(dir.path()) {
            Err(ReceiptError::UnsupportedFormat { found, .. }) => {
                assert_eq!(found, FORMAT_VERSION + 1);
            }
            other => panic!("expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn interrupted_save_leaves_previous_latest_intact() {
        let dir = tempfile::tempdir().unwrap();
        let first = ExecutionReceipt::new(ReceiptStatus::Ok);
        save_receipt(dir.path(), &first).unwrap();
        // A crashed save leaves at most a stray temp file; latest still parses.
        std::fs::write(
            dir.path().join(".jhol/receipts/.deadbeef.tmp"),
            "{\"format_version\":",
        )
        .unwrap();
        let latest = load_latest_receipt(dir.path()).expect("latest intact");
        assert_eq!(latest.id, first.id);
    }

    #[test]
    fn status_success_classification() {
        assert!(ReceiptStatus::Ok.is_success());
        assert!(ReceiptStatus::Partial.is_success());
        assert!(!ReceiptStatus::Failed.is_success());
        assert!(!ReceiptStatus::Cancelled.is_success());
        assert!(!ReceiptStatus::PolicyDenied.is_success());
    }

    #[test]
    fn roundtrip_json() {
        let mut r = ExecutionReceipt::new(ReceiptStatus::Ok);
        r.command = Some("plan".into());
        r.limitations
            .push("inference nondeterminism not pinned".into());
        let s = r.to_json_pretty().unwrap();
        let back = ExecutionReceipt::from_json(&s).unwrap();
        assert_eq!(back.format_version, FORMAT_VERSION);
        assert_eq!(back.command.as_deref(), Some("plan"));
        assert_eq!(back.limitations.len(), 1);
    }
}
