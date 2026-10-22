//! Red-Green Dependency Fingerprinting (Fast Incremental Re-Install)
//!
//! Applies the rustc-style red-green algorithm to detect unchanged subgraphs via 128-bit
//! SHA-256 fingerprints. Unchanged subtrees are colored Green and bypass the solver entirely.
//! Fine-grained graph differential mutations live in nested `incremental_resolver` (was standalone file).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// 128-bit fingerprint for change detection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint(u64, u64);

impl Fingerprint {
    pub fn compute(data: &[u8]) -> Self {
        let hash = Sha256::digest(data);
        let left = u64::from_le_bytes(hash[0..8].try_into().unwrap());
        let right = u64::from_le_bytes(hash[8..16].try_into().unwrap());
        Fingerprint(left, right)
    }

    pub fn zero() -> Self {
        Fingerprint(0, 0)
    }
}

/// Dependency graph node color (red-green algorithm)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeColor {
    Green, // Unchanged, can reuse cached result
    Red,   // Changed, must recompute
}

/// Dependency graph node
#[derive(Debug, Clone)]
pub struct DepGraphNode {
    pub key: String,
    pub fingerprint: Fingerprint,
    pub color: NodeColor,
    pub dependencies: Vec<String>,
    pub cached_result: Option<ResolutionResult>,
}

/// Resolution result cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionResult {
    pub resolved_version: String,
    pub dependencies: Vec<String>,
    pub integrity: String,
}

/// Incremental resolver with caching
pub struct IncrementalResolver {
    graph: HashMap<String, DepGraphNode>,
    cache: HashMap<Fingerprint, ResolutionResult>,
    changed_packages: Vec<String>,
}

impl Default for IncrementalResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalResolver {
    pub fn new() -> Self {
        Self {
            graph: HashMap::new(),
            cache: HashMap::new(),
            changed_packages: Vec::new(),
        }
    }

    /// Mark packages as changed
    pub fn mark_changed(&mut self, packages: &[String]) {
        self.changed_packages = packages.to_vec();

        // Mark affected nodes as red
        for pkg in packages {
            if let Some(node) = self.graph.get_mut(pkg) {
                node.color = NodeColor::Red;
            }
        }
    }

    /// Mark packages as changed using Cellular Sheaf Laplacian perturbation support $\operatorname{supp}(\Delta L_{\mathcal{F}})$.
    /// Uses Hodge theory to identify the exact subcomplex perturbed by the changed packages
    /// and their algebraic connectivity / spectral gap $\lambda_2$.
    pub fn mark_changed_topological(
        &mut self,
        packages: &[String],
        spectral_gap: f64,
    ) -> std::collections::HashSet<String> {
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut rev_adj: HashMap<String, Vec<String>> = HashMap::new();

        for (node_key, node) in &self.graph {
            adj.insert(node_key.clone(), node.dependencies.clone());
            for dep in &node.dependencies {
                rev_adj
                    .entry(dep.clone())
                    .or_default()
                    .push(node_key.clone());
            }
        }

        let affected = crate::jagr_infinite::sheaf_laplacian::compute_perturbation_support(
            packages,
            &adj,
            &rev_adj,
            spectral_gap,
        );

        for pkg in &affected {
            if let Some(node) = self.graph.get_mut(pkg) {
                node.color = NodeColor::Red;
            }
        }

        self.changed_packages = affected.iter().cloned().collect();
        affected
    }

    /// Try to mark node as green (unchanged)
    pub fn try_mark_green(&mut self, node_key: &str) -> bool {
        // Get dependencies
        let deps = {
            if let Some(node) = self.graph.get(node_key) {
                node.dependencies.clone()
            } else {
                return false;
            }
        };

        // Check if all dependencies are green
        for dep in &deps {
            if let Some(dep_node) = self.graph.get(dep) {
                if dep_node.color == NodeColor::Red {
                    return false;
                }
            }
        }

        // All dependencies are green, mark this node as green
        if let Some(node) = self.graph.get_mut(node_key) {
            node.color = NodeColor::Green;
            return true;
        }

        false
    }

    /// Resolve dependency with incremental caching
    pub fn resolve_incremental(
        &mut self,
        package: &str,
        version_req: &str,
    ) -> Result<ResolutionResult, String> {
        let key = format!("{}@{}", package, version_req);
        let fingerprint = Fingerprint::compute(format!("{}@{}", package, version_req).as_bytes());

        // Check if node exists
        if !self.graph.contains_key(&key) {
            // New node, must resolve
            let result = self.resolve_from_scratch(package, version_req)?;
            self.cache_node(key.clone(), fingerprint, result.clone());
            return Ok(result);
        }

        // Try to mark green
        if self.try_mark_green(&key) {
            // Node is green, reuse cached result
            if let Some(node) = self.graph.get(&key) {
                if let Some(cached) = &node.cached_result {
                    return Ok(cached.clone());
                }
            }
        }

        // Node is red, must recompute
        let result = self.resolve_from_scratch(package, version_req)?;
        self.cache_node(key, fingerprint, result.clone());
        Ok(result)
    }

    /// Resolve from scratch (calls underlying resolver)
    fn resolve_from_scratch(
        &self,
        _package: &str,
        _version_req: &str,
    ) -> Result<ResolutionResult, String> {
        // This would call the actual resolver (pubgrub, SAT, etc.)
        // For now, return a placeholder
        Ok(ResolutionResult {
            resolved_version: "1.0.0".to_string(),
            dependencies: Vec::new(),
            integrity: "sha256-placeholder".to_string(),
        })
    }

