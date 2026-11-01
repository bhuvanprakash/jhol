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

    /// Kahn topological order over ordinary DAG edges, deterministic: ready
    /// nodes are emitted in ascending `NodeId` order so the same graph always
    /// yields the same build schedule. `NeedsService` / `OptionalUnder` edges
    /// are ignored, matching [`Graph::validate_acyclic`].
    pub fn toposort(&self) -> Result<Vec<NodeId>, GraphError> {
        let mut indeg: HashMap<&str, usize> = self.nodes.keys().map(|k| (k.as_str(), 0)).collect();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &self.edges {
            if matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder) {
                continue;
            }
            *indeg.entry(e.to.as_str()).or_default() += 1;
            adj.entry(e.to.as_str()).or_default(); // keep sinks in adj for symmetry
            adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
        }
        let mut ready: Vec<&str> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| *k)
            .collect();
        ready.sort_unstable();
        let mut out = Vec::with_capacity(self.nodes.len());
        // `ready` is kept sorted ascending (see the partition_point insert
        // below), so the next node is the FIRST element — `pop()` would take
        // the largest id and emit the whole order descending.
        while let Some(n) = ready.first().copied() {
            ready.remove(0);
            out.push(NodeId(n.to_string()));
            if let Some(outs) = adj.get(n) {
                for t in outs {
                    let d = indeg.get_mut(*t).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        // `partition_point` returns the first index where the
                        // predicate is false; elements < t form the true prefix of
                        // an ascending vec, so this lands t at its sorted spot.
                        let pos = ready.partition_point(|c| *c < *t);
                        ready.insert(pos, *t);
                    }
                }
            }
        }
        if out.len() != self.nodes.len() {
            return Err(self.any_cycle_node());
        }
        Ok(out)
    }

    /// Direct dependencies of a node (ordinary edges only), deduplicated and
    /// sorted for determinism.
    pub fn deps_of(&self, id: &NodeId) -> Vec<NodeId> {
        let mut out: Vec<&str> = self
            .edges
            .iter()
            .filter(|e| {
                e.from == *id && !matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder)
            })
            .map(|e| e.to.as_str())
            .collect();
        out.sort_unstable();
        out.dedup();
        out.into_iter().map(|s| NodeId(s.to_string())).collect()
    }

    /// Direct dependents (reverse dependencies), deduplicated and sorted.
    pub fn dependents_of(&self, id: &NodeId) -> Vec<NodeId> {
        let mut out: Vec<&str> = self
            .edges
            .iter()
            .filter(|e| {
                e.to == *id && !matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder)
            })
            .map(|e| e.from.as_str())
            .collect();
        out.sort_unstable();
        out.dedup();
        out.into_iter().map(|s| NodeId(s.to_string())).collect()
    }

    /// Everything transitively reachable from `id` through ordinary edges
    /// (exclusive of `id` itself). Self-loops and shared descendants are
    /// visited once.
    pub fn reachable_from(&self, id: &NodeId) -> HashSet<NodeId> {
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<&str> = vec![id.as_str()];
        while let Some(n) = stack.pop() {
            for e in &self.edges {
                if e.from.as_str() != n
                    || matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder)
                {
                    continue;
                }
                if seen.insert(NodeId(e.to.as_str().to_string())) {
                    stack.push(e.to.as_str());
                }
            }
        }
        seen
    }

    /// One concrete dependency cycle as a node path `[a, b, …, a]`, or `None`
    /// when the ordinary-edge subgraph is acyclic. Deterministic: DFS explores
    /// targets in ascending id order and reports the first cycle found.
    pub fn cycle_path(&self) -> Option<Vec<NodeId>> {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &self.edges {
            if matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder) {
                continue;
            }
            adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
        }
        for v in adj.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        // Iterative DFS with an explicit path; WHITE/GRAY/BLACK coloring.
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut color: HashMap<&str, Color> = self
            .nodes
            .keys()
            .map(|k| (k.as_str(), Color::White))
            .collect();
        let mut roots: Vec<&str> = self.nodes.keys().map(|k| k.as_str()).collect();
        roots.sort_unstable();
        for root in roots {
            if color.get(root) != Some(&Color::White) {
                continue;
            }
            let mut path: Vec<&str> = vec![root];
            let mut iters: Vec<std::slice::Iter<'_, &str>> =
                vec![adj.get(root).map(|v| v.iter()).unwrap_or_default()];
            color.insert(root, Color::Gray);
            while let Some(top) = path.last() {
                let next = iters
                    .last_mut()
                    .expect("iterator stack matches path")
                    .next()
                    .copied();
                match next {
                    None => {
                        color.insert(path.pop().expect("path non-empty"), Color::Black);
                        iters.pop();
                    }
                    Some(n) => match color.get(n) {
                        Some(Color::Gray) => {
                            // Found a cycle: slice the path from n's position.
                            let start = path
                                .iter()
                                .position(|p| *p == n)
                                .expect("gray node on path");
                            let mut cyc: Vec<NodeId> = path[start..]
                                .iter()
                                .map(|s| NodeId((*s).to_string()))
                                .collect();
                            cyc.push(NodeId(n.to_string()));
                            return Some(cyc);
                        }
                        Some(Color::White) => {
                            color.insert(n, Color::Gray);
                            path.push(n);
                            iters.push(adj.get(n).map(|v| v.iter()).unwrap_or_default());
                        }
                        _ => {}
                    },
                }
            }
        }
        None
    }

    /// The induced subgraph over `ids`: nodes in the set plus edges whose both
    /// endpoints are in the set. Unknown ids are ignored so callers can pass
    /// supersets (e.g. "everything reachable from X").
    pub fn subgraph(&self, ids: &[NodeId]) -> Graph {
        let set: HashSet<&str> = ids.iter().map(|i| i.as_str()).collect();
        let mut out = Graph::new();
        for (k, n) in &self.nodes {
            if set.contains(k.as_str()) {
                out.nodes.insert(k.clone(), n.clone());
            }
        }
        for e in &self.edges {
            if set.contains(e.from.as_str()) && set.contains(e.to.as_str()) {
                out.edges.push(e.clone());
            }
        }
        out
    }

    fn any_cycle_node(&self) -> GraphError {
        let mut indeg: HashMap<&str, usize> = self.nodes.keys().map(|k| (k.as_str(), 0)).collect();
        for e in &self.edges {
            if matches!(e.kind, EdgeKind::NeedsService | EdgeKind::OptionalUnder) {
                continue;
            }
            *indeg.entry(e.to.as_str()).or_default() += 1;
        }
        let bad = indeg
            .into_iter()
            .filter(|(_, d)| *d > 0)
            .map(|(k, _)| k.to_string())
            .collect::<HashSet<_>>();
        let first = bad.into_iter().next().unwrap_or_else(|| "unknown".into());
        GraphError::Cycle(first)
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

    fn node(id: &str) -> Node {
        Node {
            id: NodeId(id.to_string()),
            kind: NodeKind::BuildAction,
            label: id.to_string(),
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: NodeId(from.to_string()),
            to: NodeId(to.to_string()),
            kind: EdgeKind::Requires,
        }
    }

    /// a -> b -> c  (ids chosen so sorting is predictable)
    fn chain_graph() -> Graph {
        let mut g = Graph::new();
        g.insert_node(node("aa"));
        g.insert_node(node("bb"));
        g.insert_node(node("cc"));
        g.add_edge(edge("aa", "bb")).unwrap();
        g.add_edge(edge("bb", "cc")).unwrap();
        g
    }

    #[test]
    fn toposort_is_deterministic_and_respects_edges() {
        let g = chain_graph();
        let order = g.toposort().unwrap();
        let ids: Vec<&str> = order.iter().map(|i| i.as_str()).collect();
        assert_eq!(ids, ["aa", "bb", "cc"]);
        // Same graph built in reverse insertion order gives the same schedule.
        let mut g2 = Graph::new();
        g2.insert_node(node("cc"));
        g2.insert_node(node("bb"));
        g2.insert_node(node("aa"));
        g2.add_edge(edge("aa", "bb")).unwrap();
        g2.add_edge(edge("bb", "cc")).unwrap();
        assert_eq!(g2.toposort().unwrap(), order);
    }

    #[test]
    fn toposort_rejects_cycles() {
        let mut g = chain_graph();
        g.add_edge(edge("cc", "aa")).unwrap();
        assert!(matches!(g.toposort(), Err(GraphError::Cycle(_))));
    }

    #[test]
    fn service_edges_do_not_affect_order() {
        let mut g = chain_graph();
        // A service "needs" edge pointing backwards would be a cycle if
        // counted; it must be ignored like in validate_acyclic.
        g.add_edge(Edge {
            from: NodeId("cc".into()),
            to: NodeId("aa".into()),
            kind: EdgeKind::NeedsService,
        })
        .unwrap();
        assert_eq!(g.toposort().unwrap().len(), 3);
        g.validate_acyclic().unwrap();
    }

    #[test]
    fn deps_and_dependents_are_direct_and_sorted() {
        let mut g = chain_graph();
        g.add_edge(edge("aa", "cc")).unwrap();
        let deps = g.deps_of(&NodeId("aa".into()));
        let deps: Vec<&str> = deps.iter().map(|i| i.as_str()).collect();
        assert_eq!(deps, ["bb", "cc"]);
        let rdeps = g.dependents_of(&NodeId("cc".into()));
        let rdeps: Vec<&str> = rdeps.iter().map(|i| i.as_str()).collect();
        assert_eq!(rdeps, ["aa", "bb"]);
        assert!(g.deps_of(&NodeId("cc".into())).is_empty());
    }

    #[test]
    fn reachability_is_transitive_and_exclusive() {
        let g = chain_graph();
        let r = g.reachable_from(&NodeId("aa".into()));
        assert_eq!(r.len(), 2);
        assert!(r.contains(&NodeId("cc".into())));
        assert!(g.reachable_from(&NodeId("cc".into())).is_empty());
    }

    #[test]
    fn cycle_path_reports_the_actual_loop() {
        let mut g = chain_graph();
        g.add_edge(edge("cc", "bb")).unwrap();
        let path = g.cycle_path().expect("cycle exists");
        let ids: Vec<&str> = path.iter().map(|i| i.as_str()).collect();
        // Starts and ends on the same node and follows real edges.
        assert_eq!(ids.first(), ids.last());
        assert!(ids.len() >= 3);
        for w in ids.windows(2) {
            assert!(
                g.edges
                    .iter()
                    .any(|e| e.from.as_str() == w[0] && e.to.as_str() == w[1]),
                "path edge {} -> {} is not a real edge",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn cycle_path_is_none_for_dag() {
        assert!(chain_graph().cycle_path().is_none());
    }

    #[test]
    fn subgraph_induces_nodes_and_edges() {
        let mut g = chain_graph();
        g.insert_node(node("dd"));
        let sub = g.subgraph(&[
            NodeId("aa".into()),
            NodeId("bb".into()),
            NodeId("dd".into()),
        ]);
        assert_eq!(sub.nodes.len(), 3);
        assert_eq!(sub.edges.len(), 1, "aa->bb kept (both ends in set)");
        let sub2 = g.subgraph(&[
            NodeId("aa".into()),
            NodeId("bb".into()),
            NodeId("cc".into()),
        ]);
        assert_eq!(sub2.edges.len(), 2);
        // Dropping an intermediate node drops the edges through it.
        let sub3 = g.subgraph(&[NodeId("aa".into()), NodeId("cc".into())]);
        assert!(sub3.edges.is_empty());
        // Unknown ids are ignored, not errors.
        let sub3 = g.subgraph(&[NodeId("zz".into())]);
        assert!(sub3.nodes.is_empty());
    }

    #[test]
    fn toposort_emits_ascending_node_ids() {
        // Edges inserted in an order that would make a LIFO ready queue emit
        // descending ids; the documented contract is ascending.
        let mut g = Graph::new();
        for n in ["d", "a", "c", "b"] {
            g.insert_node(Node {
                id: NodeId(n.into()),
                kind: NodeKind::Project,
                label: n.into(),
            });
        }
        for (f, t) in [("a", "d"), ("a", "c"), ("b", "d")] {
            g.add_edge(Edge {
                from: NodeId(f.into()),
                to: NodeId(t.into()),
                kind: EdgeKind::Requires,
            })
            .unwrap();
        }
        let order = g.toposort().unwrap();
        assert_eq!(
            order,
            vec![
                NodeId("a".into()),
                NodeId("b".into()),
                NodeId("c".into()),
                NodeId("d".into())
            ]
        );
    }
}
