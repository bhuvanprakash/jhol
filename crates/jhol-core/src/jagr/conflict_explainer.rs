//! JAGR: Derivation Graph Conflict Explainer
//!
//! Like PubGrub's error messages, but built from the three-layer
//! JAGR derivation. Provides human-readable explanations of
//! resolution failures.
//!
//! The derivation graph records:
//! - Why a package was selected (incompatibility chain)
//! - Why a constraint was derived
//! - How versions were tried
//! - The root cause of failures

use super::core_guided_lp::Assignment;
use super::tropical_math::MaxAtomSystem;
use super::tropical_sheaf::SheafAnalysis;
use std::collections::{HashMap, HashSet};

/// A node in the derivation graph
#[derive(Debug, Clone)]
pub enum DerivationNode {
    /// A package-version decision
    Decision {
        package: String,
        version: String,
        level: usize,
    },
    /// A constraint derivation
    Derivation { constraint: String, cause: String },
    /// A conflict found
    Conflict {
        incompatibility: String,
        involved: Vec<String>,
    },
    /// A learned clause/incompatibility
    Learned { expression: String, reason: String },
}

/// Edge in the derivation graph
#[derive(Debug, Clone)]
pub struct DerivationEdge {
    pub from: usize,
    pub to: usize,
    pub reason: String,
}

/// Derivation graph (similar to PubGrub's)
#[derive(Debug, Clone)]
pub struct DerivationGraph {
    pub nodes: Vec<DerivationNode>,
    pub edges: Vec<DerivationEdge>,
    /// Node indices for quick lookup
    pub decisions: Vec<usize>,
    pub conflicts: Vec<usize>,
}

impl DerivationGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            decisions: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    pub fn add_node(&mut self, node: DerivationNode) -> usize {
        let idx = self.nodes.len();

        match &node {
            DerivationNode::Decision { .. } => self.decisions.push(idx),
            DerivationNode::Conflict { .. } => self.conflicts.push(idx),
            _ => {}
        }

        self.nodes.push(node);
        idx
    }

    pub fn add_edge(&mut self, from: usize, to: usize, reason: String) {
        self.edges.push(DerivationEdge { from, to, reason });
    }

    /// Build human-readable explanation
    pub fn explain(&self) -> String {
        let mut explanation = String::new();

        for conflict_idx in &self.conflicts {
            if let DerivationNode::Conflict {
                incompatibility,
                involved,
            } = &self.nodes[*conflict_idx]
            {
                explanation.push_str(&format!("× {}\n", incompatibility));
                explanation.push_str("  Incompatible requirements:\n");
                for pkg in involved {
                    explanation.push_str(&format!("    • {}\n", pkg));
                }

                // Find what caused this conflict
                let causes: Vec<&str> = self
                    .edges
                    .iter()
                    .filter(|e| e.to == *conflict_idx)
                    .map(|e| e.reason.as_str())
                    .collect();

                for cause in causes {
                    explanation.push_str(&format!("  Because: {}\n", cause));
                }
            }
        }

        explanation
    }
}

/// Explanation builder
pub struct ConflictExplainer {
    graph: DerivationGraph,
    /// Package name to decision index
    decisions: HashMap<String, usize>,
    /// Layer 3 (tropical) analysis
    tropical_result: Option<super::tropical_max_atom::MaxAtomResult>,
    /// Layer 2 (sheaf) analysis
    sheaf_analysis: Option<SheafAnalysis>,
    /// Layer 1 (MaxSAT) result
    maxsat_assignment: Option<Assignment>,
}

impl ConflictExplainer {
    pub fn new() -> Self {
        Self {
            graph: DerivationGraph::new(),
            decisions: HashMap::new(),
            tropical_result: None,
            sheaf_analysis: None,
            maxsat_assignment: None,
        }
    }

