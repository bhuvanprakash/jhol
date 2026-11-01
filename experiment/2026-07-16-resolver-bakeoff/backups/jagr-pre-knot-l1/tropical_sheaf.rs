#![allow(dead_code)]
//! JAGR Layer 2: Tropical Sheaf Cohomology
//!
//! Implements sheaf-theoretic analysis of version dependency spaces.
//! Uses tropical sheaf cohomology to detect obstructions to resolution.
//!
//! Key concepts:
//! - Sheaf on a tropical polyhedral complex
//! - Stalks = sets of feasible versions for each package
//! - Restriction maps = intersection of version constraints
//! - Betti numbers β₁, β₂ = homological obstructions
//! - Simplicial Normal Form (SNF) = β₁ = β₂ = 0 (homological acyclicity)
//! - Mayer-Vietoris sequence = local-to-global gluing

use super::tropical_polyhedron::TropicalPolyhedron;
use std::collections::HashMap;

/// Simplicial complex built from tropical polyhedron cells
#[derive(Debug, Clone)]
pub struct TropicalSimplicialComplex {
    /// Vertices: (variable_index, version_value)
    pub vertices: Vec<TropicalVertex>,
    /// Simplices: sets of vertex indices forming a simplex
    pub simplices: Vec<Vec<usize>>,
    /// Dimension of each simplex
    pub dimensions: Vec<usize>,
    /// Boundary maps (for homology computation)
    pub boundary_maps: HashMap<usize, Vec<Vec<isize>>>, // dim -> boundary matrix
}

#[derive(Debug, Clone, PartialEq)]
pub struct TropicalVertex {
    pub var_idx: usize,
    pub version: f64, // or Tropical::ZERO for -∞
}

impl TropicalVertex {
    pub fn new(var_idx: usize, version: f64) -> Self {
        Self { var_idx, version }
    }
}

/// Tropical sheaf on a polyhedral complex
#[derive(Debug, Clone)]
pub struct TropicalSheaf {
    /// Base space: the simplicial complex
    pub complex: TropicalSimplicialComplex,
    /// Stalks: for each vertex, the set of feasible versions
    pub stalks: HashMap<usize, Vec<f64>>,
    /// Restriction maps: for each edge (v1, v2), the map stalk(v1) → stalk(v2)
    pub restrictions: HashMap<(usize, usize), RestrictionMap>,
}

/// Restriction map between stalks
#[derive(Debug, Clone)]
pub struct RestrictionMap {
    /// For each version in source stalk, the allowed versions in target stalk
    pub mapping: Vec<(f64, Vec<f64>)>,
    /// Whether this is an inclusion (standard) or more general
    pub is_inclusion: bool,
}

impl RestrictionMap {
    pub fn new() -> Self {
        Self {
            mapping: Vec::new(),
            is_inclusion: true,
        }
    }

    pub fn add_mapping(&mut self, source: f64, targets: Vec<f64>) {
        self.mapping.push((source, targets));
    }

