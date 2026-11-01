//! Jhol Diff — a graph operation, not a separate Git repository (blueprint §11).
//!
//! Diffing is deliberately class-based: callers need to know whether a change
//! is unchanged, cacheable, or requires work. It never executes anything.

#![forbid(unsafe_code)]

use jhol_graph::{EdgeKind, Graph, ProjectLock};
use jhol_receipt::ExecutionReceipt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    Unchanged,
    LocalCacheHit,
    RemoteCacheCandidate,
    RequiresWork,
    Blocked,
    PolicyDenied,
    Added,
    Removed,
}

impl ChangeClass {
    pub fn is_work(self) -> bool {
        matches!(
            self,
            ChangeClass::RequiresWork
                | ChangeClass::Blocked
                | ChangeClass::PolicyDenied
                | ChangeClass::Added
                | ChangeClass::Removed
        )
    }

    pub fn is_change(self) -> bool {
        !matches!(self, ChangeClass::Unchanged)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffScope {
    #[default]
    Input,
    Output,
    Node,
    Edge,
    Lock,
    SideEffect,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    pub node_or_path: String,
    pub class: ChangeClass,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_digest: Option<String>,
    #[serde(default)]
    pub scope: DiffScope,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffSummary {
    pub unchanged: usize,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub cacheable: usize,
    pub requires_work: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_receipt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_receipt: Option<String>,
    pub entries: Vec<DiffEntry>,
    #[serde(default)]
    pub summary: DiffSummary,
}

impl DiffReport {
    pub fn new(
        from_receipt: Option<String>,
        to_receipt: Option<String>,
        entries: Vec<DiffEntry>,
    ) -> Self {
        let summary = summarize(&entries);
        Self {
            from_receipt,
            to_receipt,
            entries,
            summary,
        }
    }

    /// True when no entry requires work or is blocked.
    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| !e.class.is_work())
    }

    pub fn has_work(&self) -> bool {
        self.entries.iter().any(|e| e.class.is_work())
    }

    pub fn by_class(&self, class: ChangeClass) -> impl Iterator<Item = &DiffEntry> {
        self.entries.iter().filter(move |e| e.class == class)
    }
}

fn summarize(entries: &[DiffEntry]) -> DiffSummary {
    let mut summary = DiffSummary::default();
    for entry in entries {
        match entry.class {
            ChangeClass::Unchanged => summary.unchanged += 1,
            ChangeClass::Added => summary.added += 1,
            ChangeClass::Removed => summary.removed += 1,
            ChangeClass::RequiresWork => summary.requires_work += 1,
            ChangeClass::Blocked | ChangeClass::PolicyDenied => summary.requires_work += 1,
            ChangeClass::LocalCacheHit | ChangeClass::RemoteCacheCandidate => {
                summary.cacheable += 1
            }
        }
        if entry.class.is_change() {
            summary.changed += 1;
        }
    }
    summary
}

fn entry(
    path: impl Into<String>,
    class: ChangeClass,
    reason: impl Into<String>,
    scope: DiffScope,
    before: Option<String>,
    after: Option<String>,
) -> DiffEntry {
    DiffEntry {
        node_or_path: path.into(),
        class,
        reason: reason.into(),
        before_digest: before,
        after_digest: after,
        scope,
    }
}

fn diff_digests<'a>(
    before: impl Iterator<Item = (&'a String, &'a String)>,
    after: impl Iterator<Item = (&'a String, &'a String)>,
    scope: DiffScope,
    out: &mut Vec<DiffEntry>,
) {
    let before: BTreeMap<&str, &str> = before.map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let after: BTreeMap<&str, &str> = after.map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let paths: BTreeSet<&str> = before.keys().chain(after.keys()).copied().collect();
    for path in paths {
        match (before.get(path), after.get(path)) {
            (Some(old), Some(new)) if old == new => out.push(entry(
                path,
                ChangeClass::Unchanged,
                "digest unchanged",
                scope,
                Some((*old).into()),
                Some((*new).into()),
            )),
            (Some(old), Some(new)) => out.push(entry(
                path,
                ChangeClass::RequiresWork,
                "digest changed",
                scope,
                Some((*old).into()),
                Some((*new).into()),
            )),
            (None, Some(new)) => out.push(entry(
                path,
                ChangeClass::Added,
                "new path",
                scope,
                None,
                Some((*new).into()),
            )),
            (Some(old), None) => out.push(entry(
                path,
                ChangeClass::Removed,
                "path removed",
                scope,
                Some((*old).into()),
                None,
            )),
            (None, None) => {}
        }
    }
}

