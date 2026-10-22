//! Jhol Diff — graph operation, not a separate Git repo (blueprint §11).

#![forbid(unsafe_code)]

use jhol_receipt::ExecutionReceipt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    Unchanged,
    LocalCacheHit,
    RemoteCacheCandidate,
    RequiresWork,
    Blocked,
    PolicyDenied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    pub node_or_path: String,
    pub class: ChangeClass,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    pub from_receipt: Option<String>,
    pub to_receipt: Option<String>,
    pub entries: Vec<DiffEntry>,
}

/// Compare two receipts by input digests (minimal Phase B stub).
pub fn diff_receipts(before: &ExecutionReceipt, after: &ExecutionReceipt) -> DiffReport {
    use std::collections::HashMap;
    let mut before_map: HashMap<&str, &str> = HashMap::new();
    for i in &before.inputs {
        before_map.insert(i.path.as_str(), i.digest.as_str());
    }
    let mut entries = Vec::new();
    for i in &after.inputs {
        match before_map.get(i.path.as_str()) {
            Some(d) if *d == i.digest.as_str() => entries.push(DiffEntry {
                node_or_path: i.path.clone(),
                class: ChangeClass::Unchanged,
                reason: "input digest unchanged".into(),
            }),
            Some(_) => entries.push(DiffEntry {
                node_or_path: i.path.clone(),
                class: ChangeClass::RequiresWork,
                reason: "input digest changed".into(),
            }),
            None => entries.push(DiffEntry {
                node_or_path: i.path.clone(),
                class: ChangeClass::RequiresWork,
                reason: "new input path".into(),
            }),
        }
    }
    DiffReport {
        from_receipt: Some(before.id.clone()),
        to_receipt: Some(after.id.clone()),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jhol_receipt::{DigestedPath, ExecutionReceipt, ReceiptStatus};

    #[test]
    fn detects_digest_change() {
        let mut a = ExecutionReceipt::new(ReceiptStatus::Ok);
        a.inputs.push(DigestedPath {
            path: "package.json".into(),
            digest: "aaa".into(),
            verified: None,
        });
        let mut b = ExecutionReceipt::new(ReceiptStatus::Ok);
        b.inputs.push(DigestedPath {
            path: "package.json".into(),
            digest: "bbb".into(),
            verified: None,
        });
        let report = diff_receipts(&a, &b);
        assert_eq!(report.entries[0].class, ChangeClass::RequiresWork);
    }
}