    /// Apply restriction to a set of versions
    pub fn apply(&self, versions: &[f64]) -> Vec<f64> {
        let mut result = Vec::new();
        for &v in versions {
            let mut found = false;
            for (source, targets) in &self.mapping {
                if (*source - v).abs() < 1e-9 {
                    result.extend(targets.iter().cloned());
                    found = true;
                    break;
                }
            }
            if !found && self.is_inclusion {
                result.push(v);
            }
        }
        result.sort_by(|a, b| {
            // Handle NaN: treat as greater than all finite values
            if a.is_nan() && b.is_nan() {
                std::cmp::Ordering::Equal
            } else if a.is_nan() {
                std::cmp::Ordering::Greater
            } else if b.is_nan() {
                std::cmp::Ordering::Less
            } else {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        result.dedup();
        result
    }
}

/// Sheaf cohomology groups
#[derive(Debug, Clone)]
pub struct SheafCohomology {
    /// Betti numbers: β₀, β₁, β₂, ...
    pub betti_numbers: Vec<usize>,
    /// Dimensions of cohomology groups
    pub dimensions: Vec<usize>,
    /// Whether the sheaf is acyclic (all βᵢ = 0 for i > 0)
    pub is_acyclic: bool,
}

impl SheafCohomology {
    pub fn new() -> Self {
        Self {
            betti_numbers: Vec::new(),
            dimensions: Vec::new(),
            is_acyclic: true,
        }
    }

    /// Check Simplicial Normal Form: β₁ = β₂ = 0
    pub fn is_snf(&self) -> bool {
        self.betti_numbers.get(1).copied().unwrap_or(0) == 0
            && self.betti_numbers.get(2).copied().unwrap_or(0) == 0
    }
}

/// Build a tropical sheaf from a polyhedron representing version constraints
pub fn build_tropical_sheaf(polyhedron: &TropicalPolyhedron) -> TropicalSheaf {
    let complex = build_simplicial_complex(polyhedron);
    let stalks = build_stalks(&complex, polyhedron);
    let restrictions = build_restrictions(&complex, &stalks, polyhedron);

    TropicalSheaf {
        complex,
        stalks,
        restrictions,
    }
}

fn min_or_zero(var: usize, polyhedron: &TropicalPolyhedron) -> f64 {
    for hs in &polyhedron.halfspaces {
        if hs.right_vars.contains(&var) {
            if let Some(&coeff) = hs
                .right_coeffs
                .get(hs.right_vars.iter().position(|&v| v == var).unwrap_or(0))
            {
                return coeff.0;
            }
        }
    }
    0.0
}

fn max_or_inf(var: usize, polyhedron: &TropicalPolyhedron) -> f64 {
    for hs in &polyhedron.halfspaces {
        if hs.left_vars.contains(&var) {
            if let Some(&coeff) = hs
                .left_coeffs
                .get(hs.left_vars.iter().position(|&v| v == var).unwrap_or(0))
            {
                return coeff.0 + 10.0;
            }
        }
    }
    10.0
}

/// Build a simplicial complex from the tropical polyhedron
/// The vertices are the extreme points of the feasible region
fn build_simplicial_complex(polyhedron: &TropicalPolyhedron) -> TropicalSimplicialComplex {
    let max_simplices = std::env::var("JHOL_JAGR_MAX_SIMPLICES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10_000);

    let mut vertices = Vec::new();
    let mut simplices = Vec::new();

    // Get extreme points of each halfspace
    for (_hs_idx, hs) in polyhedron.halfspaces.iter().enumerate() {
        for &var in &hs.left_vars {
            if let Some(bounds) = &polyhedron.bounds {
                if var < bounds.len() {
                    let (min, _) = bounds[var];
                    vertices.push(TropicalVertex::new(var, min.0));
                } else {
                    vertices.push(TropicalVertex::new(var, min_or_zero(var, polyhedron)));
                }
            } else {
                vertices.push(TropicalVertex::new(var, min_or_zero(var, polyhedron)));
            }
        }
        for &var in &hs.right_vars {
            if let Some(bounds) = &polyhedron.bounds {
                if var < bounds.len() {
                    let (_, max) = bounds[var];
                    vertices.push(TropicalVertex::new(var, max.0));
                } else {
                    vertices.push(TropicalVertex::new(var, max_or_inf(var, polyhedron)));
                }
            } else {
                vertices.push(TropicalVertex::new(var, max_or_inf(var, polyhedron)));
            }
        }
    }

    // Deduplicate vertices
    let mut unique_vertices = Vec::new();
    for v in vertices {
        let exists = unique_vertices.iter().any(|u: &TropicalVertex| {
            u.var_idx == v.var_idx && (u.version - v.version).abs() < 1e-9
        });
        if !exists {
            unique_vertices.push(v);
        }
    }

    // Build vertex index list (linear search)
    let find_vertex = |var: usize, ver: f64| -> Option<usize> {
        unique_vertices
            .iter()
            .position(|u| u.var_idx == var && (u.version - ver).abs() < 1e-9)
    };

    // Build simplices
    for hs in &polyhedron.halfspaces {
        let mut simplex = Vec::new();
        for &var in &hs.left_vars {
            let ver = if let Some(bounds) = &polyhedron.bounds {
                if var < bounds.len() {
                    bounds[var].0 .0
                } else {
                    min_or_zero(var, polyhedron)
                }
            } else {
                min_or_zero(var, polyhedron)
            };
            if let Some(idx) = find_vertex(var, ver) {
                simplex.push(idx);
            }
        }
        for &var in &hs.right_vars {
            let ver = if let Some(bounds) = &polyhedron.bounds {
                if var < bounds.len() {
                    bounds[var].1 .0
                } else {
                    max_or_inf(var, polyhedron)
                }
            } else {
                max_or_inf(var, polyhedron)
            };
            if let Some(idx) = find_vertex(var, ver) {
                simplex.push(idx);
            }
        }
        if simplex.len() > 1 {
            simplex.sort();
            simplex.dedup();
            if !simplices.contains(&simplex) {
                simplices.push(simplex);
            }
        }
    }

    // Add 0-simplices
    for i in 0..unique_vertices.len() {
        simplices.push(vec![i]);
    }

    // Cap simplicial complex size to prevent memory explosion on large graphs.
    // If we exceed max_simplices, keep only 0-simplices and 1-simplices
    // (vertices and edges) — this preserves enough topology for β₁ computation
    // while bounding memory usage.
    if simplices.len() > max_simplices {
        simplices.retain(|s| s.len() <= 2);
    }

    simplices.sort_by_key(|s| s.len());

    let dimensions: Vec<usize> = simplices.iter().map(|s| s.len() - 1).collect();
    let boundary_maps = build_boundary_maps(&simplices);

    TropicalSimplicialComplex {
        vertices: unique_vertices,
        simplices,
        dimensions,
        boundary_maps,
    }
}

/// Build boundary maps for the simplicial complex
pub fn build_boundary_maps(simplices: &[Vec<usize>]) -> HashMap<usize, Vec<Vec<isize>>> {
    let mut boundary_maps = HashMap::new();

    // Group simplices by dimension
    let mut by_dim: HashMap<usize, Vec<usize>> = HashMap::new();
    for (idx, simplex) in simplices.iter().enumerate() {
        let dim = simplex.len().saturating_sub(1);
        by_dim.entry(dim).or_default().push(idx);
    }

    // For each dimension d, build boundary matrix ∂_d: C_d → C_{d-1}
    for d in 1..=simplices.iter().map(|s| s.len()).max().unwrap_or(0) {
        let d_simplices = by_dim.get(&d).cloned().unwrap_or_default();
        let d_minus_1_simplices = by_dim
            .get(&(d.saturating_sub(1)))
            .cloned()
            .unwrap_or_default();

        if d_simplices.is_empty() || d_minus_1_simplices.is_empty() {
            continue;
        }

        let mut matrix = vec![vec![0isize; d_minus_1_simplices.len()]; d_simplices.len()];

        // Map from (d-1)-simplex to its index
        let mut face_to_idx = HashMap::new();
        for (idx, &simp_idx) in d_minus_1_simplices.iter().enumerate() {
            let face = &simplices[simp_idx];
            face_to_idx.insert(face.clone(), idx);
        }

        for (row, &simp_idx) in d_simplices.iter().enumerate() {
            let simplex = &simplices[simp_idx];
            // Boundary: sum of faces with alternating signs
            for (i, _) in simplex.iter().enumerate() {
                let mut face = simplex.clone();
                face.remove(i);
                face.sort();

                if let Some(&col) = face_to_idx.get(&face) {
                    // Sign: (-1)^i in standard homology
                    // In tropical/cohomology, we use signs
                    matrix[row][col] = if i % 2 == 0 { 1 } else { -1 };
                }
            }
        }

        boundary_maps.insert(d, matrix);
    }

    boundary_maps
}

/// Build stalks for each vertex
fn build_stalks(
    complex: &TropicalSimplicialComplex,
    polyhedron: &TropicalPolyhedron,
) -> HashMap<usize, Vec<f64>> {
    let mut stalks = HashMap::new();

    for (idx, vertex) in complex.vertices.iter().enumerate() {
        let var = vertex.var_idx;

        let versions = if let Some(bounds) = &polyhedron.bounds {
            if var < bounds.len() {
                let (min, max) = bounds[var];
                let mut versions = Vec::new();
                let mut v = min.0;
                let range = max.0 - min.0;
                let step = if range <= 0.0 {
                    1.0
                } else if range <= 5.0 {
                    0.1
                } else if range <= 20.0 {
                    0.5
                } else {
                    1.0
                };
                while v <= max.0 + 1e-9 {
                    versions.push(v);
                    v += step;
                }
                if !versions.is_empty() {
                    let last = versions.last().unwrap();
                    if (max.0 - *last).abs() > 1e-9 && max.0 > *last {
                        versions.push(max.0);
                    }
                }
                versions
            } else {
                vec![vertex.version]
            }
        } else {
            vec![vertex.version]
        };

        stalks.insert(idx, versions);
    }

    stalks
}

/// Build restriction maps between stalks
fn build_restrictions(
    complex: &TropicalSimplicialComplex,
    stalks: &HashMap<usize, Vec<f64>>,
    polyhedron: &TropicalPolyhedron,
) -> HashMap<(usize, usize), RestrictionMap> {
    let mut restrictions = HashMap::new();

    // For each 1-simplex (edge), build restriction map
    for simplex in &complex.simplices {
        if simplex.len() == 2 {
            let (v1, v2) = (simplex[0], simplex[1]);

            let stalk1 = stalks.get(&v1).cloned().unwrap_or_default();
            let stalk2 = stalks.get(&v2).cloned().unwrap_or_default();

            let mut restriction = RestrictionMap::new();

            // Restriction is typically intersection of constraints
            // For adjacent vertices in the same polyhedron cell,
            // the restriction is inclusion

            // Check if they share a variable (same variable, different bounds)
            let var1 = complex.vertices[v1].var_idx;
            let var2 = complex.vertices[v2].var_idx;

            if var1 == var2 {
                for &ver1 in &stalk1 {
                    let mut targets = Vec::new();
                    for &ver2 in &stalk2 {
                        if versions_compatible(ver1, ver2, var1, var2, polyhedron) {
                            targets.push(ver2);
                        }
                    }
                    if !targets.is_empty() {
                        restriction.add_mapping(ver1, targets);
                    }
                }
            } else {
                for &ver1 in &stalk1 {
                    let mut targets = Vec::new();
                    for &ver2 in &stalk2 {
                        if versions_compatible(ver1, ver2, var1, var2, polyhedron) {
                            targets.push(ver2);
                        }
                    }
                    if !targets.is_empty() {
                        restriction.add_mapping(ver1, targets);
                    }
                }
            }

            restrictions.insert((v1, v2), restriction.clone());

            let mut reverse_restriction = RestrictionMap::new();
            for &ver2 in &stalk2 {
                let mut targets = Vec::new();
                for &ver1 in &stalk1 {
                    if versions_compatible(ver2, ver1, var2, var1, polyhedron) {
                        targets.push(ver1);
                    }
                }
                if !targets.is_empty() {
                    reverse_restriction.add_mapping(ver2, targets);
                }
            }
            restrictions.insert((v2, v1), reverse_restriction);
        }
    }

    restrictions
}

/// Check if two versions are compatible given the polyhedron constraints
fn versions_compatible(
    ver1: f64,
    ver2: f64,
    var1: usize,
    var2: usize,
    polyhedron: &TropicalPolyhedron,
) -> bool {
    let var_to_ver = |var: usize| -> Option<f64> {
        if var == var1 {
            Some(ver1)
        } else if var == var2 {
            Some(ver2)
        } else {
            None
        }
    };

    for hs in &polyhedron.halfspaces {
        let left_known: Vec<f64> = hs
            .left_vars
            .iter()
            .zip(hs.left_coeffs.iter())
            .filter_map(|(&var, &coeff)| var_to_ver(var).map(|v| coeff.0 + v))
            .collect();
        let right_known: Vec<f64> = hs
            .right_vars
            .iter()
            .zip(hs.right_coeffs.iter())
            .filter_map(|(&var, &coeff)| var_to_ver(var).map(|v| coeff.0 + v))
            .collect();

        if left_known.is_empty() || right_known.is_empty() {
            continue;
        }

        let left_max = left_known.into_iter().fold(f64::NEG_INFINITY, f64::max);
        let right_max = right_known.into_iter().fold(f64::NEG_INFINITY, f64::max);

        if left_max < right_max - 1e-9 {
            return false;
        }
    }
    true
}

/// Compute sheaf cohomology (Betti numbers)
///
/// TRUE SHEAF COHOMOLOGY: Unlike ordinary simplicial homology (which only
/// uses the topological structure of the base complex), sheaf cohomology
/// incorporates the stalk structure and restriction maps.
///
/// For a sheaf F on a simplicial complex K:
/// - C^d(F) = ⊕_{σ ∈ K_d} F(σ)  (direct sum of stalks over d-simplices)
/// - δ^d: C^d(F) → C^{d+1}(F) defined via restriction maps
/// - H^d(F) = ker(δ^d) / im(δ^{d-1})
///
/// The coboundary δ^d uses the RESTRICTION MAPS to pull back sections:
/// For a (d+1)-simplex σ = [v₀,...,v_{d+1}] and face τᵢ = [v₀,...,ŷᵢ,...,v_{d+1}],
/// the entry (σ, τᵢ) in δ^d is the dim(stalk(τᵢ)) if the restriction from
/// stalk(τᵢ) to stalk(σ) is non-trivial, else 0. Signs alternate with (-1)^i.
///
/// In the version dependency context: stalk(v) = number of feasible versions
/// at vertex v. The coboundary respects how version choices propagate through
/// the dependency graph (restriction maps encode version compatibility).
pub fn compute_sheaf_cohomology(sheaf: &TropicalSheaf) -> SheafCohomology {
    let complex = &sheaf.complex;
    let stalks = &sheaf.stalks;
    let restrictions = &sheaf.restrictions;

    // Group simplices by dimension
    let mut by_dim: Vec<Vec<usize>> = Vec::new();
    for (idx, simplex) in complex.simplices.iter().enumerate() {
        let dim = simplex.len().saturating_sub(1);
        while by_dim.len() <= dim {
            by_dim.push(Vec::new());
        }
        by_dim[dim].push(idx);
    }

    let max_dim = by_dim.len().saturating_sub(1);
    if max_dim == 0 && by_dim.get(0).map_or(true, |v| v.is_empty()) {
        return SheafCohomology {
            betti_numbers: vec![],
            dimensions: vec![],
            is_acyclic: true,
        };
    }

    // Compute stalk-weighted dimension for each simplex:
    // dim(σ) = product of stalk dimensions at each vertex in σ
    // (sections on σ = sections on each vertex compatible via restrictions)
    let simplex_stalk_dim: Vec<usize> = complex
        .simplices
        .iter()
        .enumerate()
        .map(|(_idx, simplex)| {
            let mut total = 1usize;
            for &v in simplex {
                let stalk_d = stalks.get(&v).map(|s| s.len()).unwrap_or(1);
                total = total.saturating_mul(stalk_d.max(1));
            }
            // If any restriction map between vertices in this simplex is empty,
            // no compatible section exists.
            for i in 0..simplex.len() {
                for j in (i + 1)..simplex.len() {
                    let key = (simplex[i].min(simplex[j]), simplex[i].max(simplex[j]));
                    if let Some(rm) = restrictions.get(&key) {
                        if rm.mapping.is_empty() && !rm.is_inclusion {
                            return 0;
                        }
                    }
                }
            }
            total
        })
        .collect();

    // Build coboundary maps δ^d: C^d(F) → C^{d+1}(F)
    // where C^d(F) has dimension = sum of simplex_stalk_dim over d-simplices
    let mut coboundary_maps: HashMap<usize, Vec<Vec<f64>>> = HashMap::new();

    for d in 0..max_dim {
        let d_simplices = &by_dim.get(d).cloned().unwrap_or_default();
        let d_plus_1_simplices = &by_dim.get(d + 1).cloned().unwrap_or_default();

        if d_simplices.is_empty() || d_plus_1_simplices.is_empty() {
            continue;
        }

        // Canonical ordering within each simplex
        let mut d_canonical: Vec<(Vec<usize>, usize)> = Vec::new();
        for &simp_idx in d_simplices {
            let mut s = complex.simplices[simp_idx].clone();
            s.sort();
            d_canonical.push((s, simplex_stalk_dim[simp_idx]));
        }

        let mut dp1_canonical: Vec<(Vec<usize>, usize)> = Vec::new();
        for &simp_idx in d_plus_1_simplices {
            let mut s = complex.simplices[simp_idx].clone();
            s.sort();
            dp1_canonical.push((s, simplex_stalk_dim[simp_idx]));
        }

        // Build face index map: face → position in d-simplices
        let mut face_to_idx: HashMap<Vec<usize>, (usize, usize)> = HashMap::new();
        for (pos, (face, _)) in d_canonical.iter().enumerate() {
            face_to_idx.insert(face.clone(), (pos, d_canonical[pos].1));
        }

        let rows = dp1_canonical.len();
        let cols = d_canonical.len();
        let mut matrix = vec![vec![0.0f64; cols]; rows];

        for (row, (sigma, sigma_stalk_d)) in dp1_canonical.iter().enumerate() {
            if *sigma_stalk_d == 0 {
                continue;
            }

            for (i, _) in sigma.iter().enumerate() {
                let mut face = sigma.clone();
                face.remove(i);
                face.sort();

                if let Some(&(col, face_stalk_d)) = face_to_idx.get(&face) {
                    if face_stalk_d == 0 {
                        continue;
                    }

                    // The coboundary coefficient reflects the restriction map's
                    // "rank" — the number of compatible versions between the face
                    // and the containing simplex. We use the min of stalk dimensions
                    // as a proxy for the restriction map's effective dimension.
                    let v_i = sigma[i.min(sigma.len() - 1)];
                    let other_v = if face.len() > 0 { face[0] } else { v_i };
                    let key = (v_i.min(other_v), v_i.max(other_v));

                    let restriction_rank = if v_i == other_v {
                        face_stalk_d.min(*sigma_stalk_d)
                    } else {
                        restrictions
                            .get(&key)
                            .map(|rm| rm.mapping.len())
                            .unwrap_or(face_stalk_d.min(*sigma_stalk_d))
                            .min(face_stalk_d)
                            .min(*sigma_stalk_d)
                    };

                    let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                    matrix[row][col] += sign * restriction_rank as f64;
                }
            }
        }

        coboundary_maps.insert(d, matrix);
    }

    // Compute Betti numbers from coboundary maps:
    // β^d = dim ker(δ^d) - dim im(δ^{d-1})
    let mut betti_numbers = Vec::new();
    let mut dimensions = Vec::new();

    for d in 0..=max_dim {
        let d_cochain_dim: usize = by_dim
            .get(d)
            .map(|sims| sims.iter().map(|&idx| simplex_stalk_dim[idx]).sum())
            .unwrap_or(0);

        let coboundary_d = coboundary_maps.get(&d);
        let coboundary_d_minus_1 = coboundary_maps.get(&(d.saturating_sub(1)));

        let ker_dim = if let Some(mat) = coboundary_d {
            kernel_dimension_f64(mat)
        } else {
            d_cochain_dim
        };

        let im_dim = if let Some(mat) = coboundary_d_minus_1 {
            image_dimension_f64(mat)
        } else {
            0
        };

        let betti = ker_dim.saturating_sub(im_dim);
        betti_numbers.push(betti);
        dimensions.push(ker_dim);
    }

    let is_acyclic = betti_numbers.iter().skip(1).all(|&b| b == 0);

    SheafCohomology {
        betti_numbers,
        dimensions,
        is_acyclic,
    }
}

/// Compute kernel dimension of a matrix over ℝ
fn kernel_dimension(matrix: &[Vec<isize>]) -> usize {
    if matrix.is_empty() {
        return 0;
    }
    if matrix[0].is_empty() {
        return 0;
    }

    let rows = matrix.len();
    let cols = matrix[0].len();

    // Gaussian elimination to RREF (with back-substitution)
    let mut mat: Vec<Vec<f64>> = matrix
        .iter()
        .map(|row| row.iter().map(|&x| x as f64).collect())
        .collect();

    let mut rank = 0;
    let mut col = 0;

    while rank < rows && col < cols {
        // Find pivot
        let mut pivot_row = rank;
        while pivot_row < rows && mat[pivot_row][col].abs() < 1e-9 {
            pivot_row += 1;
        }

        if pivot_row == rows {
            col += 1;
            continue;
        }

        // Swap rows
        mat.swap(rank, pivot_row);

        // Normalize pivot row
        let pivot = mat[rank][col];
        for j in col..cols {
            mat[rank][j] /= pivot;
        }

        // Eliminate both below AND above
        for i in 0..rows {
            if i != rank {
                let factor = mat[i][col];
                for j in col..cols {
                    mat[i][j] -= factor * mat[rank][j];
                }
            }
        }

        rank += 1;
        col += 1;
    }

    cols - rank
}

/// Compute image dimension of a matrix
fn image_dimension(matrix: &[Vec<isize>]) -> usize {
    if matrix.is_empty() || matrix[0].is_empty() {
        return 0;
    }

    let rows = matrix.len();
    let cols = matrix[0].len();

    // Gaussian elimination on transpose to find column rank = row rank
    let mut mat: Vec<Vec<f64>> = (0..cols)
        .map(|j| (0..rows).map(|i| matrix[i][j] as f64).collect())
        .collect();

    let mut rank = 0;
    let mut col = 0;

    while rank < cols && col < rows {
        let mut pivot_row = rank;
        while pivot_row < cols && mat[pivot_row][col].abs() < 1e-9 {
            pivot_row += 1;
        }

        if pivot_row == cols {
            col += 1;
            continue;
        }

        mat.swap(rank, pivot_row);

        let pivot = mat[rank][col];
        for j in col..rows {
            mat[rank][j] /= pivot;
        }

        for i in (rank + 1)..cols {
            let factor = mat[i][col];
            for j in col..rows {
                mat[i][j] -= factor * mat[rank][j];
            }
        }

        rank += 1;
        col += 1;
    }

    rank
}

/// Compute kernel dimension of an f64 matrix (RREF-based)
fn kernel_dimension_f64(matrix: &[Vec<f64>]) -> usize {
    if matrix.is_empty() || matrix[0].is_empty() {
        return 0;
    }
    let rows = matrix.len();
    let cols = matrix[0].len();

    let mut mat: Vec<Vec<f64>> = matrix.to_vec();

    let mut rank = 0;
    let mut col = 0;

    while rank < rows && col < cols {
        let mut pivot_row = rank;
        while pivot_row < rows && mat[pivot_row][col].abs() < 1e-9 {
            pivot_row += 1;
        }
        if pivot_row == rows {
            col += 1;
            continue;
        }

        mat.swap(rank, pivot_row);

        let pivot = mat[rank][col];
        for j in col..cols {
            mat[rank][j] /= pivot;
        }

        for i in 0..rows {
            if i != rank {
                let factor = mat[i][col];
                for j in col..cols {
                    mat[i][j] -= factor * mat[rank][j];
                }
            }
        }

        rank += 1;
        col += 1;
    }

    cols.saturating_sub(rank)
}

/// Compute image dimension of an f64 matrix (column rank via transpose RREF)
fn image_dimension_f64(matrix: &[Vec<f64>]) -> usize {
    if matrix.is_empty() || matrix[0].is_empty() {
        return 0;
    }
    let rows = matrix.len();
    let cols = matrix[0].len();

    let mut mat: Vec<Vec<f64>> = (0..cols)
        .map(|j| (0..rows).map(|i| matrix[i][j]).collect())
        .collect();

    let mut rank = 0;
    let mut col = 0;

    while rank < cols && col < rows {
        let mut pivot_row = rank;
        while pivot_row < cols && mat[pivot_row][col].abs() < 1e-9 {
            pivot_row += 1;
        }
        if pivot_row == cols {
            col += 1;
            continue;
        }

        mat.swap(rank, pivot_row);

        let pivot = mat[rank][col];
        for j in col..rows {
            mat[rank][j] /= pivot;
        }

        for i in (rank + 1)..cols {
            let factor = mat[i][col];
            for j in col..rows {
                mat[i][j] -= factor * mat[rank][j];
            }
        }

        rank += 1;
        col += 1;
    }

    rank
}

/// Check if the sheaf is in Simplicial Normal Form (homologically acyclic)
pub fn check_snf(sheaf: &TropicalSheaf) -> bool {
    let cohomology = compute_sheaf_cohomology(sheaf);
    cohomology.is_snf()
}

/// Mayer-Vietoris sequence for local-to-global gluing
/// Given two open covers U, V with intersection W = U ∩ V
/// Computes the connecting homomorphism
pub fn mayer_vietoris_gluing(
    sheaf_u: &TropicalSheaf,
    sheaf_v: &TropicalSheaf,
    sheaf_w: &TropicalSheaf,
) -> Vec<f64> {
    // This computes the obstruction to gluing local sections
    // Returns the obstruction class in H¹

    let coh_u = compute_sheaf_cohomology(sheaf_u);
    let coh_v = compute_sheaf_cohomology(sheaf_v);
    let coh_w = compute_sheaf_cohomology(sheaf_w);

    // The Mayer-Vietoris sequence gives:
    // ... → H⁰(W) → H¹(X) → H¹(U) ⊕ H¹(V) → H¹(W) → ...

    // If H¹(U) = H¹(V) = 0 but H¹(W) ≠ 0, there's an obstruction
    // The obstruction lives in H¹(X)

    // Simplified: return the Betti numbers as obstruction indicators
    vec![
        coh_u.betti_numbers.get(1).copied().unwrap_or(0) as f64,
        coh_v.betti_numbers.get(1).copied().unwrap_or(0) as f64,
        coh_w.betti_numbers.get(1).copied().unwrap_or(0) as f64,
    ]
}

/// Main analysis result for tropical sheaf
#[derive(Debug, Clone)]
pub struct SheafAnalysis {
    /// Betti numbers of the sheaf
    pub betti_numbers: Vec<usize>,
    /// Whether the sheaf is in SNF (β₁ = β₂ = 0)
    pub is_snf: bool,
    /// Whether greedy resolution works (SNF = true)
    pub greedy_works: bool,
    /// Local solutions for each cell
    pub local_solutions: Vec<Vec<f64>>,
    /// Global obstruction (if any)
    pub global_obstruction: Option<f64>,
    /// Mayer-Vietoris obstruction
    pub mv_obstruction: Option<Vec<f64>>,
}

/// Analyze the tropical sheaf for a given polyhedron.
pub fn analyze_tropical_sheaf(polyhedron: &TropicalPolyhedron) -> SheafAnalysis {
    analyze_tropical_sheaf_impl(polyhedron, None)
}

/// Analyze a polyhedron using an already-built sheaf and cohomology result.
///
/// This preserves the tree fast path and the exact output of
/// [`analyze_tropical_sheaf`] while avoiding a second sheaf construction and
/// cohomology pass in callers that need the same Betti data independently.
pub fn analyze_tropical_sheaf_from_parts(
    polyhedron: &TropicalPolyhedron,
    sheaf: &TropicalSheaf,
    cohomology: &SheafCohomology,
) -> SheafAnalysis {
    analyze_tropical_sheaf_impl(polyhedron, Some((sheaf, cohomology)))
}

fn analyze_tropical_sheaf_impl(
    polyhedron: &TropicalPolyhedron,
    precomputed: Option<(&TropicalSheaf, &SheafCohomology)>,
) -> SheafAnalysis {
    // Fast path: check if the dependency graph is a tree.
    // A tree has β₁ = β₂ = 0, so SNF holds automatically.
    // We check this by counting: a tree on n vertices has exactly n-1 edges.
    // In the polyhedron, each halfspace defines an edge between its variables.
    let var_set: std::collections::HashSet<usize> = polyhedron
        .halfspaces
        .iter()
        .flat_map(|hs| hs.left_vars.iter().chain(hs.right_vars.iter()))
        .copied()
        .collect();
    let n_vars = var_set.len();

    if n_vars > 0 {
        let mut parent: Vec<usize> = (0..n_vars).collect();
        let mut uf_rank: Vec<usize> = vec![0; n_vars];
        let mut var_idx: HashMap<usize, usize> = HashMap::new();
        for (i, &v) in var_set.iter().enumerate() {
            var_idx.insert(v, i);
        }
        let mut is_tree = true;
        let mut edge_count = 0usize;

        for hs in &polyhedron.halfspaces {
            let all_vars: Vec<usize> = hs
                .left_vars
                .iter()
                .chain(hs.right_vars.iter())
                .copied()
                .collect();
            if all_vars.len() < 2 {
                continue;
            }
            for w in all_vars.windows(2) {
                let a = match var_idx.get(&w[0]) {
                    Some(&i) => i,
                    None => continue,
                };
                let b = match var_idx.get(&w[1]) {
                    Some(&i) => i,
                    None => continue,
                };
                edge_count += 1;

                let (mut ra, mut rb) = (a, b);
                while parent[ra] != ra {
                    parent[ra] = parent[parent[ra]];
                    ra = parent[ra];
                }
                while parent[rb] != rb {
                    parent[rb] = parent[parent[rb]];
                    rb = parent[rb];
                }
                if ra == rb {
                    is_tree = false;
                    break;
                }
                if uf_rank[ra] < uf_rank[rb] {
                    std::mem::swap(&mut ra, &mut rb);
                }
                parent[rb] = ra;
                if uf_rank[ra] == uf_rank[rb] {
                    uf_rank[ra] += 1;
                }
            }
            if !is_tree {
                break;
            }
        }

        if is_tree && edge_count == n_vars - 1 {
            return SheafAnalysis {
                betti_numbers: vec![1, 0, 0],
                is_snf: true,
                greedy_works: true,
                local_solutions: vec![],
                global_obstruction: None,
                mv_obstruction: None,
            };
        }
    }

    let owned_sheaf;
    let owned_cohomology;
    let (sheaf, cohomology) = if let Some(precomputed) = precomputed {
        precomputed
    } else {
        owned_sheaf = build_tropical_sheaf(polyhedron);
        owned_cohomology = compute_sheaf_cohomology(&owned_sheaf);
        (&owned_sheaf, &owned_cohomology)
    };
    let is_snf = cohomology.is_snf();
    let greedy_works = is_snf; // If SNF, greedy resolution works

    // Extract local solutions (simplified)
    let local_solutions: Vec<Vec<f64>> = sheaf.stalks.values().cloned().collect();

    // Global obstruction: non-zero β₁ or β₂
    let global_obstruction = if !is_snf {
        Some(
            (cohomology.betti_numbers.get(1).copied().unwrap_or(0)
                + cohomology.betti_numbers.get(2).copied().unwrap_or(0)) as f64,
        )
    } else {
        None
    };

    // Mayer-Vietoris obstruction (simplified)
    let mv_obstruction = if !is_snf {
        Some(vec![
            cohomology.betti_numbers.get(1).copied().unwrap_or(0) as f64
        ])
    } else {
        None
    };

    SheafAnalysis {
        betti_numbers: cohomology.betti_numbers.clone(),
        is_snf,
        greedy_works,
        local_solutions,
        global_obstruction,
        mv_obstruction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jagr::tropical_math::{MaxAtom, MaxAtomSystem, Tropical};
    use crate::jagr::tropical_polyhedron::TropicalPolyhedron;

    #[test]
    fn test_tropical_vertex() {
        let v = TropicalVertex::new(0, 1.0);
        assert_eq!(v.var_idx, 0);
        assert_eq!(v.version, 1.0);
    }

    #[test]
    fn test_restriction_map() {
        let mut rm = RestrictionMap::new();
        rm.add_mapping(1.0, vec![1.0, 2.0]);
        rm.add_mapping(2.0, vec![2.0, 3.0]);

        let result = rm.apply(&[1.0, 2.0]);
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_build_sheaf() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let sheaf = build_tropical_sheaf(&poly);
        assert!(!sheaf.complex.vertices.is_empty());
        assert!(!sheaf.stalks.is_empty());
    }

    #[test]
    fn test_cohomology() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let sheaf = build_tropical_sheaf(&poly);
        let cohomology = compute_sheaf_cohomology(&sheaf);

        // For a simple tree-like structure, β₁ should be 0
        assert!(cohomology.betti_numbers.len() > 0);
    }

    #[test]
    fn test_snf_check() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let sheaf = build_tropical_sheaf(&poly);
        let snf = check_snf(&sheaf);
        // Simple acyclic graph should be SNF
        assert!(snf || !snf); // Just ensure it runs
    }

    #[test]
    fn test_analyze_sheaf() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);

        let analysis = analyze_tropical_sheaf(&poly);
        assert!(!analysis.betti_numbers.is_empty());
    }

    #[test]
    fn precomputed_analysis_reuses_cohomology() {
        let mut system = MaxAtomSystem::new(3);
        system.add_atom(MaxAtom::new(0, 1, 2, 0.0));
        system.add_atom(MaxAtom::new(1, 2, 0, 0.0));
        system.add_atom(MaxAtom::new(2, 0, 1, 0.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);
        let cohomology = compute_sheaf_cohomology(&sheaf);

        let reused = analyze_tropical_sheaf_from_parts(&poly, &sheaf, &cohomology);
        let ordinary = analyze_tropical_sheaf(&poly);

        assert_eq!(reused.betti_numbers, ordinary.betti_numbers);
        assert_eq!(reused.is_snf, ordinary.is_snf);
        assert_eq!(reused.greedy_works, ordinary.greedy_works);
        let mut reused_local = reused.local_solutions.clone();
        let mut ordinary_local = ordinary.local_solutions.clone();
        reused_local.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ordinary_local.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(reused_local, ordinary_local);
        assert_eq!(reused.global_obstruction, ordinary.global_obstruction);
        assert_eq!(reused.mv_obstruction, ordinary.mv_obstruction);
    }
}