/// Compare two receipts: inputs, outputs, plan digest, status and side effects.
pub fn diff_receipts(before: &ExecutionReceipt, after: &ExecutionReceipt) -> DiffReport {
    let mut entries = Vec::new();

    diff_digests(
        before.inputs.iter().map(|i| (&i.path, &i.digest)),
        after.inputs.iter().map(|i| (&i.path, &i.digest)),
        DiffScope::Input,
        &mut entries,
    );
    diff_digests(
        before.outputs.iter().map(|i| (&i.path, &i.digest)),
        after.outputs.iter().map(|i| (&i.path, &i.digest)),
        DiffScope::Output,
        &mut entries,
    );

    match (&before.plan_digest, &after.plan_digest) {
        (None, None) => {}
        (a, b) if a == b => entries.push(entry(
            "plan",
            ChangeClass::Unchanged,
            "plan digest unchanged",
            DiffScope::Lock,
            a.clone(),
            b.clone(),
        )),
        (a, b) => entries.push(entry(
            "plan",
            ChangeClass::RequiresWork,
            "plan digest changed",
            DiffScope::Lock,
            a.clone(),
            b.clone(),
        )),
    }

    if before.status != after.status {
        entries.push(entry(
            "status",
            ChangeClass::RequiresWork,
            format!("status {:?} -> {:?}", before.status, after.status),
            DiffScope::Status,
            None,
            None,
        ));
    }

    let side_effects = |r: &ExecutionReceipt| -> BTreeSet<String> {
        r.side_effects.iter().map(|s| format!("{s:?}")).collect()
    };
    let before_effects = side_effects(before);
    let after_effects = side_effects(after);
    for effect in after_effects.difference(&before_effects) {
        entries.push(entry(
            format!("side_effect:{effect}"),
            ChangeClass::RequiresWork,
            "new side effect",
            DiffScope::SideEffect,
            None,
            None,
        ));
    }
    for effect in before_effects.difference(&after_effects) {
        entries.push(entry(
            format!("side_effect:{effect}"),
            ChangeClass::Removed,
            "side effect no longer planned",
            DiffScope::SideEffect,
            None,
            None,
        ));
    }

    DiffReport::new(Some(before.id.clone()), Some(after.id.clone()), entries)
}

/// Structural diff of two typed graphs.
pub fn diff_graphs(before: &Graph, after: &Graph) -> DiffReport {
    let mut entries = Vec::new();

    let before_nodes: BTreeMap<&str, &str> = before
        .nodes
        .iter()
        .map(|(id, node)| (id.as_str(), node.label.as_str()))
        .collect();
    let after_nodes: BTreeMap<&str, &str> = after
        .nodes
        .iter()
        .map(|(id, node)| (id.as_str(), node.label.as_str()))
        .collect();
    let ids: BTreeSet<&str> = before_nodes
        .keys()
        .chain(after_nodes.keys())
        .copied()
        .collect();
    for id in ids {
        match (before_nodes.get(id), after_nodes.get(id)) {
            (Some(a), Some(b)) if a == b => entries.push(entry(
                id,
                ChangeClass::Unchanged,
                "node unchanged",
                DiffScope::Node,
                Some((*a).into()),
                Some((*b).into()),
            )),
            (Some(a), Some(b)) => entries.push(entry(
                id,
                ChangeClass::RequiresWork,
                "node label changed",
                DiffScope::Node,
                Some((*a).into()),
                Some((*b).into()),
            )),
            (None, Some(b)) => entries.push(entry(
                id,
                ChangeClass::Added,
                "node added",
                DiffScope::Node,
                None,
                Some((*b).into()),
            )),
            (Some(a), None) => entries.push(entry(
                id,
                ChangeClass::Removed,
                "node removed",
                DiffScope::Node,
                Some((*a).into()),
                None,
            )),
            (None, None) => {}
        }
    }

    fn edge_key(edge: &jhol_graph::Edge) -> String {
        format!(
            "{} -> {} [{}]",
            edge.from.as_str(),
            edge.to.as_str(),
            edge_kind(edge.kind)
        )
    }
    fn edge_kind(kind: EdgeKind) -> &'static str {
        match kind {
            EdgeKind::Requires => "requires",
            EdgeKind::BuildRequires => "build_requires",
            EdgeKind::UsesToolchain => "uses_toolchain",
            EdgeKind::Consumes => "consumes",
            EdgeKind::Produces => "produces",
            EdgeKind::NeedsService => "needs_service",
            EdgeKind::NeedsSchema => "needs_schema",
            EdgeKind::InvalidatedBy => "invalidated_by",
            EdgeKind::GeneratedFrom => "generated_from",
            EdgeKind::OptionalUnder => "optional_under",
            EdgeKind::PinnedTo => "pinned_to",
        }
    }

    let before_edges: BTreeSet<String> = before.edges.iter().map(edge_key).collect();
    let after_edges: BTreeSet<String> = after.edges.iter().map(edge_key).collect();
    for key in after_edges.difference(&before_edges) {
        entries.push(entry(
            key.clone(),
            ChangeClass::Added,
            "edge added",
            DiffScope::Edge,
            None,
            None,
        ));
    }
    for key in before_edges.difference(&after_edges) {
        entries.push(entry(
            key.clone(),
            ChangeClass::Removed,
            "edge removed",
            DiffScope::Edge,
            None,
            None,
        ));
    }
    for key in before_edges.intersection(&after_edges) {
        entries.push(entry(
            key.clone(),
            ChangeClass::Unchanged,
            "edge unchanged",
            DiffScope::Edge,
            None,
            None,
        ));
    }

    DiffReport::new(None, None, entries)
}

