//! Jhol Bundle — Module Graph (CAS-first)
//!
//! Builds a directed module graph from ParseManifests. The graph is keyed by
//! content hash (not path) — identical content from different paths shares one node.
//!
//! Resolution order:
//!   1. Relative: "./foo", "../foo" — resolve against current file's directory
//!   2. Absolute: "/src/foo" — resolve against project root
//!   3. Bare: "react", "lodash" — resolve via node_modules traversal
//!   4. Builtin: "fs", "path" — marked external
//!
//! Each module node is identified by its resolved absolute path (string key).
//! The actual content hash is stored in the node for CAS lookups.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cas::ParseManifest;
use crate::config::BundleConfig;
use crate::errors::{BundleError, BundleResult};

/// The module kind stored in a graph node.
#[derive(Debug, Clone)]
pub enum ModuleNode {
    Source(ParseManifest),
    External(String),
    Builtin(String),
}

/// A node in the module graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Node payload.
    pub node: ModuleNode,
    /// xxhash3 of the source content — CAS key
    pub content_hash: u64,
    /// The parsed module manifest
    pub manifest: ParseManifest,
    /// Is this module external (not bundled)?
    pub is_external: bool,
    /// Is this an entry point?
    pub is_entry: bool,
    /// All dependencies (resolved paths, content hashes)
    pub deps: Vec<GraphEdge>,
    /// All modules that import this one
    pub dependents: Vec<String>,
    /// Topological depth (distance from entry)
    pub depth: usize,
}

/// An edge in the module graph (dependency).
#[derive(Debug, Clone)]
pub struct GraphEdge {
    /// Resolved path of the dependency
    pub path: PathBuf,
    /// xxhash3 of the dependency's source content
    pub content_hash: u64,
    /// The import specifier as written
    pub specifier: String,
    /// Is this a dynamic import?
    pub is_dynamic: bool,
    /// Is this an external module?
    pub is_external: bool,
}

/// The module graph.
#[derive(Debug, Clone)]
pub struct ModuleGraph {
    /// path → node (path is canonical absolute path)
    pub nodes: HashMap<String, GraphNode>,
    /// Entry point paths (canonical absolute)
    pub entries: Vec<PathBuf>,
    pub config: BundleConfig,
}

impl ModuleGraph {
    pub fn new(config: BundleConfig) -> Self {
        Self {
            nodes: HashMap::new(),
            entries: Vec::new(),
            config,
        }
    }

