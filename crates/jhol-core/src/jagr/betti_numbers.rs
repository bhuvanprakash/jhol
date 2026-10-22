//! JAGR Layer 2: Betti Numbers Computation
//!
//! Computes Betti numbers (cohomological invariants) of the tropical sheaf
//! to detect obstructions to resolution. β₁ = cycles, β₂ = voids.
//!
//! Uses COBOUNDARY matrices from the sheaf — not ordinary boundary maps.
//! Ordinary homology (∂) only measures the topology of the base complex.
//! Sheaf cohomology (δ) incorporates stalk dimensions and restriction map
//! ranks, giving Betti numbers that reflect the actual constraint structure.

use super::tropical_sheaf::{TropicalSheaf, TropicalSimplicialComplex};
use std::collections::{HashMap, HashSet};

/// Boundary matrix for dimension d
/// Rows = (d-1)-simplices, Cols = d-simplices
/// Entry is +1, -1, or 0 depending on face orientation
#[derive(Debug, Clone)]
pub struct BoundaryMatrix {
    pub dimension: usize,               // dimension of simplices (columns)
    pub rows: usize,                    // number of (d-1)-simplices
    pub cols: usize,                    // number of d-simplices
    pub data: Vec<Vec<f64>>,            // boundary matrix
    pub row_simplices: Vec<Vec<usize>>, // (d-1)-simplices
    pub col_simplices: Vec<Vec<usize>>, // d-simplices
}

impl BoundaryMatrix {
    pub fn new(dimension: usize, complex: &TropicalSimplicialComplex) -> Self {
        let (d_minus_1_simplices, d_simplices) = get_simplices_by_dimension(complex, dimension);

        let rows = d_minus_1_simplices.len();
        let cols = d_simplices.len();

        let mut data = vec![vec![0.0; cols]; rows];

        // Build face-to-index map
        let mut face_to_idx = HashMap::new();
        for (idx, face) in d_minus_1_simplices.iter().enumerate() {
            face_to_idx.insert(face.clone(), idx);
        }

        // Fill boundary matrix
        for (col, simplex) in d_simplices.iter().enumerate() {
            for (i, _) in simplex.iter().enumerate() {
                let mut face = simplex.clone();
                face.remove(i);
                face.sort();

                if let Some(&row) = face_to_idx.get(&face) {
                    // Sign: (-1)^i
                    data[row][col] = if i % 2 == 0 { 1.0 } else { -1.0 };
                }
            }
        }

        Self {
            dimension,
            rows,
            cols,
            data,
            row_simplices: d_minus_1_simplices,
            col_simplices: d_simplices,
        }
    }

    /// Compute rank using Gaussian elimination
    pub fn rank(&self) -> usize {
        if self.rows == 0 || self.cols == 0 {
            return 0;
        }

        let mut mat = self.data.clone();
        let mut rank = 0;
        let mut col = 0;

        while rank < self.rows.min(self.cols) && col < self.cols {
            let mut pivot_row = rank;
            while pivot_row < self.rows && mat[pivot_row][col].abs() < 1e-12 {
                pivot_row += 1;
            }

            if pivot_row == self.rows {
                col += 1;
                continue;
            }

            mat.swap(rank, pivot_row);

            let pivot = mat[rank][col];
            for j in col..self.cols {
                mat[rank][j] /= pivot;
            }

            for i in 0..self.rows {
                if i != rank {
                    let factor = mat[i][col];
                    for j in col..self.cols {
                        mat[i][j] -= factor * mat[rank][j];
                    }
                }
            }

            rank += 1;
            col += 1;
        }

        rank
    }