/// Diff two project locks without touching native package trees.
pub fn diff_project_locks(before: &ProjectLock, after: &ProjectLock) -> DiffReport {
    let mut entries = Vec::new();
    if before.project_config_digest == after.project_config_digest {
        entries.push(entry(
            "jhol.toml",
            ChangeClass::Unchanged,
            "project config unchanged",
            DiffScope::Lock,
            Some(before.project_config_digest.clone()),
            Some(after.project_config_digest.clone()),
        ));
    } else {
        entries.push(entry(
            "jhol.toml",
            ChangeClass::RequiresWork,
            "project config changed",
            DiffScope::Lock,
            Some(before.project_config_digest.clone()),
            Some(after.project_config_digest.clone()),
        ));
    }

    diff_digests(
        before.native_locks.iter().map(|l| (&l.path, &l.digest)),
        after.native_locks.iter().map(|l| (&l.path, &l.digest)),
        DiffScope::Lock,
        &mut entries,
    );

    let adapters = |lock: &ProjectLock| -> BTreeSet<String> {
        lock.adapters
            .iter()
            .map(|a| format!("{}@{}", a.id, a.version))
            .collect()
    };
    let before_adapters = adapters(before);
    let after_adapters = adapters(after);
    for adapter in after_adapters.difference(&before_adapters) {
        entries.push(entry(
            format!("adapter:{adapter}"),
            ChangeClass::Added,
            "adapter added",
            DiffScope::Lock,
            None,
            None,
        ));
    }
    for adapter in before_adapters.difference(&after_adapters) {
        entries.push(entry(
            format!("adapter:{adapter}"),
            ChangeClass::Removed,
            "adapter removed",
            DiffScope::Lock,
            None,
            None,
        ));
    }

    let outputs = |lock: &ProjectLock| -> BTreeSet<String> {
        lock.permitted_outputs.iter().cloned().collect()
    };
    let before_outputs = outputs(before);
    let after_outputs = outputs(after);
    for path in after_outputs.difference(&before_outputs) {
        entries.push(entry(
            path.clone(),
            ChangeClass::Added,
            "permitted output added",
            DiffScope::Output,
            None,
            None,
        ));
    }
    for path in before_outputs.difference(&after_outputs) {
        entries.push(entry(
            path.clone(),
            ChangeClass::Removed,
            "permitted output removed",
            DiffScope::Output,
            None,
            None,
        ));
    }

    DiffReport::new(None, None, entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jhol_graph::{AdapterRef, NativeLockRef, Node, NodeId, NodeKind};
    use jhol_receipt::{DigestedPath, ExecutionReceipt, ReceiptStatus};

    fn digest(path: &str, digest: &str) -> DigestedPath {
        DigestedPath {
            path: path.into(),
            digest: digest.into(),
            verified: None,
        }
    }

    #[test]
    fn detects_digest_change() {
        let mut a = ExecutionReceipt::new(ReceiptStatus::Ok);
        a.inputs.push(digest("package.json", "aaa"));
        let mut b = ExecutionReceipt::new(ReceiptStatus::Ok);
        b.inputs.push(digest("package.json", "bbb"));
        let report = diff_receipts(&a, &b);
        assert_eq!(report.entries[0].class, ChangeClass::RequiresWork);
        assert_eq!(report.summary.changed, 1);
        assert!(report.has_work());
    }

    #[test]
    fn detects_added_and_removed_inputs() {
        let mut a = ExecutionReceipt::new(ReceiptStatus::Ok);
        a.inputs.push(digest("old.txt", "aaa"));
        a.inputs.push(digest("same.txt", "ccc"));
        let mut b = ExecutionReceipt::new(ReceiptStatus::Ok);
        b.inputs.push(digest("same.txt", "ccc"));
        b.inputs.push(digest("new.txt", "bbb"));
        let report = diff_receipts(&a, &b);
        assert_eq!(report.by_class(ChangeClass::Added).count(), 1);
        assert_eq!(report.by_class(ChangeClass::Removed).count(), 1);
        assert_eq!(report.by_class(ChangeClass::Unchanged).count(), 1);
        assert_eq!(report.summary.added, 1);
        assert_eq!(report.summary.removed, 1);
    }

    #[test]
    fn clean_report_has_no_work() {
        let mut a = ExecutionReceipt::new(ReceiptStatus::Ok);
        a.inputs.push(digest("x", "1"));
        let mut b = ExecutionReceipt::new(ReceiptStatus::Ok);
        b.inputs.push(digest("x", "1"));
        b.id = a.id.clone();
        let report = diff_receipts(&a, &b);
        assert!(report.is_clean());
        assert!(!report.has_work());
    }

    #[test]
    fn status_change_is_work() {
        let a = ExecutionReceipt::new(ReceiptStatus::Ok);
        let b = ExecutionReceipt::new(ReceiptStatus::Failed);
        let report = diff_receipts(&a, &b);
        assert!(report
            .entries
            .iter()
            .any(|e| e.scope == DiffScope::Status && e.class == ChangeClass::RequiresWork));
    }

    #[test]
    fn graph_diff_sees_nodes_and_edges() {
        let mut before = Graph::new();
        let a = NodeId::compute(NodeKind::BuildAction, "a", "", "");
        before.insert_node(Node {
            id: a.clone(),
            kind: NodeKind::BuildAction,
            label: "a".into(),
        });
        let mut after = before.clone();
        let b = NodeId::compute(NodeKind::BuildAction, "b", "", "");
        after.insert_node(Node {
            id: b.clone(),
            kind: NodeKind::BuildAction,
            label: "b".into(),
        });
        after
            .add_edge(jhol_graph::Edge {
                from: a,
                to: b,
                kind: EdgeKind::Requires,
            })
            .unwrap();
        let report = diff_graphs(&before, &after);
        assert_eq!(report.by_class(ChangeClass::Added).count(), 2);
        assert!(report.has_work());
    }

    #[test]
    fn project_lock_diff_sees_changed_native_digest() {
        let before = ProjectLock {
            format_version: 1,
            project_config_digest: "cfg".into(),
            platform: None,
            adapters: vec![AdapterRef {
                id: "js".into(),
                version: "1".into(),
            }],
            native_locks: vec![NativeLockRef {
                path: "package-lock.json".into(),
                kind: "npm".into(),
                digest: "aaa".into(),
            }],
            artifacts: vec![],
            permitted_outputs: vec!["dist".into()],
        };
        let mut after = before.clone();
        after.native_locks[0].digest = "bbb".into();
        after.permitted_outputs.push("out".into());
        let report = diff_project_locks(&before, &after);
        assert!(
            report
                .entries
                .iter()
                .any(|e| e.node_or_path == "package-lock.json"
                    && e.class == ChangeClass::RequiresWork)
        );
        assert!(report
            .by_class(ChangeClass::Added)
            .any(|e| e.node_or_path == "out"));
    }
}

// ---------------------------------------------------------------------------
// Project-state diff: manifest, npm lock and installed tree, compared against a
// snapshot instead of two receipt files.
// ---------------------------------------------------------------------------

/// One package whose resolved version differs between two lock files. The key is
/// the lock path (`node_modules/a/node_modules/b`), so nested copies of the same
/// name are reported separately instead of being merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// Map lock path -> resolved version. Reads npm lockfile v2/v3 (`packages`) and
/// falls back to v1 (`dependencies`, nested). Returns an error for non-JSON input.
pub fn lock_package_versions(lock_json: &str) -> Result<BTreeMap<String, String>, String> {
    let root: serde_json::Value =
        serde_json::from_str(lock_json).map_err(|e| format!("package-lock.json: {e}"))?;
    let mut out = BTreeMap::new();
    if let Some(pkgs) = root.get("packages").and_then(|p| p.as_object()) {
        for (path, entry) in pkgs {
            if path.is_empty() {
                continue; // the project itself
            }
            if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
                out.insert(path.clone(), v.to_string());
            }
        }
    } else if let Some(deps) = root.get("dependencies").and_then(|d| d.as_object()) {
        fn walk(
            prefix: &str,
            deps: &serde_json::Map<String, serde_json::Value>,
            out: &mut BTreeMap<String, String>,
        ) {
            for (name, entry) in deps {
                let path = format!("{prefix}node_modules/{name}");
                if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
                    out.insert(path.clone(), v.to_string());
                }
                if let Some(nested) = entry.get("dependencies").and_then(|d| d.as_object()) {
                    walk(&format!("{path}/"), nested, out);
                }
            }
        }
        walk("", deps, &mut out);
    }
    Ok(out)
}