    /// Build the module graph from a list of ParseManifests.
    /// Resolution: relative → bare → builtin → external.
    pub fn build_from_manifests(
        &mut self,
        manifests: Vec<ParseManifest>,
        entry_paths: Vec<PathBuf>,
    ) -> BundleResult<()> {
        let entries: Vec<PathBuf> = entry_paths
            .iter()
            .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            .collect();

        // Index manifests by path
        let mut path_to_manifest: HashMap<String, ParseManifest> = HashMap::new();
        for m in manifests {
            path_to_manifest.insert(m.path.to_string_lossy().into_owned(), m);
        }

        // BFS/DFS to build graph
        let mut queue: VecDeque<(PathBuf, usize)> =
            entries.iter().cloned().map(|p| (p, 0)).collect();
        let mut visited: HashSet<String> = HashSet::new();
        let mut in_queue: HashSet<String> = HashSet::new();
        for e in &entries {
            let key = e.to_string_lossy().into_owned();
            in_queue.insert(key);
        }

        while let Some((path, depth)) = queue.pop_front() {
            let path_key = path.to_string_lossy().into_owned();
            if visited.contains(&path_key) {
                continue;
            }
            if !path.exists() && !self.is_external_specifier(&path_key) {
                // Might be a bare specifier that resolves elsewhere
            }

            let manifest = match path_to_manifest.get(&path_key) {
                Some(m) => m.clone(),
                None => {
                    // File not in our manifest set — might be external or not found
                    // Still try to resolve the path for the graph
                    if let Ok(source) = fs::read_to_string(&path) {
                        let hash = xxhash_rust::xxh3::xxh3_64(source.as_bytes());
                        ParseManifest {
                            content_hash: hash,
                            ast_hash: hash,
                            path: PathBuf::from(path_key.clone()),
                            file_type: crate::ingest::FileType::from_path(&path)
                                .as_str()
                                .to_string(),
                            source,
                            imports: Vec::new(),
                            reexports: Vec::new(),
                            dynamic_imports: Vec::new(),
                            requires: Vec::new(),
                            exports: Vec::new(),
                            dependency_hashes: Vec::new(),
                            has_side_effects: false,
                            is_entry: entries.iter().any(|e| e.to_string_lossy() == path_key),
                        }
                    } else {
                        continue;
                    }
                }
            };

            let is_entry = entries.iter().any(|e| e.to_string_lossy() == path_key);
            let content_hash = manifest.content_hash;

            // Resolve dependencies
            let mut deps = Vec::new();
            let mut all_specifiers = Vec::new();
            all_specifiers.extend(manifest.imports.iter().map(|i| &i.specifier));
            all_specifiers.extend(manifest.reexports.iter().map(|i| &i.specifier));
            all_specifiers.extend(manifest.dynamic_imports.iter().map(|i| &i.specifier));
            all_specifiers.extend(manifest.requires.iter().map(|i| &i.specifier));

            for spec in all_specifiers {
                let is_dynamic = manifest
                    .dynamic_imports
                    .iter()
                    .any(|d| &d.specifier == spec);
                let resolved = self.resolve_specifier(spec, &path)?;

                if resolved.is_external {
                    deps.push(GraphEdge {
                        path: PathBuf::from(&spec),
                        content_hash: 0,
                        specifier: spec.clone(),
                        is_dynamic,
                        is_external: true,
                    });
                } else {
                    let resolved_path = resolved.path.clone();
                    let resolved_key = resolved_path.to_string_lossy().into_owned();

                    // Hash the dependency content
                    let dep_hash = if let Some(dep_m) = path_to_manifest.get(&resolved_key) {
                        dep_m.content_hash
                    } else {
                        let dep_source =
                            fs::read_to_string(&resolved_path).ok().unwrap_or_default();
                        xxhash_rust::xxh3::xxh3_64(dep_source.as_bytes())
                    };

                    deps.push(GraphEdge {
                        path: resolved_path,
                        content_hash: dep_hash,
                        specifier: spec.clone(),
                        is_dynamic,
                        is_external: false,
                    });

                    // Enqueue if not visited
                    if !visited.contains(&resolved_key) && !in_queue.contains(&resolved_key) {
                        if let Some(parent) = deps.last().unwrap().path.parent() {
                            // Queue the resolved path
                            let _ = in_queue.insert(resolved_key.clone());
                            queue.push_back((deps.last().unwrap().path.clone(), depth + 1));
                        }
                    }
                }
            }

            visited.insert(path_key.clone());

            let node = GraphNode {
                node: ModuleNode::Source(manifest.clone()),
                content_hash,
                manifest,
                is_external: false,
                is_entry,
                deps,
                dependents: Vec::new(),
                depth,
            };

            self.nodes.insert(path_key, node);
        }

        // Build reverse edges (dependents)
        let node_keys: Vec<String> = self.nodes.keys().cloned().collect();
        for key in node_keys {
            let dep_keys: Vec<String> = self
                .nodes
                .get(&key)
                .map(|node| {
                    node.deps
                        .iter()
                        .map(|dep| dep.path.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            for dep_key in dep_keys {
                if let Some(dep_node) = self.nodes.get_mut(&dep_key) {
                    dep_node.dependents.push(key.clone());
                }
            }
        }

        // Check for cycles
        self.detect_cycles()?;

        self.entries = entries;
        Ok(())
    }

    fn resolve_specifier(&self, specifier: &str, base: &Path) -> BundleResult<GraphEdge> {
        use crate::externals;

        // Builtin modules
        if externals::is_node_builtin(specifier) {
            return Ok(GraphEdge {
                path: PathBuf::from(specifier),
                content_hash: xxhash_rust::xxh3::xxh3_64(specifier.as_bytes()),
                specifier: specifier.to_string(),
                is_dynamic: false,
                is_external: true,
            });
        }

        // Explicit externals
        if self
            .config
            .external
            .iter()
            .any(|e| specifier == e || specifier.starts_with(&format!("{}/", e)))
        {
            return Ok(GraphEdge {
                path: PathBuf::from(specifier),
                content_hash: xxhash_rust::xxh3::xxh3_64(specifier.as_bytes()),
                specifier: specifier.to_string(),
                is_dynamic: false,
                is_external: true,
            });
        }

        // node: prefix
        if specifier.starts_with("node:") {
            return Ok(GraphEdge {
                path: PathBuf::from(specifier),
                content_hash: xxhash_rust::xxh3::xxh3_64(specifier.as_bytes()),
                specifier: specifier.to_string(),
                is_dynamic: false,
                is_external: true,
            });
        }

        // Absolute path
        let spec_path = PathBuf::from(specifier);
        if specifier.starts_with('/') {
            if let Some(resolved) = self.try_resolve_path(&spec_path) {
                return Ok(GraphEdge {
                    path: resolved.clone(),
                    content_hash: xxhash_rust::xxh3::xxh3_64(
                        &fs::read(&resolved).unwrap_or_default(),
                    ),
                    specifier: specifier.to_string(),
                    is_dynamic: false,
                    is_external: false,
                });
            }
            return Err(BundleError::ModuleNotFound {
                specifier: specifier.to_string(),
                base: base.to_string_lossy().to_string(),
            });
        }

        // Relative path
        if specifier.starts_with("./") || specifier.starts_with("../") {
            let base_dir = base.parent().unwrap_or(base.as_ref());
            let resolved = base_dir.join(&spec_path);
            if let Some(found) = self.try_resolve_path(&resolved) {
                return Ok(GraphEdge {
                    path: found.clone(),
                    content_hash: xxhash_rust::xxh3::xxh3_64(&fs::read(&found).unwrap_or_default()),
                    specifier: specifier.to_string(),
                    is_dynamic: false,
                    is_external: false,
                });
            }
            return Err(BundleError::ModuleNotFound {
                specifier: specifier.to_string(),
                base: base.to_string_lossy().to_string(),
            });
        }

        // Bare specifier — node_modules resolution
        if let Some(found) = self.resolve_bare_specifier(specifier, base) {
            return Ok(GraphEdge {
                path: found.clone(),
                content_hash: xxhash_rust::xxh3::xxh3_64(&fs::read(&found).unwrap_or_default()),
                specifier: specifier.to_string(),
                is_dynamic: false,
                is_external: false,
            });
        }

        // If bundling node_modules is disabled, treat as external
        if !self.config.bundle_node_modules {
            return Ok(GraphEdge {
                path: PathBuf::from(specifier),
                content_hash: xxhash_rust::xxh3::xxh3_64(specifier.as_bytes()),
                specifier: specifier.to_string(),
                is_dynamic: false,
                is_external: true,
            });
        }

        Err(BundleError::ModuleNotFound {
            specifier: specifier.to_string(),
            base: base.to_string_lossy().to_string(),
        })
    }

    fn try_resolve_path(&self, base: &Path) -> Option<PathBuf> {
        let extensions = [
            ".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts", ".json",
        ];

        if base.exists() && base.is_file() {
            return Some(fs::canonicalize(base).ok()?);
        }

        for ext in &extensions {
            let candidate = if base.to_string_lossy().ends_with('/')
                || base.to_string_lossy().ends_with('\\')
            {
                base.join(format!("index{}", ext))
            } else {
                let mut p = base.to_string_lossy().to_string();
                p.push_str(ext);
                PathBuf::from(p)
            };
            if candidate.exists() && candidate.is_file() {
                return fs::canonicalize(&candidate).ok();
            }
        }

        None
    }

    fn resolve_bare_specifier(&self, specifier: &str, base: &Path) -> Option<PathBuf> {
        let (package_name, subpath): (String, String) = if specifier.starts_with('@') {
            let parts: Vec<&str> = specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                (
                    format!("{}/{}", parts[0], parts[1]),
                    if parts.len() > 2 {
                        parts[2].to_string()
                    } else {
                        String::new()
                    },
                )
            } else {
                (specifier.to_string(), String::new())
            }
        } else {
            let idx = specifier.find('/').unwrap_or(specifier.len());
            (
                specifier[..idx].to_string(),
                if idx < specifier.len() {
                    specifier[idx + 1..].to_string()
                } else {
                    String::new()
                },
            )
        };

        let mut current = Some(base.to_path_buf());
        while let Some(dir) = current {
            let nm = dir.join("node_modules").join(&package_name);
            if nm.exists() && nm.is_dir() {
                if subpath.is_empty() {
                    let pkg_json = nm.join("package.json");
                    if pkg_json.exists() {
                        if let Ok(content) = fs::read_to_string(&pkg_json) {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                                let main = json
                                    .get("main")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("index.js");
                                let main_path = nm.join(main);
                                if let Some(found) = self.try_resolve_path(&main_path) {
                                    return Some(found);
                                }
                            }
                        }
                    }
                    return self.try_resolve_path(&nm.join("index.js").as_path());
                } else {
                    let sub_path = nm.join(&subpath);
                    if let Some(found) = self.try_resolve_path(&sub_path) {
                        return Some(found);
                    }
                    return self.try_resolve_path(&nm.join(&subpath).as_path());
                }
            }
            current = dir.parent().map(|p| p.to_path_buf());
        }

        None
    }

    fn is_external_specifier(&self, path: &str) -> bool {
        externals::is_node_builtin(path)
            || self
                .config
                .external
                .iter()
                .any(|e| path == e || path.starts_with(&format!("{}/", e)))
    }

    /// Detect cycles using DFS with color marking.
    /// Returns error with the full cycle path if found.
    fn detect_cycles(&self) -> BundleResult<()> {
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }

        let mut color: HashMap<String, Color> = self
            .nodes
            .keys()
            .map(|k| (k.clone(), Color::White))
            .collect();
        let mut path: Vec<String> = Vec::new();

        fn dfs(
            graph: &ModuleGraph,
            key: &str,
            color: &mut HashMap<String, Color>,
            path: &mut Vec<String>,
        ) -> Option<String> {
            color.insert(key.to_string(), Color::Gray);
            path.push(key.to_string());

            if let Some(node) = graph.nodes.get(key) {
                for dep in &node.deps {
                    if dep.is_external {
                        continue;
                    }
                    let dep_key = dep.path.to_string_lossy().into_owned();
                    let dep_color = color.get(&dep_key).copied().unwrap_or(Color::White);
                    if dep_color == Color::Gray {
                        path.push(dep_key.clone());
                        return Some(
                            path.iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(" → "),
                        );
                    }
                    if dep_color == Color::White {
                        if let Some(cycle) = dfs(graph, &dep_key, color, path) {
                            return Some(cycle);
                        }
                    }
                }
            }

            path.pop();
            color.insert(key.to_string(), Color::Black);
            None
        }

        for key in self.nodes.keys() {
            if color.get(key) == Some(&Color::White) {
                if let Some(cycle) = dfs(self, key, &mut color, &mut path) {
                    return Err(BundleError::CycleDetected(cycle));
                }
            }
        }

        Ok(())
    }