    /// Add a decision to the graph
    pub fn add_decision(&mut self, package: &str, version: &str, level: usize) -> usize {
        let idx = self.graph.add_node(DerivationNode::Decision {
            package: package.to_string(),
            version: version.to_string(),
            level,
        });
        self.decisions.insert(package.to_string(), idx);
        idx
    }

    /// Add a constraint derivation
    pub fn add_derivation(&mut self, constraint: &str, cause: &str) -> usize {
        self.graph.add_node(DerivationNode::Derivation {
            constraint: constraint.to_string(),
            cause: cause.to_string(),
        })
    }

    /// Add a learned incompatibility
    pub fn add_learned(&mut self, expression: &str, reason: &str) -> usize {
        self.graph.add_node(DerivationNode::Learned {
            expression: expression.to_string(),
            reason: reason.to_string(),
        })
    }

    /// Add a conflict
    pub fn add_conflict(&mut self, incompatibility: &str, involved: Vec<String>) -> usize {
        self.graph.add_node(DerivationNode::Conflict {
            incompatibility: incompatibility.to_string(),
            involved,
        })
    }

    /// Set Layer 3 (tropical) result
    pub fn set_tropical_result(&mut self, result: super::tropical_max_atom::MaxAtomResult) {
        self.tropical_result = Some(result);
    }

    /// Set Layer 2 (sheaf) analysis
    pub fn set_sheaf_analysis(&mut self, analysis: SheafAnalysis) {
        self.sheaf_analysis = Some(analysis);
    }

    /// Set Layer 1 (MaxSAT) assignment
    pub fn set_maxsat_assignment(&mut self, assignment: Assignment) {
        self.maxsat_assignment = Some(assignment);
    }

    /// Build a complete explanation from max-atom conflict
    pub fn explain_max_atom_conflict(
        &mut self,
        conflict: &super::tropical_max_atom::MaxAtomConflict,
        system: &MaxAtomSystem,
    ) -> String {
        let mut explanation = String::new();

        explanation.push_str("Resolution failed at Layer 3 (Tropical Geometry filter)\n");
        explanation.push_str("═══════════════════════════════════════════════════\n\n");

        match conflict.conflict_type {
            super::tropical_max_atom::ConflictType::PositiveCycle => {
                explanation.push_str("A positive cycle was detected in the constraint graph.\n");
                explanation
                    .push_str("This means constraints form a cycle with positive total weight,\n");
                explanation.push_str("making the system infeasible in the tropical semiring.\n\n");

                explanation.push_str("Conflicting max-atoms:\n");
                for (i, atom) in conflict.atoms.iter().enumerate() {
                    explanation.push_str(&format!(
                        "  {}. max(x[{}], x[{}]) + {} ≥ x[{}]\n",
                        i + 1,
                        atom.z,
                        atom.y,
                        atom.r,
                        atom.x
                    ));
                }
            }
            super::tropical_max_atom::ConflictType::DirectContradiction => {
                explanation.push_str("Direct contradiction detected between constraints.\n\n");
                for (i, atom) in conflict.atoms.iter().enumerate() {
                    explanation.push_str(&format!(
                        "  {}. max(x[{}], x[{}]) + {} ≥ x[{}]\n",
                        i + 1,
                        atom.z,
                        atom.y,
                        atom.r,
                        atom.x
                    ));
                }
            }
            super::tropical_max_atom::ConflictType::TrivialCone => {
                explanation.push_str("The tropical cone is trivial (all variables must be -∞).\n");
            }
        }

        explanation.push_str(&format!(
            "\nTotal constraints in system: {}\n",
            system.atoms.len()
        ));
        explanation.push_str(&format!("Number of variables: {}\n", system.num_vars));

        explanation
    }

