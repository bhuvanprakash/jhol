//! Jhol Bundle — Tree Shaking (CAS-first)
//!
//! Side-effect-based tree shaking. Unlike export-based tree shaking (rollup),
//! we use side-effect analysis to determine what CAN be eliminated safely.
//!
//! Side effect categories:
//!   PURE: Module only exports values, no top-level effects. Safe to tree shake.
//!   HAS_SIDE_EFFECTS: Module has top-level effects (console.log, fetch, require, etc.)
//!   UNKNOWN: No manifest (external/native) — conservatively kept.
//!
//! Algorithm (bottom-up, post-topological sort):
//!   1. Walk modules from leaves to root (reverse topological order)
//!   2. If module.exports is empty AND module has no side effects → mark as unused
//!   3. If module has side effects → always keep it
//!   4. If module is used by a kept module → keep it
//!
//! The result is a `used_hashes: HashSet<u64>` — all content hashes that must be emitted.

use std::collections::{HashMap, HashSet};

use crate::cas::ParseManifest;
use crate::graph::{GraphNode, ModuleGraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectStatus {
    Pure,
    HasSideEffects,
    Unknown,
}

impl SideEffectStatus {
    pub fn from_manifest(manifest: &ParseManifest) -> Self {
        if manifest.has_side_effects {
            SideEffectStatus::HasSideEffects
        } else if manifest.exports.is_empty() && manifest.imports.is_empty() {
            SideEffectStatus::Pure
        } else {
            SideEffectStatus::Pure
        }
    }
}

/// Tree shaking result — which modules should be emitted.
#[derive(Debug, Clone)]
pub struct TreeShakeResult {
    /// Content hashes of modules that should be emitted
    pub used_hashes: HashSet<u64>,
    /// Total unused module count
    pub unused_count: usize,
}

pub struct TreeShaker;

impl TreeShaker {
    pub fn new() -> Self {
        Self
    }

    /// Run tree shaking on the module graph.
    /// Returns a HashSet of content hashes that should be emitted.
    pub fn shake(&self, graph: &ModuleGraph) -> TreeShakeResult {
        let order = graph.topological_order();
        let mut used_hashes: HashSet<u64> = HashSet::new();
        let side_effect_modules: HashSet<u64> = HashSet::new();
        let mut export_used: HashMap<String, HashSet<String>> = HashMap::new();

        // Phase 1: mark all entry modules as used
        for key in &order {
            if let Some(node) = graph.get(key) {
                if node.is_entry {
                    used_hashes.insert(node.content_hash);
                    // All exports of entry modules are considered "used"
                    export_used.insert(key.clone(), HashSet::new()); // empty = all used
                }
            }
        }

        // Phase 2: walk in reverse topological order (leaves first)
        for key in order.iter().rev() {
            let node = match graph.get(key) {
                Some(n) => n,
                None => continue,
            };

            let status = SideEffectStatus::from_manifest(&node.manifest);

            // Check if any dependent uses this module's exports
            let dep_uses_exports = export_used
                .get(key)
                .map(|used| !used.is_empty()) // empty = all exports used (entry or unused all)
                .unwrap_or(false);

            let has_dependents = !node.dependents.is_empty();

            // Keep if: has side effects OR is entry OR dependent uses exports
            if status == SideEffectStatus::HasSideEffects
                || node.is_entry
                || dep_uses_exports
                || has_dependents
            {
                used_hashes.insert(node.content_hash);
            }

            // Propagate: this module's exports are "used" if this module itself is used
            if used_hashes.contains(&node.content_hash) {
                // If this module uses something, propagate "used" upward
                for dep in &node.deps {
                    if dep.is_external {
                        continue;
                    }
                    let dep_key = dep.path.to_string_lossy().into_owned();
                    // Add this module's exports to what the dep provides
                    for export in &node.manifest.exports {
                        export_used
                            .entry(dep_key.clone())
                            .or_default()
                            .insert(export.name.clone());
                    }
                }
            }
        }

        let total = graph.module_count();
        let unused_count = total.saturating_sub(used_hashes.len());

        TreeShakeResult {
            used_hashes,
            unused_count,
        }
    }
}

impl Default for TreeShaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Filter a module graph to only include "used" modules.
/// The result can be passed directly to emit().
pub fn filter_graph<'a>(graph: &'a ModuleGraph, used: &HashSet<u64>) -> Vec<&'a GraphNode> {
    graph
        .nodes
        .values()
        .filter(|n| used.contains(&n.content_hash) && !n.is_external)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_side_effect_status() {
        let pure = ParseManifest {
            content_hash: 1,
            ast_hash: 1,
            path: PathBuf::from("/pure.js"),
            file_type: "js".to_string(),
            source: "const x = 1; export { x };".to_string(),
            imports: vec![],
            reexports: vec![],
            dynamic_imports: vec![],
            requires: vec![],
            exports: vec![crate::cas::ExportRecord {
                name: "x".to_string(),
                alias: None,
                source: None,
            }],
            dependency_hashes: vec![],
            has_side_effects: false,
            is_entry: false,
        };

        assert_eq!(
            SideEffectStatus::from_manifest(&pure),
            SideEffectStatus::Pure
        );
    }
}
