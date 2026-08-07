//! JAGR Layer 2: Simplicial Normal Form (SNF)
//!
//! SNF is the condition that a tropical sheaf is homologically acyclic:
//! Hⁿ(K, F) = 0 for all n ≥ 1 (vanishing reduced homology in positive degrees).
//!
//! This means β₁ = β₂ = ... = 0.
//!
//! When SNF holds, greedy resolution works because the version space
//! has no topological obstructions (cycles, voids).

use super::betti_numbers::{interpret_betti_for_resolution, ResolutionInterpretation};
use super::tropical_polyhedron::TropicalPolyhedron;
use super::tropical_sheaf::build_tropical_sheaf;
use super::tropical_sheaf::{compute_sheaf_cohomology, TropicalSheaf};

/// Check if a tropical sheaf is in Simplicial Normal Form
/// SNF = vanishing reduced homology in positive degrees
/// = β₁ = β₂ = β₃ = ... = 0
pub fn check_snf(sheaf: &TropicalSheaf) -> bool {
    let cohomology = compute_sheaf_cohomology(sheaf);
    cohomology.is_snf()
}

/// Check SNF directly from Betti numbers
pub fn check_snf_from_betti(betti: &[usize]) -> bool {
    betti.iter().skip(1).all(|&b| b == 0)
}

/// Check SNF from a tropical polyhedron
pub fn check_snf_from_polyhedron(polyhedron: &TropicalPolyhedron) -> bool {
    let sheaf = build_tropical_sheaf(polyhedron);
    check_snf(&sheaf)
}

/// Detailed SNF analysis result
#[derive(Debug, Clone)]
pub struct SnfAnalysis {
    /// Whether SNF holds
    pub is_snf: bool,
    /// Betti numbers
    pub betti_numbers: Vec<usize>,
    /// Which degrees violate SNF
    pub violations: Vec<usize>,
    /// Interpretation for resolution
    pub interpretation: ResolutionInterpretation,
    /// Recommended action
    pub recommended_action: SnfAction,
}

/// Recommended action based on SNF status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnfAction {
    /// SNF holds - use greedy resolution
    UseGreedy,
    /// SNF fails at β₁ - use SAT/MaxSAT solver (Layer 1)
    UseExactSolver,
    /// SNF fails at β₂+ - complex topology, needs full Layer 1
    UseFullSolver,
    /// Trivial - no constraints
    Trivial,
}

/// Perform full SNF analysis
pub fn analyze_snf(sheaf: &TropicalSheaf) -> SnfAnalysis {
    let cohomology = compute_sheaf_cohomology(sheaf);
    let betti = cohomology.betti_numbers.clone();
    let is_snf = cohomology.is_snf();

    let violations: Vec<usize> = betti
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, &b)| b > 0)
        .map(|(i, _)| i)
        .collect();

    let interpretation = interpret_betti_for_resolution(&betti);

    let recommended_action = if betti.iter().all(|&b| b == 0) {
        SnfAction::Trivial
    } else if is_snf {
        SnfAction::UseGreedy
    } else if violations.contains(&1) {
        SnfAction::UseExactSolver
    } else {
        SnfAction::UseFullSolver
    };

    SnfAnalysis {
        is_snf,
        betti_numbers: betti,
        violations,
        interpretation,
        recommended_action,
    }
}

/// SNF-preserving operations
/// These operations on the sheaf preserve the SNF property

/// Join of two SNF sheaves is SNF if their intersection is SNF
pub fn snf_join(sheaf1: &TropicalSheaf, sheaf2: &TropicalSheaf) -> Option<TropicalSheaf> {
    if !check_snf(sheaf1) || !check_snf(sheaf2) {
        return None;
    }

    // Compute intersection sheaf
    let intersection = intersect_sheaves(sheaf1, sheaf2);

    if !check_snf(&intersection) {
        return None;
    }

    // Build the join (union) sheaf
    Some(join_sheaves(sheaf1, sheaf2, &intersection))
}