    /// Build explanation from sheaf analysis
    pub fn explain_sheaf_analysis(&self, analysis: &SheafAnalysis) -> String {
        let mut explanation = String::new();

        explanation.push_str("Resolution passed Layer 3 (Tropical Geometry filter)\n");
        explanation.push_str("Layer 2 (Tropical Sheaf Cohomology) analysis:\n");
        explanation.push_str("═════════════════════════════════════════════════════\n\n");

        explanation.push_str(&format!("Betti numbers: {:?}\n", analysis.betti_numbers));
        explanation.push_str(&format!(
            "  β₀ = {} (connected components)\n",
            analysis.betti_numbers.get(0).copied().unwrap_or(0)
        ));
        explanation.push_str(&format!(
            "  β₁ = {} (1-cycles, obstruction to acyclicity)\n",
            analysis.betti_numbers.get(1).copied().unwrap_or(0)
        ));
        explanation.push_str(&format!(
            "  β₂ = {} (voids, higher-order obstruction)\n",
            analysis.betti_numbers.get(2).copied().unwrap_or(0)
        ));

        explanation.push_str(&format!(
            "\nIn Simplicial Normal Form: {}\n",
            analysis.is_snf
        ));

        if analysis.is_snf {
            explanation.push_str("✓ Homologically acyclic - greedy resolution works\n");
        } else {
            explanation.push_str("✗ Has topological obstructions\n");
            if let Some(obs) = analysis.global_obstruction {
                explanation.push_str(&format!("  Global obstruction magnitude: {}\n", obs));
            }
        }

        explanation
    }

    /// Build complete three-layer explanation
    pub fn explain_full(
        &self,
        max_atom_result: &super::tropical_max_atom::MaxAtomResult,
        sheaf_analysis: &SheafAnalysis,
        maxsat_satisfied: bool,
    ) -> String {
        let mut explanation = String::new();

        explanation.push_str("═══════════════════════════════════════════════════════════\n");
        explanation.push_str("              JAGR Three-Layer Resolution Report\n");
        explanation.push_str("═══════════════════════════════════════════════════════════\n\n");

        // Layer 3
        explanation.push_str("Layer 3: Tropical Geometry Filter (PTIME)\n");
        explanation.push_str("─────────────────────────────────────────\n");
        match max_atom_result {
            super::tropical_max_atom::MaxAtomResult::Satisfiable(_) => {
                explanation.push_str("✓ PTIME filter passed - system is feasible\n\n");
            }
            super::tropical_max_atom::MaxAtomResult::Unsatisfiable(_) => {
                if maxsat_satisfied {
                    explanation.push_str(
                        "○ PTIME filter found an advisory obstruction; exact Layer 1 certified the assignment\n\n",
                    );
                } else {
                    explanation.push_str(
                        "✗ PTIME filter found an obstruction; exact Layer 1 also found no assignment\n\n",
                    );
                }
            }
            super::tropical_max_atom::MaxAtomResult::Trivial => {
                explanation.push_str("○ Trivial system (no constraints)\n\n");
            }
        }

        // Layer 2
        explanation.push_str("Layer 2: Tropical Sheaf Cohomology\n");
        explanation.push_str("───────────────────────────────────\n");
        explanation.push_str(&format!(
            "  β₀ = {}\n",
            sheaf_analysis.betti_numbers.get(0).copied().unwrap_or(0)
        ));
        explanation.push_str(&format!(
            "  β₁ = {}\n",
            sheaf_analysis.betti_numbers.get(1).copied().unwrap_or(0)
        ));
        explanation.push_str(&format!(
            "  β₂ = {}\n",
            sheaf_analysis.betti_numbers.get(2).copied().unwrap_or(0)
        ));
        explanation.push_str(&format!("  SNF (β₁=β₂=0): {}\n", sheaf_analysis.is_snf));
        explanation.push_str(&format!(
            "  Greedy resolution works: {}\n\n",
            sheaf_analysis.greedy_works
        ));

        // Layer 1
        explanation.push_str("Layer 1: Core-Guided MaxSAT (OL3P/OL3P-S)\n");
        explanation.push_str("────────────────────────────────────────\n");
        if maxsat_satisfied {
            explanation.push_str("✓ All hard constraints satisfied\n");
            explanation.push_str("  Max-SMT optimization complete\n\n");
        } else {
            explanation.push_str("✗ Hard constraints unsatisfiable\n");
            explanation.push_str("  See derivation graph for details\n\n");
        }

        explanation
    }

