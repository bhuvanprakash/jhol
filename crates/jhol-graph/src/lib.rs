//! Typed cross-ecosystem dependency / task graph (blueprint §5).
//!
//! This crate owns relationships *between* adapter outcomes and tasks.
//! It does **not** unify npm semver, PEP 440 and Cargo into one version order.

#![forbid(unsafe_code)]

pub mod action_cache;
pub mod project_lock;

pub use action_cache::{action_key, lookup as action_cache_lookup, store as action_cache_store};
pub use project_lock::{
    build_project_lock, collect_native_locks, digest_bytes, digest_file, native_locks_match,
    read_project_lock, write_project_lock, AdapterRef, NativeLockRef, ProjectLock,
    ProjectLockError, PROJECT_LOCK_FILENAME, PROJECT_LOCK_FORMAT,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

/// Identity schema version folded into every [`NodeId`].
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Project,
    PackageEnvironment,
    PackageArtifact,
    Toolchain,
    SourceTree,
    BuildAction,
    TestAction,
    RunService,
    DatabaseSchema,
    Migration,
    ModelArtifact,
    DatasetArtifact,
    GeneratedClient,
    OutputArtifact,
    Policy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Requires,
    BuildRequires,
    UsesToolchain,
    Consumes,
    Produces,
    NeedsService,
    NeedsSchema,
    InvalidatedBy,
    GeneratedFrom,
    OptionalUnder,
    PinnedTo,
}

/// Content-addressed node identity:
/// `Hash(schema_version, node_kind, normalized_external_identity, platform, digest_or_lock)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(String);

fn hash_node_part(hash: &mut Sha256, part: &[u8]) {
    hash.update((part.len() as u64).to_le_bytes());
    hash.update(part);
}

impl NodeId {
    pub fn compute(
        kind: NodeKind,
        external_identity: &str,
        platform: &str,
        digest_or_lock: &str,
    ) -> Self {
        let mut h = Sha256::new();
        h.update(SCHEMA_VERSION.to_le_bytes());
        let kind = format!("{kind:?}");
        hash_node_part(&mut h, kind.as_bytes());
        hash_node_part(&mut h, external_identity.as_bytes());
        hash_node_part(&mut h, platform.as_bytes());
        hash_node_part(&mut h, digest_or_lock.as_bytes());
        NodeId(hex::encode(h.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: HashMap<String, Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    #[error("cycle detected involving node {0}")]
    Cycle(String),
    #[error("unknown node {0}")]
    UnknownNode(String),
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_node(&mut self, node: Node) {
        self.nodes.insert(node.id.as_str().to_string(), node);
    }

    pub fn add_edge(&mut self, edge: Edge) -> Result<(), GraphError> {
        if !self.nodes.contains_key(edge.from.as_str()) {
            return Err(GraphError::UnknownNode(edge.from.as_str().to_string()));
        }
        if !self.nodes.contains_key(edge.to.as_str()) {
            return Err(GraphError::UnknownNode(edge.to.as_str().to_string()));
        }
        self.edges.push(edge);
        Ok(())
    }

    /// Ordinary DAG edges only (Requires / BuildRequires / Consumes / Produces / …).
    /// Service readiness cycles are *not* validated here (blueprint §5.2).
    pub fn validate_acyclic(&self) -> Result<(), GraphError> {
        let mut indeg: HashMap<&str, usize> = self.nodes.keys().map(|k| (k.as_str(), 0)).collect();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &self.edges {
            if matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder) {
                continue;
            }
            *indeg.entry(e.to.as_str()).or_default() += 1;
            adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
        }
        let mut q: VecDeque<&str> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| *k)
            .collect();
        let mut seen = 0usize;
        while let Some(n) = q.pop_front() {
            seen += 1;
            if let Some(outs) = adj.get(n) {
                for t in outs {
                    let d = indeg.get_mut(*t).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        q.push_back(*t);
                    }
                }
            }
        }
        if seen != self.nodes.len() {
            let bad = indeg
                .into_iter()
                .filter(|(_, d)| *d > 0)
                .map(|(k, _)| k.to_string())
                .collect::<HashSet<_>>();
            let first = bad.into_iter().next().unwrap_or_else(|| "unknown".into());
            return Err(GraphError::Cycle(first));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_is_stable() {
        let a = NodeId::compute(NodeKind::Project, "atlas", "darwin-arm64", "cfg1");
        let b = NodeId::compute(NodeKind::Project, "atlas", "darwin-arm64", "cfg1");
        assert_eq!(a, b);
        let c = NodeId::compute(NodeKind::Project, "atlas", "linux-x64", "cfg1");
        assert_ne!(a, c);
    }

    #[test]
    fn node_id_encodes_component_boundaries() {
        let a = NodeId::compute(NodeKind::BuildAction, "ab", "c", "");
        let b = NodeId::compute(NodeKind::BuildAction, "a", "bc", "");
        assert_ne!(a, b);
    }

    #[test]
    fn detects_cycle() {
        let mut g = Graph::new();
        let n1 = NodeId::compute(NodeKind::BuildAction, "a", "", "");
        let n2 = NodeId::compute(NodeKind::BuildAction, "b", "", "");
        g.insert_node(Node {
            id: n1.clone(),
            kind: NodeKind::BuildAction,
            label: "a".into(),
        });
        g.insert_node(Node {
            id: n2.clone(),
            kind: NodeKind::BuildAction,
            label: "b".into(),
        });
        g.add_edge(Edge {
            from: n1.clone(),
            to: n2.clone(),
            kind: EdgeKind::Requires,
        })
        .unwrap();
        g.add_edge(Edge {
            from: n2,
            to: n1,
            kind: EdgeKind::Requires,
        })
        .unwrap();
        assert!(matches!(g.validate_acyclic(), Err(GraphError::Cycle(_))));
    }
}