/// Package-level changes between two version maps, sorted by path.
pub fn diff_package_versions(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<PackageChange> {
    let mut paths: BTreeSet<&String> = before.keys().collect();
    paths.extend(after.keys());
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .map(|p| PackageChange {
            path: p.clone(),
            before: before.get(p).cloned(),
            after: after.get(p).cloned(),
        })
        .collect()
}

/// Map Cargo.lock text to `crate name -> version`.
///
/// Handles lockfile v3 (`[[package]]` tables). Duplicate crate names (same
/// name from different sources, e.g. crates.io vs git) are keyed
/// `name (source)`; without a source the second occurrence is keyed `name#2`.
pub fn cargo_lock_versions(lock_toml: &str) -> Result<BTreeMap<String, String>, String> {
    let root: toml::Value = toml::from_str(lock_toml).map_err(|e| format!("Cargo.lock: {e}"))?;
    let Some(pkgs) = root.get("package").and_then(|p| p.as_array()) else {
        return Ok(BTreeMap::new());
    };
    let mut entries: Vec<(String, String, Option<String>)> = Vec::new();
    for pkg in pkgs {
        let Some(name) = pkg.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(version) = pkg.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        entries.push((
            name.to_string(),
            version.to_string(),
            pkg.get("source").and_then(|v| v.as_str()).map(String::from),
        ));
    }
    let mut dup_count: BTreeMap<String, usize> = BTreeMap::new();
    for (name, _, _) in &entries {
        *dup_count.entry(name.clone()).or_insert(0) += 1;
    }
    let mut ordinal: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (name, version, source) in entries {
        let mut key = if dup_count[&name] == 1 {
            name
        } else {
            match source {
                Some(s) => format!("{name} ({s})"),
                None => {
                    let n = ordinal.entry(name.clone()).or_insert(1);
                    let key = format!("{name}#{}", *n);
                    *n += 1;
                    key
                }
            }
        };
        // Two entries can still collide (same name and same source — a
        // malformed but possible lock). Suffix instead of silently dropping
        // the first version.
        if out.contains_key(&key) {
            let n = ordinal.entry(key.clone()).or_insert(1);
            key = format!("{key}#{}", *n);
            *n += 1;
        }
        out.insert(key, version);
    }
    Ok(out)
}

/// Map `requirements.txt` text to `package -> pinned version`.
///
/// Only `name==version` pins are collected (PEP 503 name normalization:
/// lowercased, `_`/`.` runs folded to `-`). Comments, options, `-r` includes
/// and unpinned/other-operator requirements are ignored.
pub fn requirements_versions(txt: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in txt.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        // PEP 508 environment markers (`; python_version < "3"`) and extras
        // (`name[extra1,extra2]`) are not part of the pinned identity.
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('-') {
            continue;
        }
        let Some((name, version)) = line.split_once("==") else {
            continue;
        };
        let name = normalize_pip_name(name.trim().split('[').next().unwrap_or("").trim());
        let version = version.trim();
        if name.is_empty() || version.is_empty() {
            continue;
        }
        out.insert(name, version.to_string());
    }
    out
}