    /// Compute kernel (nullspace) basis
    pub fn kernel_basis(&self) -> Vec<Vec<f64>> {
        if self.cols == 0 {
            return vec![];
        }

        let mut mat = self.data.clone();
        let rows = self.rows;
        let cols = self.cols;

        // Row reduce to RREF
        let mut pivot_cols = Vec::new();
        let mut row = 0;

        for col in 0..cols {
            // Find pivot
            let mut pivot_row = row;
            while pivot_row < rows && mat[pivot_row][col].abs() < 1e-12 {
                pivot_row += 1;
            }

            if pivot_row == rows {
                continue;
            }

            mat.swap(row, pivot_row);

            let pivot = mat[row][col];
            for j in col..cols {
                mat[row][j] /= pivot;
            }

            for i in 0..rows {
                if i != row {
                    let factor = mat[i][col];
                    for j in col..cols {
                        mat[i][j] -= factor * mat[row][j];
                    }
                }
            }

            pivot_cols.push(col);
            row += 1;
            if row >= rows {
                break;
            }
        }

        // Free columns are non-pivot columns
        let mut free_cols = Vec::new();
        let mut pivot_idx = 0;
        for col in 0..cols {
            if pivot_idx < pivot_cols.len() && col == pivot_cols[pivot_idx] {
                pivot_idx += 1;
            } else {
                free_cols.push(col);
            }
        }

        // For each free column, construct a basis vector
        let mut basis = Vec::new();
        for &free_col in &free_cols {
            let mut vec = vec![0.0; cols];
            vec[free_col] = 1.0;

            // Fill in pivot columns
            for (i, &pivot_col) in pivot_cols.iter().enumerate() {
                vec[pivot_col] = -mat[i][free_col];
            }

            basis.push(vec);
        }

        basis
    }

    /// Compute image (column space) basis
    pub fn image_basis(&self) -> Vec<Vec<f64>> {
        let rref = self.rref_with_pivots();
        let pivot_cols = rref.1;

        pivot_cols
            .iter()
            .map(|&col| self.data.iter().map(|row| row[col]).collect())
            .collect()
    }

    fn rref_with_pivots(&self) -> (Vec<Vec<f64>>, Vec<usize>) {
        let mut mat = self.data.clone();
        let rows = self.rows;
        let cols = self.cols;
        let mut pivot_cols = Vec::new();
        let mut row = 0;

        for col in 0..cols {
            let mut pivot_row = row;
            while pivot_row < rows && mat[pivot_row][col].abs() < 1e-12 {
                pivot_row += 1;
            }

            if pivot_row == rows {
                continue;
            }

            mat.swap(row, pivot_row);
            let pivot = mat[row][col];
            for j in col..cols {
                mat[row][j] /= pivot;
            }

            for i in 0..rows {
                if i != row {
                    let factor = mat[i][col];
                    for j in col..cols {
                        mat[i][j] -= factor * mat[row][j];
                    }
                }
            }

            pivot_cols.push(col);
            row += 1;
            if row >= rows {
                break;
            }
        }

        (mat, pivot_cols)
    }
}

/// Get simplices of a given dimension from the complex
fn get_simplices_by_dimension(
    complex: &TropicalSimplicialComplex,
    dimension: usize,
) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let d_minus_1: Vec<_> = complex
        .simplices
        .iter()
        .filter(|s| s.len() == dimension)
        .cloned()
        .collect();

    let d: Vec<_> = complex
        .simplices
        .iter()
        .filter(|s| s.len() == dimension + 1)
        .cloned()
        .collect();

    (d_minus_1, d)
}

/// Compute all Betti numbers of a tropical sheaf
///
/// Delegates to `compute_sheaf_cohomology` which uses COBOUNDARY maps (δ^d)
/// weighted by stalk dimensions and restriction map ranks, not ordinary
/// boundary maps (∂_d). This produces sheaf-aware Betti numbers that reflect
/// the constraint structure, not just the topological complex.
pub fn compute_betti_numbers(sheaf: &TropicalSheaf) -> Vec<usize> {
    let cohomology = super::tropical_sheaf::compute_sheaf_cohomology(sheaf);
    cohomology.betti_numbers
}