    /// Cache node with fingerprint
    fn cache_node(&mut self, key: String, fingerprint: Fingerprint, result: ResolutionResult) {
        let node = DepGraphNode {
            key: key.clone(),
            fingerprint,
            color: NodeColor::Green,
            dependencies: result.dependencies.clone(),
            cached_result: Some(result.clone()),
        };

        self.graph.insert(key, node);
        self.cache.insert(fingerprint, result);
    }

    /// Get cache statistics
    pub fn stats(&self) -> ResolverStats {
        let green_count = self
            .graph
            .values()
            .filter(|n| n.color == NodeColor::Green)
            .count();
        let red_count = self
            .graph
            .values()
            .filter(|n| n.color == NodeColor::Red)
            .count();

        ResolverStats {
            total_nodes: self.graph.len(),
            green_nodes: green_count,
            red_nodes: red_count,
            cache_size: self.cache.len(),
            hit_rate: if self.graph.is_empty() {
                0.0
            } else {
                green_count as f64 / self.graph.len() as f64
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolverStats {
    pub total_nodes: usize,
    pub green_nodes: usize,
    pub red_nodes: usize,
    pub cache_size: usize,
    pub hit_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_incremental_resolver() {
        let mut resolver = IncrementalResolver::new();

        // First resolution (cold)
        let result1 = resolver.resolve_incremental("react", "^18.0.0").unwrap();
        assert_eq!(result1.resolved_version, "1.0.0");

        // Second resolution (should be cached/green)
        let result2 = resolver.resolve_incremental("react", "^18.0.0").unwrap();
        assert_eq!(result2.resolved_version, "1.0.0");

        let stats = resolver.stats();
        assert_eq!(stats.total_nodes, 1);
        assert_eq!(stats.green_nodes, 1);
        assert!(stats.hit_rate > 0.0);
    }

    #[test]
    fn test_red_green_marking() {
        let mut resolver = IncrementalResolver::new();

        // Resolve dependencies
        resolver.resolve_incremental("react", "^18.0.0").unwrap();
        resolver
            .resolve_incremental("react-dom", "^18.0.0")
            .unwrap();

        // Mark react as changed
        resolver.mark_changed(&["react@^18.0.0".to_string()]);

        // React should be red
        let react_node = resolver.graph.get("react@^18.0.0").unwrap();
        assert_eq!(react_node.color, NodeColor::Red);

        // react-dom should still be green (not affected)
        let dom_node = resolver.graph.get("react-dom@^18.0.0").unwrap();
        assert_eq!(dom_node.color, NodeColor::Green);
    }

    #[test]
    fn test_topological_red_green_marking() {
        let mut resolver = IncrementalResolver::new();
        resolver.resolve_incremental("react", "^18.0.0").unwrap();
        resolver
            .resolve_incremental("scheduler", "^0.23.0")
            .unwrap();

        // Connect react -> scheduler
        if let Some(react_node) = resolver.graph.get_mut("react@^18.0.0") {
            react_node
                .dependencies
                .push("scheduler@^0.23.0".to_string());
        }

        // With high spectral gap, marking scheduler as changed should propagate along incidence to react
        let affected = resolver.mark_changed_topological(&["scheduler@^0.23.0".to_string()], 0.75);
        assert!(affected.contains("scheduler@^0.23.0"));
        assert!(affected.contains("react@^18.0.0"));

        let sched_node = resolver.graph.get("scheduler@^0.23.0").unwrap();
        assert_eq!(sched_node.color, NodeColor::Red);

        let react_node = resolver.graph.get("react@^18.0.0").unwrap();
        assert_eq!(react_node.color, NodeColor::Red);
    }
}

/// Differential graph mutation engine (was `incremental_resolver.rs`).
pub mod incremental_resolver {
    use std::collections::HashMap;

    #[derive(Debug, Clone, Eq, PartialEq, Hash)]
    pub struct PackageNode {
        pub name: String,
        pub version: String,
        pub dependencies: Vec<String>,
    }

    #[derive(Debug, Clone, Eq, PartialEq, Hash)]
    pub enum GraphChange {
        AddPackage(PackageNode),
        RemovePackage(String),
        UpdateVersion(String, String),
    }

    pub struct IncrementalResolver {
        packages: HashMap<String, PackageNode>,
    }

    impl Default for IncrementalResolver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl IncrementalResolver {
        pub fn new() -> Self {
            Self {
                packages: HashMap::new(),
            }
        }

        pub fn add_package(&mut self, package: PackageNode) {
            self.packages.insert(package.name.clone(), package);
        }

        pub fn update_package(&mut self, name: &str, new_version: &str) {
            if let Some(pkg) = self.packages.get_mut(name) {
                pkg.version = new_version.to_string();
            }
        }

        pub fn remove_package(&mut self, name: &str) {
            self.packages.remove(name);
        }

        pub fn get_resolved(&self) -> Vec<PackageNode> {
            self.packages.values().cloned().collect()
        }

        pub fn step(&mut self) -> bool {
            false
        }
    }

    use parking_lot::RwLock;
    use std::sync::OnceLock;

    fn global_resolver() -> &'static RwLock<IncrementalResolver> {
        static INSTANCE: OnceLock<RwLock<IncrementalResolver>> = OnceLock::new();
        INSTANCE.get_or_init(|| RwLock::new(IncrementalResolver::new()))
    }

    pub fn add_package_global(package: PackageNode) {
        global_resolver().write().add_package(package);
    }

    pub fn update_package_global(name: &str, version: &str) {
        global_resolver().write().update_package(name, version);
    }

    pub fn get_resolved_global() -> Vec<PackageNode> {
        global_resolver().read().get_resolved()
    }
}