/// PEP 503 name normalization: lowercase, runs of `-_.` collapse to `-`.
fn normalize_pip_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_sep = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !last_sep {
                out.push('-');
            }
            last_sep = true;
        } else {
            out.push(ch.to_ascii_lowercase());
            last_sep = false;
        }
    }
    out
}

/// Parse any lock kind the graph crate tracks into a version map.
///
/// `kind` is the native lock kind string used by
/// `jhol_graph::collect_native_locks` (`"npm"`, `"cargo"`, `"pip"`).
pub fn native_lock_versions(kind: &str, text: &str) -> Result<BTreeMap<String, String>, String> {
    match kind {
        "npm" => lock_package_versions(text),
        "cargo" => cargo_lock_versions(text),
        "pip" => Ok(requirements_versions(text)),
        other => Err(format!("unsupported lock kind `{other}`")),
    }
}

/// Text of one project file in a snapshot or on disk. `None` = absent.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileState<'a> {
    pub manifest: Option<&'a str>,
    pub lock: Option<&'a str>,
}

/// Result of comparing two project states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateReport {
    pub manifest: ChangeClass,
    pub lock: ChangeClass,
    pub packages: Vec<PackageChange>,
    /// Installed packages that disagree with the lock (`before` = installed,
    /// `after` = locked). Empty when `node_modules` was not inspected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed: Vec<PackageChange>,
    /// `true` when `jhol sync --apply` must run to make the installed tree match.
    pub needs_sync: bool,
    pub notes: Vec<String>,
}