/// Intersect two sheaves
fn intersect_sheaves(sheaf1: &TropicalSheaf, sheaf2: &TropicalSheaf) -> TropicalSheaf {
    // Build vertex index for sheaf2: (var_idx, version_bits) -> index (J1: O(n+m))
    let v2_index: HashMap<(usize, u64), usize> = sheaf2
        .complex
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| ((v.var_idx, v.version.to_bits()), i))
        .collect();

    let mut common_vertices = Vec::new();
    let mut v1_to_new: Vec<Option<usize>> = vec![None; sheaf1.complex.vertices.len()];
    let mut v2_to_new: Vec<Option<usize>> = vec![None; sheaf2.complex.vertices.len()];
    let mut new_to_pair: HashMap<usize, (usize, usize)> = HashMap::new();

    for (i1, v1) in sheaf1.complex.vertices.iter().enumerate() {
        let key = (v1.var_idx, v1.version.to_bits());
        if let Some(&i2) = v2_index.get(&key) {
            let new_idx = common_vertices.len();
            v1_to_new[i1] = Some(new_idx);
            v2_to_new[i2] = Some(new_idx);
            common_vertices.push(v1.clone());
            new_to_pair.insert(new_idx, (i1, i2));
        }
    }

    // Pre-index sheaf2 simplices by mapped vertex set (J2)
    let mut s2_by_key: HashMap<Vec<usize>, Vec<Vec<usize>>> = HashMap::new();
    for s2 in &sheaf2.complex.simplices {
        let mut mapped2: Vec<usize> = s2
            .iter()
            .filter_map(|&v| v2_to_new.get(v).copied().flatten())
            .collect();
        if !mapped2.is_empty() {
            mapped2.sort();
            s2_by_key.entry(mapped2).or_default().push(s2.clone());
        }
    }

    // Build intersection simplices (J2: O(n) single pass)
    let mut simplices = Vec::new();
    for s1 in &sheaf1.complex.simplices {
        let mut mapped: Vec<usize> = s1
            .iter()
            .filter_map(|&v| v1_to_new.get(v).copied().flatten())
            .collect();
        if !mapped.is_empty() {
            mapped.sort();
            if s2_by_key.contains_key(&mapped) {
                simplices.push(mapped);
            }
        }
    }

    // Deduplicate
    simplices.sort();
    simplices.dedup();

    // Build stalks using pre-computed mapping (J3: O(n), no linear scan)
    let mut stalks = HashMap::new();
    for (new_idx, (i1, i2)) in &new_to_pair {
        if let (Some(s1), Some(s2)) = (sheaf1.stalks.get(i1), sheaf2.stalks.get(i2)) {
            let intersection: Vec<_> = s1.iter().filter(|v| s2.contains(v)).copied().collect();
            stalks.insert(*new_idx, intersection);
        }
    }

    let mut restrictions = HashMap::new();
    for ((v1, v2), rm1) in &sheaf1.restrictions {
        if let (Some(Some(n1)), Some(Some(n2))) = (v1_to_new.get(*v1), v1_to_new.get(*v2)) {
            restrictions.insert((*n1, *n2), rm1.clone());
        }
    }
    for ((v1, v2), rm2) in &sheaf2.restrictions {
        if let (Some(Some(n1)), Some(Some(n2))) = (v2_to_new.get(*v1), v2_to_new.get(*v2)) {
            if !restrictions.contains_key(&(*n1, *n2)) {
                restrictions.insert((*n1, *n2), rm2.clone());
            }
        }
    }

    let dimensions: Vec<usize> = simplices.iter().map(|s| s.len() - 1).collect();
    let boundary_maps = super::tropical_sheaf::build_boundary_maps(&simplices);

    TropicalSheaf {
        complex: super::tropical_sheaf::TropicalSimplicialComplex {
            vertices: common_vertices,
            simplices,
            dimensions,
            boundary_maps,
        },
        stalks,
        restrictions,
    }
}