    /// Get the derivation graph
    pub fn graph(&self) -> &DerivationGraph {
        &self.graph
    }

    /// Get mutable derivation graph
    pub fn graph_mut(&mut self) -> &mut DerivationGraph {
        &mut self.graph
    }
}

impl Default for ConflictExplainer {
    fn default() -> Self {
        Self::new()
    }
}

/// Build an explanation from a package.json-style conflict
/// PubGrub-quality conflict explanation with derivation trees.
///
/// Produces human-readable error messages that trace the root cause of
/// dependency conflicts through the full chain of version requirements,
/// similar to how PubGrub (used by pub/dart) explains incompatibilities.
///
/// The explanation follows the pattern:
///   "Because A@1.0 depends on B@^2.0 which depends on C@^1.0,
///    and A@1.0 also depends on C@^2.0,
///    A@1.0 is incompatible with itself."
pub fn explain_package_conflict(
    root_pkg: &str,
    root_ver: &str,
    conflicts: &[(&str, &str, &str, &str)],
) -> String {
    let mut explanation = String::new();

    if conflicts.is_empty() {
        explanation.push_str(&format!(
            "Could not resolve dependencies for {}@{}.\nNo specific conflict chain was identified. \
             This may indicate an empty available-versions list.\n",
            root_pkg, root_ver
        ));
        return explanation;
    }

    explanation.push_str(&format!(
        "Because {}@{} has the following dependency conflicts:\n\n",
        root_pkg, root_ver
    ));

    // Group conflicts by the package that has contradictory requirements
    let mut pkg_to_requirements: HashMap<String, Vec<(&str, &str, &str, &str)>> = HashMap::new();
    for &(pkg, constraint, from_pkg, from_ver) in conflicts {
        // pkg here is the CONSTRAINT target; from_pkg is who requires it
        let key = format!("{}@{}", from_pkg, from_ver);
        pkg_to_requirements
            .entry(key)
            .or_default()
            .push((pkg, constraint, from_pkg, from_ver));
    }

    // For packages with multiple contradictory requirements, show the derivation chain
    let mut seen_root_causes: HashSet<String> = HashSet::new();
    for (_source, reqs) in &pkg_to_requirements {
        if reqs.len() < 2 {
            continue;
        }

        // Extract the conflicting package names from the constraints
        let constraint_pkgs: Vec<&str> = reqs.iter().map(|(pkg, _, _, _)| *pkg).collect();
        let unique_pkgs: HashSet<&str> = constraint_pkgs.into_iter().collect();

        if unique_pkgs.len() > 1 {
            // Multiple different packages required by the same source
            for (pkg, constraint, from_pkg, from_ver) in reqs {
                let cause = format!("{} requires {} ({})", from_pkg, pkg, constraint);
                if !seen_root_causes.contains(&cause) {
                    explanation.push_str(&format!(
                        "  {}@{} depends on {} (from {}@{})\n",
                        root_pkg, root_ver, constraint, from_pkg, from_ver
                    ));
                    seen_root_causes.insert(cause);
                }
            }
            explanation.push('\n');
        }
    }

    // Build derivation chains for version conflicts (same package, different version ranges)
    let mut pkg_versions: HashMap<&str, Vec<(&str, &str, &str)>> = HashMap::new();
    for &(pkg, constraint, from_pkg, from_ver) in conflicts {
        pkg_versions
            .entry(pkg)
            .or_default()
            .push((constraint, from_pkg, from_ver));
    }

    for (pkg, versions) in &pkg_versions {
        if versions.len() < 2 {
            // Single requirement — just list it
            for (constraint, from_pkg, from_ver) in versions {
                explanation.push_str(&format!(
                    "  {}@{} requires {} {}\n",
                    from_pkg, from_ver, pkg, constraint
                ));
            }
        } else {
            // Conflicting requirements for the same package
            explanation.push_str(&format!(
                "  Conflicting requirements for package {}:\n",
                pkg
            ));
            for (constraint, from_pkg, from_ver) in versions {
                explanation.push_str(&format!(
                    "    {}@{} requires {} {}\n",
                    from_pkg, from_ver, pkg, constraint
                ));
            }
            explanation.push_str(&format!(
                "    -> No version of {} satisfies all of the above.\n\n",
                pkg
            ));
        }
    }

    if seen_root_causes.is_empty() && pkg_versions.is_empty() {
        // Fallback: just list the raw conflict tuples
        for (pkg, constraint, from_pkg, from_ver) in conflicts {
            explanation.push_str(&format!(
                "  {}@{} requires {} (from {}@{})\n",
                pkg, from_ver, constraint, from_pkg, from_ver
            ));
        }
    }

    explanation.push_str("\nConsider using a different version of one of these packages,\n");
    explanation.push_str("or relaxing the conflicting constraints.\n");

    explanation
}

