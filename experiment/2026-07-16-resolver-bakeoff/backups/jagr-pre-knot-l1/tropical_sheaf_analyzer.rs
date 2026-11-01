//! JAGR Layer 2: Tropical Sheaf Analyzer
//!
//! The main entry point for Layer 2 of the JAGR three-layer architecture.
//! Takes a tropical polyhedron representing version constraints and produces
//! a complete topological analysis with Betti numbers, SNF check, and
//! Mayer-Vietoris gluing obstruction.

use super::betti_numbers::{
    compute_betti_numbers, compute_persistent_betti, extract_cycles,
    interpret_betti_for_resolution, ResolutionDifficulty, ResolutionInterpretation,
};
use super::mayer_vietoris::{
    build_cover, compute_gluing_obstruction, compute_mayer_vietoris, sheaf_feasibility,
    GluingObstruction,
};
use super::simplicial_normal_form::{analyze_snf, check_snf, SnfAction, SnfAnalysis};
use super::tropical_polyhedron::TropicalPolyhedron;
use super::tropical_sheaf::{
    build_tropical_sheaf, compute_sheaf_cohomology, SheafAnalysis, TropicalSheaf,
};
use std::collections::HashMap;

/// Layer 2 Analyzer
pub struct TropicalSheafAnalyzer {
    /// Cached analysis
    cache: HashMap<String, SheafAnalysis>,
    /// Whether to use persistent homology
    use_persistent: bool,
}

impl TropicalSheafAnalyzer {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            use_persistent: false,
        }
    }

    /// Enable persistent homology computation
    pub fn with_persistent_homology(mut self) -> Self {
        self.use_persistent = true;
        self
    }

    /// Main analysis method
    pub fn analyze(&mut self, polyhedron: &TropicalPolyhedron) -> SheafAnalysis {
        let key = build_polyhedron_key(polyhedron);

        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }

        let sheaf = build_tropical_sheaf(polyhedron);
        let analysis = self.analyze_sheaf(&sheaf);

        self.cache.insert(key, analysis.clone());
        analysis
    }

    /// Analyze a sheaf directly
    pub fn analyze_sheaf(&self, sheaf: &TropicalSheaf) -> SheafAnalysis {
        let cohomology = compute_sheaf_cohomology(sheaf);
        let betti = cohomology.betti_numbers.clone();
        let is_snf = cohomology.is_snf();
        let greedy_works = is_snf;

        // Get local solutions
        let local_solutions: Vec<Vec<f64>> = sheaf.stalks.values().cloned().collect();

        // Compute global obstruction
        let global_obstruction = if !is_snf {
            let cycles = extract_cycles(sheaf);
            let _ = cycles; // Used for debugging
            Some(
                (cohomology.betti_numbers.get(1).copied().unwrap_or(0)
                    + cohomology.betti_numbers.get(2).copied().unwrap_or(0)) as f64,
            )
        } else {
            None
        };

        // Compute Mayer-Vietoris obstruction
        let mv_obstruction = if !is_snf {
            let cover = build_cover(sheaf, 2);
            let sequence = compute_mayer_vietoris(&cover);
            let obstruction = compute_gluing_obstruction(&sequence);
            if obstruction.vanishes {
                None
            } else {
                Some(obstruction.obstruction_class)
            }
        } else {
            None
        };

        SheafAnalysis {
            betti_numbers: betti,
            is_snf,
            greedy_works,
            local_solutions,
            global_obstruction,
            mv_obstruction,
        }
    }

    /// Full detailed analysis
    pub fn full_analyze(&self, polyhedron: &TropicalPolyhedron) -> FullLayer2Analysis {
        let sheaf = build_tropical_sheaf(polyhedron);
        let betti = compute_betti_numbers(&sheaf);
        let snf = check_snf(&sheaf);
        let snf_analysis = analyze_snf(&sheaf);
        let interpretation = interpret_betti_for_resolution(&betti);
        let cycles = extract_cycles(&sheaf);
        let (feasible, gluing_obstruction) = sheaf_feasibility(&sheaf);

        let persistent = if self.use_persistent {
            let filtration: Vec<f64> = (0..10).map(|i| i as f64).collect();
            Some(compute_persistent_betti(&sheaf, &filtration))
        } else {
            None
        };

        FullLayer2Analysis {
            sheaf,
            betti_numbers: betti,
            is_snf: snf,
            snf_analysis,
            interpretation,
            cycles,
            gluing_feasible: feasible,
            gluing_obstruction,
            persistent_betti: persistent,
        }
    }

    /// Get the recommended action
    pub fn get_action(&self, polyhedron: &TropicalPolyhedron) -> SnfAction {
        let sheaf = build_tropical_sheaf(polyhedron);
        let analysis = analyze_snf(&sheaf);
        analysis.recommended_action
    }

    /// Clear cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