impl StateReport {
    /// Record the installed-tree drift found by [`installed_drift`].
    pub fn mark_installed(&mut self, drift: Vec<PackageChange>) {
        if !drift.is_empty() {
            self.needs_sync = true;
            self.notes.push(format!(
                "{} installed package(s) differ from the lock",
                drift.len()
            ));
        }
        self.installed = drift;
    }
}

/// Compare the lock's resolved versions with the versions actually installed in
/// `node_modules`. A missing installed copy shows up with `before: None`.
pub fn installed_drift(
    lock_json: &str,
    installed: &BTreeMap<String, String>,
) -> Result<Vec<PackageChange>, String> {
    let locked = lock_package_versions(lock_json)?;
    let mut out: Vec<PackageChange> = locked
        .iter()
        .filter(|(path, want)| installed.get(*path) != Some(*want))
        .map(|(path, want)| PackageChange {
            path: path.clone(),
            before: installed.get(path).cloned(),
            after: Some(want.clone()),
        })
        .collect();
    // The other direction matters too: something installed but absent from
    // the lock is drift the next `ci` would delete.
    for (path, have) in installed {
        if !locked.contains_key(path) {
            out.push(PackageChange {
                path: path.clone(),
                before: Some(have.clone()),
                after: None,
            });
        }
    }
    Ok(out)
}

fn file_class(before: Option<&str>, after: Option<&str>) -> ChangeClass {
    match (before, after) {
        (None, None) => ChangeClass::Unchanged,
        (None, Some(_)) => ChangeClass::Added,
        (Some(_), None) => ChangeClass::Removed,
        (Some(a), Some(b)) if a == b => ChangeClass::Unchanged,
        (Some(_), Some(_)) => ChangeClass::RequiresWork,
    }
}