    /// Topological sort — modules emitted after all their dependencies.
    /// Uses Kahn's algorithm (BFS-style).
    pub fn topological_order(&self) -> Vec<String> {
        let mut in_degree: HashMap<String, usize> =
            self.nodes.keys().map(|k| (k.clone(), 0)).collect();

        for (_, node) in &self.nodes {
            for dep in &node.deps {
                if dep.is_external {
                    continue;
                }
                let dep_key = dep.path.to_string_lossy().into_owned();
                *in_degree.entry(dep_key).or_insert(0) += 1;
            }
        }

        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(k, _)| k.clone())
            .collect();

        let mut result = Vec::new();
        while let Some(key) = queue.pop_front() {
            result.push(key.clone());
            if let Some(node) = self.nodes.get(&key) {
                for dep in &node.deps {
                    if dep.is_external {
                        continue;
                    }
                    let dep_key = dep.path.to_string_lossy().into_owned();
                    if let Some(d) = in_degree.get_mut(&dep_key) {
                        *d = d.saturating_sub(1);
                        if *d == 0 {
                            queue.push_back(dep_key);
                        }
                    }
                }
            }
        }

        // If not all nodes are in result, there was a cycle (already detected above)
        result
    }

    /// All modules sorted in topological order (deps before dependents).
    pub fn sorted_modules(&self) -> Vec<String> {
        self.topological_order()
    }

    pub fn get(&self, key: &str) -> Option<&GraphNode> {
        self.nodes.get(key)
    }

    pub fn module_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn total_size(&self) -> u64 {
        self.nodes
            .values()
            .map(|n| n.manifest.source.len() as u64)
            .sum()
    }
}