#[cfg(test)]
mod tests {
    use super::super::tropical_math::MaxAtom;
    use super::*;

    #[test]
    fn exact_layer_remains_authoritative_over_tropical_filter() {
        let explainer = ConflictExplainer::new();
        let tropical = super::super::tropical_max_atom::MaxAtomResult::Unsatisfiable(
            super::super::tropical_max_atom::MaxAtomConflict {
                atoms: vec![],
                conflict_type: super::super::tropical_max_atom::ConflictType::DirectContradiction,
            },
        );
        let sheaf = SheafAnalysis {
            betti_numbers: vec![],
            is_snf: false,
            greedy_works: false,
            local_solutions: vec![],
            global_obstruction: None,
            mv_obstruction: None,
        };

        let report = explainer.explain_full(&tropical, &sheaf, true);
        assert!(report.contains("advisory obstruction"));
        assert!(report.contains("All hard constraints satisfied"));
    }

    #[test]
    fn test_derivation_graph() {
        let mut graph = DerivationGraph::new();

        let d = graph.add_node(DerivationNode::Decision {
            package: "A".to_string(),
            version: "1.0".to_string(),
            level: 1,
        });

        let c = graph.add_node(DerivationNode::Conflict {
            incompatibility: "A and B incompatible".to_string(),
            involved: vec!["A 1.0".to_string(), "B 1.0".to_string()],
        });

        graph.add_edge(d, c, "A 1.0 requires B 1.0".to_string());

        let explanation = graph.explain();
        assert!(explanation.contains("A and B incompatible"));
    }

    #[test]
    fn test_explain_max_atom_conflict() {
        let mut explainer = ConflictExplainer::new();

        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 1.0));
        system.add_atom(MaxAtom::new(1, 1, 0, 1.0));

        let conflict = super::super::tropical_max_atom::MaxAtomConflict {
            atoms: system.atoms.clone(),
            conflict_type: super::super::tropical_max_atom::ConflictType::PositiveCycle,
        };

        let explanation = explainer.explain_max_atom_conflict(&conflict, &system);
        assert!(explanation.contains("positive cycle"));
    }

    #[test]
    fn test_explain_sheaf_analysis() {
        let explainer = ConflictExplainer::new();
        let analysis = SheafAnalysis {
            betti_numbers: vec![1, 0, 0],
            is_snf: true,
            greedy_works: true,
            local_solutions: vec![],
            global_obstruction: None,
            mv_obstruction: None,
        };

        let explanation = explainer.explain_sheaf_analysis(&analysis);
        assert!(explanation.contains("β₁"));
    }

    #[test]
    fn test_explain_package_conflict() {
        let conflicts = vec![("B", "C >= 2.0", "A", "1.0")];
        let explanation = explain_package_conflict("A", "1.0", &conflicts);
        assert!(explanation.contains("A@1.0"));
    }
}