/// Compare a baseline (`before`, usually a snapshot) with the current project
/// (`after`). `node_modules_present` is whether the installed tree exists now.
pub fn diff_state(
    before: FileState<'_>,
    after: FileState<'_>,
    node_modules_present: bool,
) -> StateReport {
    let manifest = file_class(before.manifest, after.manifest);
    let mut notes = Vec::new();
    let (lock, packages) = match (before.lock, after.lock) {
        (Some(b), Some(a)) => match (lock_package_versions(b), lock_package_versions(a)) {
            (Ok(bv), Ok(av)) => {
                let changes = diff_package_versions(&bv, &av);
                let class = if changes.is_empty() {
                    ChangeClass::Unchanged
                } else {
                    ChangeClass::RequiresWork
                };
                (class, changes)
            }
            (Err(e), _) | (_, Err(e)) => {
                notes.push(format!(
                    "lock could not be compared package by package: {e}"
                ));
                (file_class(Some(b), Some(a)), Vec::new())
            }
        },
        (b, a) => (file_class(b, a), Vec::new()),
    };
    let needs_sync = manifest.is_change() || lock.is_change() || !node_modules_present;
    if !node_modules_present {
        notes.push("node_modules is missing or not installed; run `jhol sync --apply`".into());
    }
    StateReport {
        manifest,
        lock,
        packages,
        needs_sync,
        installed: Vec::new(),
        notes,
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    const LOCK_V3_A: &str = r#"{"lockfileVersion":3,"packages":{
        "":{"name":"app"},
        "node_modules/react":{"version":"18.2.0"},
        "node_modules/loose-envify":{"version":"1.4.0"},
        "node_modules/a/node_modules/loose-envify":{"version":"1.0.0"}}}"#;

    const LOCK_V3_B: &str = r#"{"lockfileVersion":3,"packages":{
        "":{"name":"app"},
        "node_modules/react":{"version":"18.3.1"},
        "node_modules/loose-envify":{"version":"1.4.0"},
        "node_modules/scheduler":{"version":"0.23.2"}}}"#;

    #[test]
    fn reads_v3_packages_and_skips_root() {
        let m = lock_package_versions(LOCK_V3_A).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m["node_modules/react"], "18.2.0");
        assert!(!m.contains_key(""));
    }

    #[test]
    fn reads_v1_nested_dependencies() {
        let v1 = r#"{"lockfileVersion":1,"dependencies":{"react":{"version":"18.2.0",
            "dependencies":{"loose-envify":{"version":"1.4.0"}}}}}"#;
        let m = lock_package_versions(v1).unwrap();
        assert_eq!(m["node_modules/react"], "18.2.0");
        assert_eq!(m["node_modules/react/node_modules/loose-envify"], "1.4.0");
    }

    #[test]
    fn package_changes_are_path_keyed_and_sorted() {
        let changes = diff_package_versions(
            &lock_package_versions(LOCK_V3_A).unwrap(),
            &lock_package_versions(LOCK_V3_B).unwrap(),
        );
        let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "node_modules/a/node_modules/loose-envify",
                "node_modules/react",
                "node_modules/scheduler",
            ]
        );
        let react = changes
            .iter()
            .find(|c| c.path == "node_modules/react")
            .unwrap();
        assert_eq!(react.before.as_deref(), Some("18.2.0"));
        assert_eq!(react.after.as_deref(), Some("18.3.1"));
        let scheduler = changes
            .iter()
            .find(|c| c.path == "node_modules/scheduler")
            .unwrap();
        assert_eq!(scheduler.before, None, "added package has no prior version");
    }

    #[test]
    fn identical_state_is_unchanged_and_needs_no_sync() {
        let manifest = r#"{"dependencies":{"react":"18.2.0"}}"#;
        let st = FileState {
            manifest: Some(manifest),
            lock: Some(LOCK_V3_A),
        };
        let r = diff_state(st, st, true);
        assert_eq!(r.manifest, ChangeClass::Unchanged);
        assert_eq!(r.lock, ChangeClass::Unchanged);
        assert!(r.packages.is_empty());
        assert!(!r.needs_sync);
    }

    #[test]
    fn lock_edit_requires_sync_and_lists_packages() {
        let before = FileState {
            manifest: Some("m"),
            lock: Some(LOCK_V3_A),
        };
        let after = FileState {
            manifest: Some("m"),
            lock: Some(LOCK_V3_B),
        };
        let r = diff_state(before, after, true);
        assert_eq!(r.lock, ChangeClass::RequiresWork);
        assert_eq!(r.manifest, ChangeClass::Unchanged);
        assert_eq!(r.packages.len(), 3);
        assert!(r.needs_sync);
    }

    #[test]
    fn missing_node_modules_requires_sync_even_when_files_match() {
        let st = FileState {
            manifest: Some("m"),
            lock: Some(LOCK_V3_A),
        };
        let r = diff_state(st, st, false);
        assert!(r.needs_sync);
        assert!(r.notes.iter().any(|n| n.contains("jhol sync --apply")));
    }

    #[test]
    fn installed_tree_that_disagrees_with_lock_needs_sync() {
        // The lock says react 18.2.0 (restored from a snapshot), but the tree on
        // disk still has 18.3.1 and lacks scheduler. Files alone look unchanged.
        let mut installed = BTreeMap::new();
        installed.insert("node_modules/react".to_string(), "18.3.1".to_string());
        installed.insert("node_modules/loose-envify".to_string(), "1.4.0".to_string());
        let drift = installed_drift(LOCK_V3_A, &installed).unwrap();
        let react = drift
            .iter()
            .find(|c| c.path == "node_modules/react")
            .unwrap();
        assert_eq!(react.before.as_deref(), Some("18.3.1"));
        assert_eq!(react.after.as_deref(), Some("18.2.0"));
        let missing = drift
            .iter()
            .find(|c| c.path == "node_modules/a/node_modules/loose-envify")
            .unwrap();
        assert_eq!(
            missing.before, None,
            "missing copy has no installed version"
        );

        let st = FileState {
            manifest: Some("m"),
            lock: Some(LOCK_V3_A),
        };
        let mut r = diff_state(st, st, true);
        assert!(
            !r.needs_sync,
            "files match, so only the tree can say sync is needed"
        );
        r.mark_installed(drift);
        assert!(r.needs_sync);
        assert!(r.notes.iter().any(|n| n.contains("differ from the lock")));
    }

    #[test]
    fn matching_installed_tree_has_no_drift() {
        let mut installed = BTreeMap::new();
        installed.insert("node_modules/react".to_string(), "18.2.0".to_string());
        installed.insert("node_modules/loose-envify".to_string(), "1.4.0".to_string());
        installed.insert(
            "node_modules/a/node_modules/loose-envify".to_string(),
            "1.0.0".to_string(),
        );
        assert!(installed_drift(LOCK_V3_A, &installed).unwrap().is_empty());
    }

    #[test]
    fn unparseable_lock_is_reported_not_panicked() {
        let before = FileState {
            manifest: None,
            lock: Some("{not json"),
        };
        let after = FileState {
            manifest: None,
            lock: Some(LOCK_V3_A),
        };
        let r = diff_state(before, after, true);
        assert_eq!(r.lock, ChangeClass::RequiresWork);
        assert!(r.notes.iter().any(|n| n.contains("package by package")));
    }

    #[test]
    fn cargo_lock_versions_parses_v3() {
        let lock = r#"# This file is automatically @generated by Cargo.
version = 3

[[package]]
name = "serde"
version = "1.0.210"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "myapp"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.999"
source = "git+https://example.com/serde"
"#;
        let m = cargo_lock_versions(lock).unwrap();
        assert_eq!(
            m["serde (registry+https://github.com/rust-lang/crates.io-index)"],
            "1.0.210"
        );
        assert_eq!(m["myapp"], "0.1.0");
        // Duplicate name from a second source is disambiguated, not lost.
        assert_eq!(m["serde (git+https://example.com/serde)"], "1.0.999");
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn cargo_lock_versions_unique_names_keep_plain_keys() {
        let lock = "[[package]]\nname = \"only\"\nversion = \"1.2.3\"\n";
        let m = cargo_lock_versions(lock).unwrap();
        assert_eq!(m["only"], "1.2.3");
    }

    #[test]
    fn cargo_lock_versions_rejects_garbage() {
        assert!(cargo_lock_versions("not toml {{{").is_err());
        assert!(cargo_lock_versions("").unwrap().is_empty());
    }

    #[test]
    fn requirements_versions_strips_extras_and_markers() {
        let m = requirements_versions(
            "Django[bcrypt]==4.2 ; python_version < \"3.12\"\ncryptography==41.0.7 # pin\n",
        );
        assert_eq!(m.get("django").map(String::as_str), Some("4.2"));
        assert_eq!(m.get("cryptography").map(String::as_str), Some("41.0.7"));
        assert_eq!(m.len(), 2, "extras/markers must not leak into keys: {m:?}");
    }

    #[test]
    fn installed_drift_flags_extras() {
        let lock = r#"{"lockfileVersion":3,"packages":{
            "node_modules/a":{"version":"1.0.0"},
            "node_modules/b":{"version":"2.0.0"}
        }}"#;
        let mut installed = std::collections::BTreeMap::new();
        installed.insert("node_modules/a".to_string(), "1.0.0".to_string());
        installed.insert("node_modules/extra".to_string(), "9.9.9".to_string());
        let drift = installed_drift(lock, &installed).unwrap();
        assert!(drift
            .iter()
            .any(|c| c.path == "node_modules/b" && c.before.is_none()));
        assert!(
            drift
                .iter()
                .any(|c| c.path == "node_modules/extra" && c.after.is_none()),
            "installed-but-unlocked packages must be flagged: {drift:?}"
        );
    }

    #[test]
    fn requirements_versions_pins_and_normalizes() {
        let txt = "# comment
Flask==2.3.1
requests >= 2.0
Django_Extras==1.0
-r other.txt
--index-url https://example.com
zope.interface==5.4.0
numpy==1.26.4  # inline comment
";
        let m = requirements_versions(txt);
        assert_eq!(m.get("flask").map(String::as_str), Some("2.3.1"));
        assert_eq!(m.get("django-extras").map(String::as_str), Some("1.0"));
        assert_eq!(m.get("zope-interface").map(String::as_str), Some("5.4.0"));
        assert_eq!(m.get("numpy").map(String::as_str), Some("1.26.4"));
        assert!(!m.contains_key("requests"), "unpinned requirement ignored");
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn native_lock_versions_dispatches_by_kind() {
        let npm = native_lock_versions(
            "npm",
            r#"{"packages":{"node_modules/x":{"version":"1.0.0"}}}"#,
        )
        .unwrap();
        assert_eq!(npm["node_modules/x"], "1.0.0");
        let pip = native_lock_versions("pip", "a==1.0\n").unwrap();
        assert_eq!(pip["a"], "1.0");
        let cargo = native_lock_versions("cargo", "[[package]]\nname = \"c\"\nversion = \"2.0\"\n")
            .unwrap();
        assert_eq!(cargo["c"], "2.0");
        assert!(native_lock_versions("poetry", "").is_err());
    }
}
