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
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