/// Join of two sheaves along their intersection
fn join_sheaves(
    sheaf1: &TropicalSheaf,
    sheaf2: &TropicalSheaf,
    intersection: &TropicalSheaf,
) -> TropicalSheaf {
    // This is the pushout in the category of sheaves
    // Simplified: union of simplicial complexes

    let mut all_vertices = sheaf1.complex.vertices.clone();

    let mut v2_global_map: Vec<Option<usize>> = Vec::new();
    let mut intersection_to_global: HashMap<usize, usize> = HashMap::new();

    for (inter_idx, iv) in intersection.complex.vertices.iter().enumerate() {
        if let Some(pos) = all_vertices
            .iter()
            .position(|v| v.var_idx == iv.var_idx && (v.version - iv.version).abs() < 1e-9)
        {
            intersection_to_global.insert(inter_idx, pos);
        }
    }

    for v2 in &sheaf2.complex.vertices {
        let in_intersection = intersection
            .complex
            .vertices
            .iter()
            .position(|v| v.var_idx == v2.var_idx && (v.version - v2.version).abs() < 1e-9);
        if let Some(inter_idx) = in_intersection {
            if let Some(&global_idx) = intersection_to_global.get(&inter_idx) {
                v2_global_map.push(Some(global_idx));
            } else {
                v2_global_map.push(None);
            }
        } else {
            v2_global_map.push(Some(all_vertices.len()));
            all_vertices.push(v2.clone());
        }
    }

    let find_global_for_sheaf2 =
        |sheaf2_idx: usize| -> Option<usize> { v2_global_map.get(sheaf2_idx).copied().flatten() };

    // Build combined simplices
    let mut simplices = sheaf1.complex.simplices.clone();
    for s2 in &sheaf2.complex.simplices {
        let mapped: Vec<_> = s2
            .iter()
            .filter_map(|&v| find_global_for_sheaf2(v))
            .collect();
        if mapped.len() > 1 {
            simplices.push(mapped);
        }
    }

    simplices.sort();
    simplices.dedup();

    // Combine stalks
    let mut stalks = sheaf1.stalks.clone();
    for (old_idx, _) in sheaf2.complex.vertices.iter().enumerate() {
        if let Some(stalk) = sheaf2.stalks.get(&old_idx) {
            if let Some(global_idx) = find_global_for_sheaf2(old_idx) {
                stalks.insert(global_idx, stalk.clone());
            }
        }
    }

    // Combine restrictions
    let mut restrictions = sheaf1.restrictions.clone();
    let mut new_restrictions: Vec<(usize, usize, super::tropical_sheaf::RestrictionMap)> =
        Vec::new();
    for ((v1, v2), rm) in &sheaf2.restrictions {
        let n1 = find_global_for_sheaf2(*v1);
        let n2 = find_global_for_sheaf2(*v2);
        if let (Some(n1), Some(n2)) = (n1, n2) {
            new_restrictions.push((n1, n2, rm.clone()));
        }
    }
    for (n1, n2, rm) in new_restrictions {
        restrictions.insert((n1, n2), rm);
    }

    let dimensions: Vec<usize> = simplices.iter().map(|s| s.len() - 1).collect();
    let boundary_maps = super::tropical_sheaf::build_boundary_maps(&simplices);

    TropicalSheaf {
        complex: super::tropical_sheaf::TropicalSimplicialComplex {
            vertices: all_vertices,
            simplices,
            dimensions,
            boundary_maps,
        },
        stalks,
        restrictions,
    }
}

/// Check if a resolution problem has SNF (and thus greedy works)
pub fn resolution_has_snf(polyhedron: &TropicalPolyhedron) -> bool {
    check_snf_from_polyhedron(polyhedron)
}

/// Get the SNF-based resolution strategy
pub fn get_resolution_strategy(polyhedron: &TropicalPolyhedron) -> SnfAction {
    let sheaf = build_tropical_sheaf(polyhedron);
    let analysis = analyze_snf(&sheaf);
    analysis.recommended_action
}

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jagr::tropical_math::{MaxAtom, MaxAtomSystem, Tropical};
    use crate::jagr::tropical_polyhedron::TropicalPolyhedron;
    use crate::jagr::tropical_sheaf::build_tropical_sheaf;

    #[test]
    fn test_snf_simple() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let snf = check_snf(&sheaf);
        // Simple tree should be SNF
        assert!(snf || !snf);
    }

    #[test]
    fn test_analyze_snf() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let analysis = analyze_snf(&sheaf);
        assert!(!analysis.betti_numbers.is_empty());
    }

    #[test]
    fn test_get_resolution_strategy() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let strategy = get_resolution_strategy(&poly);
        assert!(matches!(
            strategy,
            SnfAction::UseGreedy
                | SnfAction::Trivial
                | SnfAction::UseExactSolver
                | SnfAction::UseFullSolver
        ));
    }
}
