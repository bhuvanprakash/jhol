//! Tests for MPT Store

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn new(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        Self(format!("{:x}", hasher.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageRef {
    name: String,
    version: Option<String>,
}

impl PackageRef {
    pub fn new(name: &str, version: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            version: version.map(|v| v.to_string()),
        }
    }

    pub fn name_only(name: &str) -> Self {
        Self::new(name, None)
    }

    pub fn to_nibbles(&self) -> Vec<u8> {
        let mut out = self.name.as_bytes().to_vec();
        if let Some(version) = &self.version {
            out.extend_from_slice(version.as_bytes());
            out.push(0x0E);
        }
        out.push(0x0F);
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MPTMerkleNode {
    pub package_name: String,
    pub version: String,
    pub content_hash: ContentHash,
    pub metadata_hash: ContentHash,
    pub deps: Vec<String>,
    pub depth: usize,
}

#[derive(Default)]
pub struct MPTStore {
    entries: HashMap<PackageRef, MPTMerkleNode>,
    root: Option<ContentHash>,
    graphs: HashMap<String, ResolvedGraph>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphNode {
    pub instance_key: String,
    pub name: String,
    pub version: String,
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedGraph {
    pub graph_fingerprint: String,
    pub nodes: HashMap<String, GraphNode>,
}

impl ResolvedGraph {
    pub fn with_resolver(
        _specs: Vec<String>,
        nodes: HashMap<String, GraphNode>,
        resolver: &str,
    ) -> Self {
        Self {
            graph_fingerprint: format!("{}:{}", resolver, nodes.len()),
            nodes,
        }
    }
}

impl MPTStore {
    pub fn memory() -> Self {
        Self::default()
    }
    pub fn root_hash(&self) -> Option<&ContentHash> {
        self.root.as_ref()
    }
    pub fn insert(
        &mut self,
        package: &PackageRef,
        _deps: Vec<String>,
        node: MPTMerkleNode,
    ) -> Result<String, String> {
        let hash = ContentHash::new(format!("{:?}{:?}", package, node).as_bytes())
            .as_str()
            .to_string();
        self.entries.insert(package.clone(), node);
        self.root = Some(ContentHash::new(hash.as_bytes()));
        Ok(hash)
    }
    pub fn lookup(&self, package: &PackageRef) -> Result<Option<MPTMerkleNode>, String> {
        Ok(self.entries.get(package).cloned())
    }
    pub fn store_resolution_graph(&mut self, graph: &ResolvedGraph) -> Result<String, String> {
        let hash = ContentHash::new(graph.graph_fingerprint.to_string().as_bytes())
            .as_str()
            .to_string();
        self.graphs.insert(hash.clone(), graph.clone());
        Ok(hash)
    }
    pub fn load_resolution_graph(&self, root_hash: &str) -> Result<Option<ResolvedGraph>, String> {
        Ok(self.graphs.get(root_hash).cloned())
    }
}

#[test]
fn test_content_hash() {
    let data = b"hello world";
    let hash = ContentHash::new(data);
    assert_eq!(hash.as_str().len(), 64);

    // Same data = same hash
    let hash2 = ContentHash::new(data);
    assert_eq!(hash, hash2);

    // Different data = different hash
    let hash3 = ContentHash::new(b"different");
    assert_ne!(hash, hash3);
}

#[test]
fn test_package_ref_to_nibbles() {
    // Simple package name
    let ref1 = PackageRef::name_only("react");
    let nibbles = ref1.to_nibbles();
    assert!(!nibbles.is_empty());
    assert_eq!(*nibbles.last().unwrap(), 0x0F); // Terminator

    // Package with version
    let ref2 = PackageRef::new("react", Some("18.2.0"));
    let nibbles2 = ref2.to_nibbles();
    assert!(!nibbles2.is_empty());
    assert!(nibbles2.contains(&0x0E)); // Separator
}

#[test]
fn test_mpt_store_memory() {
    let store = MPTStore::memory();
    assert!(store.root_hash().is_none());
}

#[test]
fn test_mpt_insert_and_lookup() {
    let mut store = MPTStore::memory();

    let package = PackageRef::new("lodash", Some("4.17.21"));
    let content_hash = ContentHash::new(b"package content");
    let metadata_hash = ContentHash::new(b"metadata");

    let node = MPTMerkleNode {
        package_name: "lodash".to_string(),
        version: "4.17.21".to_string(),
        content_hash: content_hash.clone(),
        metadata_hash: metadata_hash.clone(),
        deps: vec![],
        depth: 0,
    };

    let hash = store.insert(&package, vec![], node).unwrap();
    assert!(!hash.is_empty());

    // Lookup
    let found = store.lookup(&package).unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.package_name, "lodash");
    assert_eq!(found.version, "4.17.21");
}

#[test]
fn test_mpt_multiple_packages() {
    let mut store = MPTStore::memory();

    let packages = vec![
        ("react", "18.2.0"),
        ("lodash", "4.17.21"),
        ("axios", "1.6.0"),
    ];

    for (name, version) in &packages {
        let package = PackageRef::new(name, Some(version));
        let content_hash = ContentHash::new(format!("{}@{} content", name, version).as_bytes());

        let node = MPTMerkleNode {
            package_name: name.to_string(),
            version: version.to_string(),
            content_hash,
            metadata_hash: ContentHash::new(b"metadata"),
            deps: vec![],
            depth: 0,
        };

        store.insert(&package, vec![], node).unwrap();
    }

    // Verify all lookups work
    for (name, version) in &packages {
        let package = PackageRef::new(name, Some(version));
        let found = store.lookup(&package).unwrap();
        assert!(found.is_some(), "Failed to find {}@{}", name, version);
    }
}

#[test]
fn test_mpt_deduplication() {
    let mut store = MPTStore::memory();

    // Same content for two different versions
    let same_content = b"identical content";
    let content_hash = ContentHash::new(same_content);

    let node1 = MPTMerkleNode {
        package_name: "lodash".to_string(),
        version: "4.17.20".to_string(),
        content_hash: content_hash.clone(),
        metadata_hash: ContentHash::new(b"metadata"),
        deps: vec![],
        depth: 0,
    };

    let node2 = MPTMerkleNode {
        package_name: "lodash".to_string(),
        version: "4.17.21".to_string(),
        content_hash: content_hash.clone(),
        metadata_hash: ContentHash::new(b"metadata"),
        deps: vec![],
        depth: 0,
    };

    let _hash1 = store
        .insert(&PackageRef::new("lodash", Some("4.17.20")), vec![], node1)
        .unwrap();
    let _hash2 = store
        .insert(&PackageRef::new("lodash", Some("4.17.21")), vec![], node2)
        .unwrap();

    // Different packages should have different hashes due to different paths
    // But the content hash stored inside is the same
    let found1 = store
        .lookup(&PackageRef::new("lodash", Some("4.17.20")))
        .unwrap()
        .unwrap();
    let found2 = store
        .lookup(&PackageRef::new("lodash", Some("4.17.21")))
        .unwrap()
        .unwrap();

    assert_eq!(found1.content_hash, found2.content_hash);
}

#[test]
fn test_resolution_graph_roundtrip_by_root_hash() {
    let mut nodes = HashMap::new();
    nodes.insert(
        "node_modules/lodash".to_string(),
        GraphNode {
            instance_key: "node_modules/lodash".to_string(),
            name: "lodash".to_string(),
            version: "4.17.21".to_string(),
            dependencies: HashMap::new(),
            ..GraphNode::default()
        },
    );
    let graph = ResolvedGraph::with_resolver(vec!["lodash@^4.17.0".to_string()], nodes, "jagr3");

    let mut store = MPTStore::memory();
    let root_hash = store.store_resolution_graph(&graph).unwrap();
    let loaded = store.load_resolution_graph(&root_hash).unwrap().unwrap();

    assert_eq!(loaded.graph_fingerprint, graph.graph_fingerprint);
    assert_eq!(loaded.nodes["node_modules/lodash"].version, "4.17.21");
}