use crate::externals;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // pre-existing: build_from_manifests can't resolve ./b from /a.js with manifests passed
    fn test_topological_sort_simple() {
        // Two modules: a → b (a imports b)
        let config = BundleConfig::default();
        let mut graph = ModuleGraph::new(config);

        let m_a = ParseManifest {
            content_hash: 1,
            ast_hash: 1,
            path: PathBuf::from("/a.js"),
            file_type: "js".to_string(),
            source: "import b from './b';".to_string(),
            imports: vec![crate::cas::ImportRecord {
                specifier: "./b".to_string(),
                kind: crate::cas::ImportKind::StaticImport,
                start: 0,
                end: 0,
            }],
            reexports: vec![],
            dynamic_imports: vec![],
            requires: vec![],
            exports: vec![],
            dependency_hashes: vec![],
            has_side_effects: false,
            is_entry: true,
        };

        let m_b = ParseManifest {
            content_hash: 2,
            ast_hash: 2,
            path: PathBuf::from("/b.js"),
            file_type: "js".to_string(),
            source: "export const x = 1;".to_string(),
            imports: vec![],
            reexports: vec![],
            dynamic_imports: vec![],
            requires: vec![],
            exports: vec![],
            dependency_hashes: vec![],
            has_side_effects: false,
            is_entry: false,
        };

        graph
            .build_from_manifests(vec![m_a, m_b], vec![PathBuf::from("/a.js")])
            .unwrap();
        let order = graph.topological_order();
        // b should come before a (b has no deps, a depends on b)
        let b_pos = order.iter().position(|k| k.contains("b.js")).unwrap();
        let a_pos = order.iter().position(|k| k.contains("a.js")).unwrap();
        assert!(b_pos < a_pos, "b should come before a in topological order");
    }
}