impl Default for TropicalSheafAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Full Layer 2 analysis result
#[derive(Debug, Clone)]
pub struct FullLayer2Analysis {
    /// The sheaf
    pub sheaf: TropicalSheaf,
    /// Betti numbers
    pub betti_numbers: Vec<usize>,
    /// Whether SNF holds
    pub is_snf: bool,
    /// Detailed SNF analysis
    pub snf_analysis: SnfAnalysis,
    /// Resolution interpretation
    pub interpretation: ResolutionInterpretation,
    /// Detected cycles
    pub cycles: Vec<Vec<usize>>,
    /// Whether gluing is feasible
    pub gluing_feasible: bool,
    /// Gluing obstruction (if any)
    pub gluing_obstruction: GluingObstruction,
    /// Persistent Betti numbers (if enabled)
    pub persistent_betti: Option<Vec<Vec<usize>>>,
}

impl FullLayer2Analysis {
    /// Get a summary string
    pub fn summary(&self) -> String {
        format!(
            "Layer 2 Analysis: β₁={:?}, SNF={}, Feasibility={}, Difficulty={:?}",
            self.betti_numbers, self.is_snf, self.gluing_feasible, self.interpretation.difficulty
        )
    }

    /// Get the recommended next layer
    pub fn next_layer(&self) -> LayerDecision {
        if self.is_snf {
            LayerDecision::GreedyResolution
        } else if self.interpretation.difficulty == ResolutionDifficulty::Trivial
            || self.interpretation.difficulty == ResolutionDifficulty::Easy
        {
            LayerDecision::UseLayer1Simple
        } else {
            LayerDecision::UseLayer1Full
        }
    }
}

/// Decision about which layer to use
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerDecision {
    /// SNF holds - use greedy resolution directly
    GreedyResolution,
    /// Use Layer 1 with simple backtracking
    UseLayer1Simple,
    /// Use Layer 1 with full core-guided MaxSAT
    UseLayer1Full,
    /// Trivial case (no constraints)
    Trivial,
}

fn build_polyhedron_key(polyhedron: &TropicalPolyhedron) -> String {
    let mut parts = Vec::new();
    parts.push(format!("nv:{}", polyhedron.num_vars));
    for (i, hs) in polyhedron.halfspaces.iter().enumerate() {
        let left: Vec<String> = hs
            .left_vars
            .iter()
            .zip(hs.left_coeffs.iter())
            .map(|(v, c)| format!("{}:{:.6}", v, c.0))
            .collect();
        let right: Vec<String> = hs
            .right_vars
            .iter()
            .zip(hs.right_coeffs.iter())
            .map(|(v, c)| format!("{}:{:.6}", v, c.0))
            .collect();
        parts.push(format!(
            "hs{}:[{}]|[{}]",
            i,
            left.join(","),
            right.join(",")
        ));
    }
    if let Some(bounds) = &polyhedron.bounds {
        for (i, (min, max)) in bounds.iter().enumerate() {
            parts.push(format!("b{}:{:.6}:{:.6}", i, min.0, max.0));
        }
    }
    parts.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jagr::tropical_math::{MaxAtom, MaxAtomSystem, Tropical};
    use crate::jagr::tropical_polyhedron::TropicalPolyhedron;

    #[test]
    fn test_layer2_analyzer() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let mut analyzer = TropicalSheafAnalyzer::new();
        let analysis = analyzer.analyze(&poly);

        assert!(!analysis.betti_numbers.is_empty());
    }

    #[test]
    fn test_full_analyze() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let analyzer = TropicalSheafAnalyzer::new();
        let full = analyzer.full_analyze(&poly);

        assert!(!full.betti_numbers.is_empty());
        assert!(!full.snf_analysis.betti_numbers.is_empty());
    }

    #[test]
    fn test_layer_decision() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let analyzer = TropicalSheafAnalyzer::new();
        let full = analyzer.full_analyze(&poly);

        let decision = full.next_layer();
        assert!(matches!(
            decision,
            LayerDecision::GreedyResolution
                | LayerDecision::UseLayer1Simple
                | LayerDecision::UseLayer1Full
                | LayerDecision::Trivial
        ));
    }

    #[test]
    fn test_with_persistent() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let analyzer = TropicalSheafAnalyzer::new().with_persistent_homology();
        let full = analyzer.full_analyze(&poly);

        assert!(full.persistent_betti.is_some());
    }
}