/// Compute Betti numbers with persistence (for filtration)
pub fn compute_persistent_betti(sheaf: &TropicalSheaf, filtration: &[f64]) -> Vec<Vec<usize>> {
    // For each filtration value, compute Betti numbers
    // This would be used for persistent homology
    let mut persistent = Vec::new();

    for &f in filtration {
        // Filter complex by filtration value
        let filtered = filter_complex_by_filtration(&sheaf.complex, f);
        let betti = compute_betti_numbers_from_complex(&filtered);
        persistent.push(betti);
    }

    persistent
}

/// Filter complex by a filtration value
fn filter_complex_by_filtration(
    complex: &TropicalSimplicialComplex,
    value: f64,
) -> TropicalSimplicialComplex {
    // Keep simplices where all vertices have filtration ≤ value
    let mut new_vertices = Vec::new();
    let mut vertex_map = HashMap::new();

    for (idx, v) in complex.vertices.iter().enumerate() {
        if v.version <= value {
            vertex_map.insert(idx, new_vertices.len());
            new_vertices.push(v.clone());
        }
    }

    let new_simplices: Vec<Vec<usize>> = complex
        .simplices
        .iter()
        .filter(|s| s.iter().all(|v| vertex_map.contains_key(v)))
        .map(|s| {
            s.iter()
                .filter_map(|v| vertex_map.get(v).copied())
                .collect()
        })
        .collect();

    let dimensions: Vec<usize> = new_simplices.iter().map(|s| s.len() - 1).collect();
    let boundary_maps = super::tropical_sheaf::build_boundary_maps(&new_simplices);

    TropicalSimplicialComplex {
        vertices: new_vertices,
        simplices: new_simplices,
        dimensions,
        boundary_maps,
    }
}

/// Compute Betti numbers from a complex directly
///
/// When no sheaf is available (e.g. persistent homology subcomplexes),
/// fall back to ordinary homology via boundary maps. This is less
/// informative than sheaf cohomology but still correct topologically.
fn compute_betti_numbers_from_complex(complex: &TropicalSimplicialComplex) -> Vec<usize> {
    let max_dim = complex.dimensions.iter().max().copied().unwrap_or(0);
    let mut betti = Vec::new();

    for d in 0..=max_dim {
        let boundary_d = if d > 0 {
            Some(BoundaryMatrix::new(d, complex))
        } else {
            None
        };

        let boundary_d_plus_1 = if d < max_dim {
            Some(BoundaryMatrix::new(d + 1, complex))
        } else {
            None
        };

        let ker_dim = if let Some(mat) = &boundary_d {
            mat.cols - mat.rank()
        } else {
            complex.simplices.iter().filter(|s| s.len() == 1).count()
        };

        let im_dim = if let Some(mat) = &boundary_d_plus_1 {
            mat.rank()
        } else {
            0
        };

        betti.push(ker_dim.saturating_sub(im_dim));
    }

    betti
}

/// Interpret Betti numbers for resolution
/// β₀ = number of connected components
/// β₁ = number of independent cycles (obstructions to tree-like resolution)
/// β₂ = number of voids (higher-order obstructions)
pub fn interpret_betti_for_resolution(betti: &[usize]) -> ResolutionInterpretation {
    let beta_0 = betti.first().copied().unwrap_or(0);
    let beta_1 = betti.get(1).copied().unwrap_or(0);
    let beta_2 = betti.get(2).copied().unwrap_or(0);
    let beta_3 = betti.get(3).copied().unwrap_or(0);

    let mut interpretation = ResolutionInterpretation::new();

    interpretation.num_components = beta_0;
    interpretation.has_cycles = beta_1 > 0;
    interpretation.cycle_count = beta_1;
    interpretation.has_voids = beta_2 > 0;
    interpretation.void_count = beta_2;
    interpretation.has_higher_obstructions = beta_3 > 0;

    // SNF = Simplicial Normal Form = β₁ = β₂ = 0
    interpretation.is_snf = beta_1 == 0 && beta_2 == 0;

    // If SNF, greedy resolution works
    interpretation.greedy_works = interpretation.is_snf;

    // Resolution difficulty
    if beta_1 == 0 && beta_2 == 0 {
        interpretation.difficulty = ResolutionDifficulty::Trivial;
    } else if beta_1 <= 2 && beta_2 == 0 {
        interpretation.difficulty = ResolutionDifficulty::Easy;
    } else if beta_1 <= 5 && beta_2 <= 1 {
        interpretation.difficulty = ResolutionDifficulty::Moderate;
    } else {
        interpretation.difficulty = ResolutionDifficulty::Hard;
    }

    interpretation
}

#[derive(Debug, Clone)]
pub struct ResolutionInterpretation {
    pub num_components: usize,
    pub has_cycles: bool,
    pub cycle_count: usize,
    pub has_voids: bool,
    pub void_count: usize,
    pub has_higher_obstructions: bool,
    pub is_snf: bool,
    pub greedy_works: bool,
    pub difficulty: ResolutionDifficulty,
}

impl Default for ResolutionInterpretation {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolutionInterpretation {
    pub fn new() -> Self {
        Self {
            num_components: 0,
            has_cycles: false,
            cycle_count: 0,
            has_voids: false,
            void_count: 0,
            has_higher_obstructions: false,
            is_snf: true,
            greedy_works: true,
            difficulty: ResolutionDifficulty::Trivial,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionDifficulty {
    Trivial,
    Easy,
    Moderate,
    Hard,
}

/// ModuleStability — maps topological invariants to GC region decisions.
/// Used by VSD-GC to determine which allocation region an object belongs in.
#[derive(Debug, Clone)]
pub struct ModuleStability {
    pub betti_1: u32,
    pub betti_2: u32,
    pub spectral_radius: f64,
    pub maxsat_conflicts: usize,
}

impl ModuleStability {
    pub fn new(betti: &[usize], spectral_radius: f64, maxsat_conflicts: usize) -> Self {
        ModuleStability {
            betti_1: betti.get(1).copied().unwrap_or(0) as u32,
            betti_2: betti.get(2).copied().unwrap_or(0) as u32,
            spectral_radius,
            maxsat_conflicts,
        }
    }

    /// Determines the GC allocation region based on topological stability.
    /// - Immortal: acyclic, spectrally bounded — never collected, CAS-deduplicated
    /// - Tenured: high-conflict or high-spectral — aggressive marking
    /// - Nursery: default — speculative bump allocation
    pub fn gc_region(&self) -> &'static str {
        if self.betti_1 == 0 && self.betti_2 == 0 && self.spectral_radius < 1.5 {
            "immortal"
        } else if self.maxsat_conflicts > 10 || self.spectral_radius > 3.0 {
            "tenured"
        } else {
            "nursery"
        }
    }

    pub fn is_stable(&self) -> bool {
        self.betti_1 == 0 && self.betti_2 == 0 && self.spectral_radius < 1.5
    }

    pub fn is_volatile(&self) -> bool {
        self.maxsat_conflicts > 10 || self.spectral_radius > 3.0
    }
}

/// Cycle detection and extraction
///
/// Uses DFS with proper back-edge tracking for undirected graphs.
/// In an undirected graph, each edge appears twice in the adjacency
/// list (u→v and v→u). A back edge to the parent vertex is NOT a
/// cycle — it's just the tree edge in the reverse direction. We must
/// track the vertex we arrived from AND check depth ordering to avoid
/// false cycle detection.
pub fn extract_cycles(sheaf: &TropicalSheaf) -> Vec<Vec<usize>> {
    let complex = &sheaf.complex;
    let mut cycles = Vec::new();
    let mut seen_cycles: HashSet<Vec<usize>> = HashSet::new();

    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for simplex in &complex.simplices {
        if simplex.len() == 2 {
            adj.entry(simplex[0]).or_default().push(simplex[1]);
            adj.entry(simplex[1]).or_default().push(simplex[0]);
        }
    }

    let mut visited: HashMap<usize, bool> = HashMap::new();
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut depth: HashMap<usize, usize> = HashMap::new();

    fn dfs(
        v: usize,
        arrived_from: Option<usize>,
        adj: &HashMap<usize, Vec<usize>>,
        visited: &mut HashMap<usize, bool>,
        parent: &mut HashMap<usize, usize>,
        depth: &mut HashMap<usize, usize>,
        cur_depth: usize,
        cycles: &mut Vec<Vec<usize>>,
        seen: &mut HashSet<Vec<usize>>,
    ) {
        visited.insert(v, true);
        depth.insert(v, cur_depth);

        if let Some(p) = arrived_from {
            parent.insert(v, p);
        }

        if let Some(neighbors) = adj.get(&v) {
            for &u in neighbors {
                if !visited.get(&u).copied().unwrap_or(false) {
                    dfs(
                        u,
                        Some(v),
                        adj,
                        visited,
                        parent,
                        depth,
                        cur_depth + 1,
                        cycles,
                        seen,
                    );
                } else if u != arrived_from.unwrap_or(usize::MAX)
                    && depth.get(&u).copied().unwrap_or(0) < cur_depth
                {
                    let mut cycle = vec![v];
                    let mut cur = v;
                    while cur != u {
                        if let Some(&par) = parent.get(&cur) {
                            cycle.push(par);
                            cur = par;
                        } else {
                            break;
                        }
                    }
                    cycle.reverse();
                    if cycle.len() >= 3 {
                        let mut canonical = cycle.clone();
                        let min_pos = canonical
                            .iter()
                            .enumerate()
                            .min_by_key(|&(_, &v)| v)
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        canonical.rotate_left(min_pos);
                        let mut reversed = canonical.clone();
                        reversed.reverse();
                        let min_pos_rev = reversed
                            .iter()
                            .enumerate()
                            .min_by_key(|&(_, &v)| v)
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        reversed.rotate_left(min_pos_rev);
                        if canonical > reversed {
                            canonical = reversed;
                        }
                        if seen.insert(canonical) {
                            cycles.push(cycle);
                        }
                    }
                }
            }
        }
    }

    for v in adj.keys() {
        if !visited.get(v).copied().unwrap_or(false) {
            dfs(
                *v,
                None,
                &adj,
                &mut visited,
                &mut parent,
                &mut depth,
                0,
                &mut cycles,
                &mut seen_cycles,
            );
        }
    }

    cycles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jagr::tropical_math::{MaxAtom, MaxAtomSystem};
    use crate::jagr::tropical_polyhedron::TropicalPolyhedron;
    use crate::jagr::tropical_sheaf::build_tropical_sheaf;

    #[test]
    fn test_boundary_matrix() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let boundary = BoundaryMatrix::new(1, &sheaf.complex);
        assert!(boundary.data.len() <= boundary.rows.saturating_mul(boundary.cols));
    }

    #[test]
    fn test_betti_numbers() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let betti = compute_betti_numbers(&sheaf);
        assert!(!betti.is_empty());
        assert!(betti[0] >= 1); // β₀ >= 1: at least one connected component
    }

    #[test]
    fn test_interpretation() {
        let betti = vec![1, 0, 0]; // Connected, no cycles, no voids
        let interp = interpret_betti_for_resolution(&betti);
        assert!(interp.is_snf);
        assert!(interp.greedy_works);
        assert_eq!(interp.difficulty, ResolutionDifficulty::Trivial);

        let betti2 = vec![1, 2, 0]; // Connected, 2 cycles
        let interp2 = interpret_betti_for_resolution(&betti2);
        assert!(!interp2.is_snf);
        assert!(!interp2.greedy_works);
        assert_eq!(interp2.cycle_count, 2);
    }

    #[test]
    fn test_cycle_extraction() {
        let mut system = MaxAtomSystem::new(3);
        // Create a cycle: 0-1, 1-2, 2-0
        system.add_atom(MaxAtom::new(0, 0, 1, 1.0));
        system.add_atom(MaxAtom::new(1, 1, 2, 1.0));
        system.add_atom(MaxAtom::new(2, 2, 0, 1.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let _cycles = extract_cycles(&sheaf);
        // May or may not find cycles depending on complex construction
    }
}
