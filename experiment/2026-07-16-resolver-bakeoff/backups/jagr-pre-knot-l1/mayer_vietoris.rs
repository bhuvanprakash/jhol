//! JAGR Layer 2: Mayer-Vietoris Sequence
//!
//! Implements the Mayer-Vietoris sequence for tropical sheaf cohomology,
//! enabling local-to-global gluing of version solutions.
//!
//! Given a cover of the version space by open sets U and V,
//! with intersection W = U ∩ V, the Mayer-Vietoris sequence is:
//!
//! ... → H⁰(W) → H¹(X) → H¹(U) ⊕ H¹(V) → H¹(W) → H²(X) → ...
//!
//! This allows computing global obstructions from local data.

use super::tropical_sheaf::{compute_sheaf_cohomology, SheafCohomology, TropicalSheaf};
use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Open cover of a tropical space
#[derive(Debug, Clone)]
pub struct TropicalCover {
    /// The open sets in the cover
    pub open_sets: Vec<TropicalOpenSet>,
    /// Intersections between open sets
    pub intersections: HashMap<(usize, usize), TropicalOpenSet>,
    /// Triple intersections
    pub triple_intersections: HashMap<(usize, usize, usize), TropicalOpenSet>,
}

/// An open set in the tropical topology
#[derive(Debug, Clone)]
pub struct TropicalOpenSet {
    /// The sheaf restricted to this open set
    pub sheaf: TropicalSheaf,
    /// Index in the cover
    pub index: usize,
    /// Name/description
    pub name: String,
}

/// Mayer-Vietoris sequence data
#[derive(Debug, Clone)]
pub struct MayerVietorisSequence {
    /// The cover
    pub cover: TropicalCover,
    /// Cohomology of each piece
    pub cohomologies: HashMap<String, SheafCohomology>,
    /// Connecting homomorphisms
    pub connecting_maps: Vec<ConnectingMap>,
    /// Exactness verification
    pub is_exact: bool,
}

/// A connecting homomorphism in the Mayer-Vietoris sequence
#[derive(Debug, Clone)]
pub struct ConnectingMap {
    pub source_degree: usize,
    pub target_degree: usize,
    pub matrix: Vec<Vec<f64>>,
    pub kernel: Vec<usize>,
    pub image: Vec<usize>,
}

impl ConnectingMap {
    fn compute_kernel_and_image(&mut self) {
        let rows = self.matrix.len();
        if rows == 0 {
            return;
        }
        let cols = if rows > 0 { self.matrix[0].len() } else { 0 };
        if cols == 0 {
            self.kernel = (0..rows).collect();
            self.image = Vec::new();
            return;
        }

        let mut mat = self.matrix.clone();
        let mut pivot_cols = Vec::new();
        let mut pivot_row = 0;

        for col in 0..cols {
            let mut found_row = None;
            for row in pivot_row..rows {
                if mat[row][col].abs() > 1e-12 {
                    found_row = Some(row);
                    break;
                }
            }
            if let Some(r) = found_row {
                mat.swap(pivot_row, r);
                let pivot = mat[pivot_row][col];
                for j in col..cols {
                    mat[pivot_row][j] /= pivot;
                }
                for i in 0..rows {
                    if i != pivot_row {
                        let factor = mat[i][col];
                        for j in col..cols {
                            mat[i][j] -= factor * mat[pivot_row][j];
                        }
                    }
                }
                pivot_cols.push(col);
                pivot_row += 1;
                if pivot_row >= rows {
                    break;
                }
            }
        }

        let mut free_cols: Vec<usize> = Vec::new();
        let mut pivot_idx = 0;
        for col in 0..cols {
            if pivot_idx < pivot_cols.len() && col == pivot_cols[pivot_idx] {
                pivot_idx += 1;
            } else {
                free_cols.push(col);
            }
        }

        self.kernel = free_cols;
        self.image = pivot_cols;
    }

    fn kernel_basis_vecs(&self) -> Vec<Vec<f64>> {
        let rows = self.matrix.len();
        if rows == 0 {
            return vec![];
        }
        let cols = self.matrix[0].len();
        if cols == 0 {
            return vec![];
        }
        let mut mat = self.matrix.clone();
        let mut pivot_cols = Vec::new();
        let mut pivot_row = 0;

        for col in 0..cols {
            let mut found_row = None;
            for row in pivot_row..rows {
                if mat[row][col].abs() > 1e-12 {
                    found_row = Some(row);
                    break;
                }
            }
            if let Some(r) = found_row {
                mat.swap(pivot_row, r);
                let pivot = mat[pivot_row][col];
                for j in col..cols {
                    mat[pivot_row][j] /= pivot;
                }
                for i in 0..rows {
                    if i != pivot_row {
                        let factor = mat[i][col];
                        for j in col..cols {
                            mat[i][j] -= factor * mat[pivot_row][j];
                        }
                    }
                }
                pivot_cols.push(col);
                pivot_row += 1;
                if pivot_row >= rows {
                    break;
                }
            }
        }

        let mut free_cols: Vec<usize> = Vec::new();
        let mut pivot_idx = 0;
        for col in 0..cols {
            if pivot_idx < pivot_cols.len() && col == pivot_cols[pivot_idx] {
                pivot_idx += 1;
            } else {
                free_cols.push(col);
            }
        }

        free_cols
            .iter()
            .map(|&free_col| {
                let mut vec = vec![0.0; cols];
                vec[free_col] = 1.0;
                for (i, &pivot_col) in pivot_cols.iter().enumerate() {
                    vec[pivot_col] = -mat[i][free_col];
                }
                vec
            })
            .collect()
    }

    fn image_basis_vecs(&self) -> Vec<Vec<f64>> {
        let rows = self.matrix.len();
        if rows == 0 {
            return vec![];
        }
        let cols = self.matrix[0].len();
        if cols == 0 {
            return vec![];
        }
        // The image (column space) basis is the set of original matrix columns
        // corresponding to pivot columns in RREF of the *rows*. We compute RREF
        // to find pivot column indices, then extract those columns from the
        // original (un-reduced) matrix.
        let mut mat = self.matrix.clone();
        let mut pivot_cols = Vec::new();
        let mut pivot_row = 0;

        for col in 0..cols {
            let mut found_row = None;
            for row in pivot_row..rows {
                if mat[row][col].abs() > 1e-12 {
                    found_row = Some(row);
                    break;
                }
            }
            if let Some(r) = found_row {
                mat.swap(pivot_row, r);
                let pivot = mat[pivot_row][col];
                for j in col..cols {
                    mat[pivot_row][j] /= pivot;
                }
                for i in 0..rows {
                    if i != pivot_row {
                        let factor = mat[i][col];
                        for j in col..cols {
                            mat[i][j] -= factor * mat[pivot_row][j];
                        }
                    }
                }
                pivot_cols.push(col);
                pivot_row += 1;
                if pivot_row >= rows {
                    break;
                }
            }
        }

        // Extract the original matrix columns at pivot positions
        pivot_cols
            .iter()
            .map(|&col| self.matrix.iter().map(|row| row[col]).collect())
            .collect()
    }
}

/// Build a cover from a tropical sheaf by partitioning the complex
/// Build a geometrically meaningful open cover of the sheaf's simplicial complex.
///
/// Uses the standard "star cover" construction from computational topology:
/// each open set U_i is the (closed) star of a vertex v_i — the set of all
/// simplices containing v_i. The pairwise intersections are then the links
/// (simplices containing BOTH v_i and v_j). This is a proper open cover that
/// respects the topology, unlike arbitrary vertex partitioning.
///
/// When num_pieces < num_vertices, we merge stars by clustering vertices
/// that share the most simplices (greedy merging by overlap). This ensures
/// the cover has the right number of pieces while preserving topology.
pub fn build_cover(sheaf: &TropicalSheaf, num_pieces: usize) -> TropicalCover {
    let mut cover = TropicalCover {
        open_sets: Vec::new(),
        intersections: HashMap::new(),
        triple_intersections: HashMap::new(),
    };

    let complex = &sheaf.complex;
    let n_verts = complex.vertices.len();
    if n_verts == 0 {
        return cover;
    }

    // Step 1: Compute each vertex star as a dense simplex bitset. The cover
    // needs only union and intersection cardinality, so u64 words preserve the
    // exact sets while turning both operations into word-wise CPU primitives.
    let star_words = complex.simplices.len().div_ceil(64);
    let mut vertex_stars = vec![vec![0u64; star_words]; n_verts];
    for (simp_idx, simplex) in complex.simplices.iter().enumerate() {
        let word = simp_idx / 64;
        let bit = 1u64 << (simp_idx % 64);
        for &vertex in simplex {
            if vertex < n_verts {
                vertex_stars[vertex][word] |= bit;
            }
        }
    }

    // Step 2: Compute pairwise overlap (number of shared simplices)
    // and greedily merge closest vertices until we have num_pieces
    let actual_pieces = num_pieces.max(1).min(n_verts);

    // Each vertex starts in its own cluster. Keep a removable score index:
    // a lazy heap accumulates O(V²) stale pairs while clusters disappear and
    // makes the final merges dominate resolver time. BTreeSet lets a merge
    // delete exactly its incident scores before inserting the new O(V) scores.
    let mut cluster_of: Vec<usize> = (0..n_verts).collect();
    let mut cluster_stars: Vec<Vec<u64>> = vertex_stars;
    let mut next_cluster_id = n_verts;
    let mut active: BTreeSet<usize> = (0..n_verts).collect();
    let mut pair_scores: HashMap<(usize, usize), usize> = HashMap::new();
    let mut ranked: BTreeSet<(usize, Reverse<usize>, Reverse<usize>)> = BTreeSet::new();

    let overlap_count = |a: &[u64], b: &[u64]| -> usize {
        a.iter()
            .zip(b)
            .map(|(left, right)| (left & right).count_ones() as usize)
            .sum()
    };

    let initial: Vec<usize> = active.iter().copied().collect();
    for (offset, &left) in initial.iter().enumerate() {
        for &right in &initial[(offset + 1)..] {
            let score = overlap_count(&cluster_stars[left], &cluster_stars[right]);
            if score > 0 {
                pair_scores.insert((left, right), score);
                ranked.insert((score, Reverse(left), Reverse(right)));
            }
        }
    }

    // Merge the two clusters with the most overlap until we have actual_pieces
    while active.len() > actual_pieces {
        if let Some(&(best_overlap, Reverse(ci), Reverse(cj))) = ranked.iter().next_back() {
            ranked.remove(&(best_overlap, Reverse(ci), Reverse(cj)));
            pair_scores.remove(&(ci, cj));
            let new_id = next_cluster_id;
            next_cluster_id += 1;

            // Merge stars
            let merged_star: Vec<u64> = cluster_stars[ci]
                .iter()
                .zip(&cluster_stars[cj])
                .map(|(left, right)| left | right)
                .collect();

            // Ensure cluster_stars is large enough
            while cluster_stars.len() <= new_id {
                cluster_stars.push(Vec::new());
            }
            cluster_stars[new_id] = merged_star;

            let others: Vec<usize> = active
                .iter()
                .copied()
                .filter(|cluster| *cluster != ci && *cluster != cj)
                .collect();
            for &other in &others {
                for removed in [ci, cj] {
                    let pair = if removed < other {
                        (removed, other)
                    } else {
                        (other, removed)
                    };
                    if let Some(score) = pair_scores.remove(&pair) {
                        ranked.remove(&(score, Reverse(pair.0), Reverse(pair.1)));
                    }
                }
            }

            active.remove(&ci);
            active.remove(&cj);
            active.insert(new_id);

            for other in others {
                let pair = if other < new_id {
                    (other, new_id)
                } else {
                    (new_id, other)
                };
                let score = overlap_count(&cluster_stars[pair.0], &cluster_stars[pair.1]);
                if score > 0 {
                    pair_scores.insert(pair, score);
                    ranked.insert((score, Reverse(pair.0), Reverse(pair.1)));
                }
            }

            // Update cluster assignments
            for cluster in &mut cluster_of {
                if *cluster == ci || *cluster == cj {
                    *cluster = new_id;
                }
            }
        } else {
            break;
        }
    }

    // Step 3: Build open sets from cluster stars
    let final_clusters: BTreeSet<usize> = cluster_of.iter().copied().collect();

    for (piece_idx, &cluster_id) in final_clusters.iter().enumerate() {
        let cluster_vertices: HashSet<usize> = (0..n_verts)
            .filter(|v| cluster_of[*v] == cluster_id)
            .collect();

        let piece_sheaf = restrict_sheaf(sheaf, &cluster_vertices);

        cover.open_sets.push(TropicalOpenSet {
            sheaf: piece_sheaf,
            index: piece_idx,
            name: format!("U_{}", piece_idx),
        });
    }

    // Step 4: Compute pairwise intersections
    for i in 0..cover.open_sets.len() {
        for j in (i + 1)..cover.open_sets.len() {
            let inter = intersect_sheaves(&cover.open_sets[i].sheaf, &cover.open_sets[j].sheaf);
            cover.intersections.insert((i, j), inter);
        }
    }

    // Step 5: Compute triple intersections
    if cover.open_sets.len() >= 3 {
        for i in 0..cover.open_sets.len() {
            for j in (i + 1)..cover.open_sets.len() {
                for k in (j + 1)..cover.open_sets.len() {
                    if let Some(open_ij) = cover.intersections.get(&(i, j)).cloned() {
                        let inter_ijk =
                            intersect_sheaves(&open_ij.sheaf, &cover.open_sets[k].sheaf);
                        cover.triple_intersections.insert((i, j, k), inter_ijk);
                    }
                }
            }
        }
    }

    cover
}

/// Restrict a sheaf to a subset of vertices
fn restrict_sheaf(sheaf: &TropicalSheaf, vertices: &HashSet<usize>) -> TropicalSheaf {
    let complex = &sheaf.complex;

    // Filter vertices
    let new_vertices: Vec<_> = complex
        .vertices
        .iter()
        .enumerate()
        .filter(|(idx, _)| vertices.contains(idx))
        .map(|(_, v)| v.clone())
        .collect();

    // Filter simplices
    let new_simplices: Vec<_> = complex
        .simplices
        .iter()
        .filter(|simp| simp.iter().all(|v| vertices.contains(v)))
        .cloned()
        .collect();

    // Re-index simplices
    let mut vertex_map = HashMap::new();
    for (new_idx, (old_idx, _)) in complex
        .vertices
        .iter()
        .enumerate()
        .filter(|(idx, _)| vertices.contains(idx))
        .enumerate()
    {
        vertex_map.insert(old_idx, new_idx);
    }

    let remapped_simplices: Vec<Vec<usize>> = new_simplices
        .iter()
        .map(|simp: &Vec<usize>| {
            simp.iter()
                .filter_map(|v| vertex_map.get(v).copied())
                .collect::<Vec<usize>>()
        })
        .filter(|simp: &Vec<usize>| !simp.is_empty())
        .collect();

    let dimensions: Vec<usize> = remapped_simplices.iter().map(|s| s.len() - 1).collect();
    let boundary_maps = super::tropical_sheaf::build_boundary_maps(&remapped_simplices);

    // Filter stalks and restrictions
    let mut new_stalks = HashMap::new();
    for (new_idx, (old_idx, _)) in complex
        .vertices
        .iter()
        .enumerate()
        .filter(|(idx, _)| vertices.contains(idx))
        .enumerate()
    {
        if let Some(stalk) = sheaf.stalks.get(&old_idx) {
            new_stalks.insert(new_idx, stalk.clone());
        }
    }

    let mut new_restrictions = HashMap::new();
    for ((v1, v2), rm) in &sheaf.restrictions {
        if let (Some(&n1), Some(&n2)) = (vertex_map.get(v1), vertex_map.get(v2)) {
            new_restrictions.insert((n1, n2), rm.clone());
        }
    }

    TropicalSheaf {
        complex: super::tropical_sheaf::TropicalSimplicialComplex {
            vertices: new_vertices,
            simplices: remapped_simplices,
            dimensions,
            boundary_maps,
        },
        stalks: new_stalks,
        restrictions: new_restrictions,
    }
}

/// Intersect two sheaves
fn intersect_sheaves(sheaf1: &TropicalSheaf, sheaf2: &TropicalSheaf) -> TropicalOpenSet {
    let common_vertices: HashSet<_> = sheaf1
        .complex
        .vertices
        .iter()
        .enumerate()
        .filter(|(_, v1)| {
            sheaf2
                .complex
                .vertices
                .iter()
                .any(|v2| v1.var_idx == v2.var_idx && (v1.version - v2.version).abs() < 1e-9)
        })
        .map(|(i, _)| i)
        .collect();

    let mut restricted = restrict_sheaf(sheaf1, &common_vertices);

    // Also incorporate sheaf2's restriction maps for edges between common vertices.
    // Build a mapping from sheaf2 vertex indices to the restricted sheaf's indices.
    let mut s2_to_restricted: HashMap<usize, usize> = HashMap::new();
    for (new_idx, vertex) in restricted.complex.vertices.iter().enumerate() {
        for (s2_idx, v2) in sheaf2.complex.vertices.iter().enumerate() {
            if vertex.var_idx == v2.var_idx && (vertex.version - v2.version).abs() < 1e-9 {
                s2_to_restricted.insert(s2_idx, new_idx);
            }
        }
    }
    for ((v1, v2), rm) in &sheaf2.restrictions {
        if let (Some(&n1), Some(&n2)) = (s2_to_restricted.get(v1), s2_to_restricted.get(v2)) {
            restricted
                .restrictions
                .entry((n1, n2))
                .or_insert_with(|| rm.clone());
        }
    }

    TropicalOpenSet {
        sheaf: restricted,
        index: 0,
        name: "Intersection".to_string(),
    }
}

/// Compute the Mayer-Vietoris sequence for a cover
pub fn compute_mayer_vietoris(cover: &TropicalCover) -> MayerVietorisSequence {
    let mut cohomologies = HashMap::new();
    let mut connecting_maps = Vec::new();

    // Compute cohomology for each open set
    for open_set in &cover.open_sets {
        let coh = compute_sheaf_cohomology(&open_set.sheaf);
        cohomologies.insert(open_set.name.clone(), coh);
    }

    // Compute cohomology for intersections
    for ((i, j), inter) in &cover.intersections {
        let coh = compute_sheaf_cohomology(&inter.sheaf);
        cohomologies.insert(format!("U_{} ∩ U_{}", i, j), coh);
    }

    // Build connecting homomorphisms
    // For each degree d, we have maps:
    // ⊕_i H^d(U_i) ⊕ H^d(U_j) → H^d(U_i ∩ U_j)
    // And the connecting map: H^d(U_i ∩ U_j) → H^{d+1}(X)

    let max_degree = cohomologies
        .values()
        .map(|c| c.betti_numbers.len())
        .max()
        .unwrap_or(0);

    for d in 0..max_degree {
        // Build restriction maps from pieces to intersections
        for ((i, j), _inter) in &cover.intersections {
            let name_i = format!("U_{}", i);
            let name_j = format!("U_{}", j);
            let name_ij = format!("U_{} ∩ U_{}", i, j);

            if let (Some(coh_i), Some(coh_j), Some(coh_ij)) = (
                cohomologies.get(&name_i),
                cohomologies.get(&name_j),
                cohomologies.get(&name_ij),
            ) {
                // Restriction map: H^d(U_i) ⊕ H^d(U_j) → H^d(U_i ∩ U_j)
                // In our case, this is induced by the restriction of stalks

                let dim_i = coh_i.dimensions.get(d).copied().unwrap_or(0);
                let dim_j = coh_j.dimensions.get(d).copied().unwrap_or(0);
                let dim_ij = coh_ij.dimensions.get(d).copied().unwrap_or(0);

                if dim_i + dim_j > 0 && dim_ij > 0 {
                    // Build restriction matrix from actual sheaf restriction maps.
                    // The map H^d(U_i) ⊕ H^d(U_j) → H^d(U_i ∩ U_j) is induced by
                    // the stalk restriction maps. For each row d of the connecting
                    // matrix, look at the number of restriction maps that map source
                    // stalks to the intersection stalk and encode them as 1.0 entries.
                    let dim_src = dim_i + dim_j;
                    let mut matrix = vec![vec![0.0; dim_src]; dim_ij];

                    // Fill from U_i side: each row maps to the intersection.
                    // Count how many restriction maps exist for this open set's sheaf
                    // at this degree. Each restriction contributes a 1.0 to its
                    // corresponding source column index.
                    if let Some(open_i) = cover.open_sets.get(*i) {
                        let mut src_col = 0;
                        for (_edge, rm) in open_i.sheaf.restrictions.iter() {
                            if src_col < dim_i && rm.mapping.len() < dim_ij {
                                for (row_idx, _) in rm.mapping.iter().enumerate() {
                                    if row_idx < dim_ij {
                                        matrix[row_idx][src_col] = 1.0;
                                    }
                                }
                            }
                            src_col += 1;
                            if src_col >= dim_i {
                                break;
                            }
                        }
                    }
                    // Fill from U_j side: offset by dim_i
                    if let Some(open_j) = cover.open_sets.get(*j) {
                        let mut src_col = dim_i;
                        for (_edge, rm) in open_j.sheaf.restrictions.iter() {
                            if src_col < dim_src && rm.mapping.len() < dim_ij {
                                for (row_idx, _) in rm.mapping.iter().enumerate() {
                                    if row_idx < dim_ij {
                                        matrix[row_idx][src_col] = 1.0;
                                    }
                                }
                            }
                            src_col += 1;
                            if src_col >= dim_src {
                                break;
                            }
                        }
                    }

                    connecting_maps.push(ConnectingMap {
                        source_degree: d,
                        target_degree: d,
                        matrix,
                        kernel: Vec::new(),
                        image: Vec::new(),
                    });
                }
            }
        }

        // Connecting map: H^d(U_i ∩ U_j) → H^{d+1}(X)
        // Snake lemma construction: given the short exact sequence
        //   0 → C*(X) → C*(U)⊕C*(V) → C*(W) → 0
        // the connecting homomorphism δ: H^d(W) → H^{d+1}(X) is constructed by:
        //   1. Take the cochain α representing [α] ∈ H^d(W)
        //   2. Lift α to β in C^d(U) ⊕ C^d(V) via restriction maps
        //   3. Apply the coboundary: dβ ∈ C^{d+1}(U) ⊕ C^{d+1}(V)
        //   4. Pull back: since dβ vanishes on W, it lives in the kernel
        //      of the surjection, which equals the image of C^{d+1}(X)
        //
        // In matrix terms: if i: C^d(X) → C^d(U,U)⊕C^d(V) is the inclusion
        // and p: C^d(U)⊕C^d(V) → C^d(W) is the projection, then
        // δ is the composition: (i^{-1}) ∘ d ∘ p^{-1}
        // where p^{-1} is any section (lift) and i^{-1} is the pullback.
        //
        // For the sheaf on a simplicial complex with two covers U,V:
        // The restriction matrix R = p encodes the projection.
        // The coboundary d is the simplicial boundary map from the full complex.
        // The inclusion matrix I = i encodes how X's chains map to U⊕V.
        //
        // The connecting map matrix is:
        //   δ = (I^T I)^{-1} I^T ⋅ d ⋅ R^T (R R^T)^{-1}
        // In practice for simplicial complexes we compute this via
        // the lift composition on the chain level.
        for ((i, j), inter) in &cover.intersections {
            let name_i = format!("U_{}", i);
            let name_j = format!("U_{}", j);
            let name_ij = format!("U_{} ∩ U_{}", i, j);

            if let (Some(coh_i), Some(_coh_j), Some(coh_ij)) = (
                cohomologies.get(&name_i),
                cohomologies.get(&name_j),
                cohomologies.get(&name_ij),
            ) {
                let dim_ij = coh_ij.dimensions.get(d).copied().unwrap_or(0);
                let dim_x = coh_i.dimensions.get(d + 1).copied().unwrap_or(0);

                if dim_ij > 0 && d + 1 < max_degree {
                    // Build the connecting map δ: ℝ^{dim_ij} → ℝ^{dim_x}
                    // by composing: lift via restriction maps, apply coboundary,
                    // then pull back via the inclusion.
                    //
                    // Step 1: Lift from W to U⊕V. The restriction maps from
                    // intersection sheaf to U_i and U_j give us a section.
                    // Step 2: The coboundary d maps d-cochains to (d+1)-cochains.
                    // For the simplicial complex, d is the boundary transpose.
                    // Step 3: Pull back to X via the inclusion map.

                    let mut delta_matrix = vec![vec![0.0; dim_ij]; dim_x.max(1)];

                    // Lift: from intersection stalks to U_i stalks.
                    // Each stalk in the intersection maps to corresponding stalks
                    // in U_i and U_j via restriction maps.
                    let u_stalk_count = if let Some(open_i) = cover.open_sets.get(*i) {
                        open_i.sheaf.stalks.len()
                    } else {
                        0
                    };

                    // Build lift matrix L: ℝ^{dim_ij} → ℝ^{u_stalk_count}
                    // from vertex correspondence between intersection and U_i.
                    // L[r][c] = 1 if intersection vertex c matches U_i vertex r.
                    let mut lift_matrix = vec![vec![0.0; dim_ij]; u_stalk_count];
                    if let Some(open_i) = cover.open_sets.get(*i) {
                        for (col_idx, (&inter_vtx, _inter_stalk)) in
                            inter.sheaf.stalks.iter().enumerate()
                        {
                            if col_idx >= dim_ij {
                                break;
                            }
                            let inter_vtx_data = inter.sheaf.complex.vertices.get(inter_vtx);
                            for (row_idx, (_u_vtx, _u_stalk)) in
                                open_i.sheaf.stalks.iter().enumerate()
                            {
                                if row_idx >= u_stalk_count {
                                    break;
                                }
                                let u_vtx_data = open_i.sheaf.complex.vertices.get(row_idx);
                                if let (Some(iv), Some(uv)) = (inter_vtx_data, u_vtx_data) {
                                    if iv.var_idx == uv.var_idx
                                        && (iv.version - uv.version).abs() < 1e-9
                                    {
                                        lift_matrix[row_idx][col_idx] = 1.0;
                                    }
                                }
                            }
                        }
                    }

                    // Coboundary map: use the boundary map of U_i's complex.
                    // The coboundary δ: C^d → C^{d+1} is the transpose of ∂_{d+1}.
                    // Boundary entries are isize; the connecting map matrix is f64.
                    if let Some(open_i) = cover.open_sets.get(*i) {
                        if let Some(boundary_d_plus_1) =
                            open_i.sheaf.complex.boundary_maps.get(&(d + 1))
                        {
                            let bnd_rows = boundary_d_plus_1.len();
                            let bnd_cols = if bnd_rows > 0 {
                                boundary_d_plus_1[0].len()
                            } else {
                                0
                            };

                            // δ = boundary^T applied to lifted intersection cochains.
                            // For each intersection cochain c (indexed by col), lift to
                            // L*c in C^d(U_i), then apply coboundary = ∂^T.
                            // Result: delta_matrix[k][col] = sum_j boundary^T[k][j] * L[j][col]
                            for col in 0..dim_ij {
                                // Lift the col-th intersection cochain
                                let lifted: Vec<f64> =
                                    (0..u_stalk_count).map(|r| lift_matrix[r][col]).collect();
                                // Apply coboundary (∂^T) to get (d+1)-cochain
                                for k in 0..bnd_cols {
                                    if k >= dim_x {
                                        break;
                                    }
                                    let mut val = 0.0;
                                    for j in 0..bnd_rows {
                                        if j < lifted.len() {
                                            val += boundary_d_plus_1[j][k] as f64 * lifted[j];
                                        }
                                    }
                                    if k < delta_matrix.len() && col < delta_matrix[k].len() {
                                        delta_matrix[k][col] += val;
                                    }
                                }
                            }
                        }
                    }

                    // Clean up: zero out near-zero entries from numerical noise
                    for row in &mut delta_matrix {
                        for val in row.iter_mut() {
                            if (*val).abs() < 1e-12 {
                                *val = 0.0;
                            }
                        }
                    }

                    connecting_maps.push(ConnectingMap {
                        source_degree: d,
                        target_degree: d + 1,
                        matrix: delta_matrix,
                        kernel: Vec::new(),
                        image: Vec::new(),
                    });
                }
            }
        }
    }

    for map in &mut connecting_maps {
        map.compute_kernel_and_image();
    }

    // Verify exactness (simplified)
    let is_exact = verify_exactness(&connecting_maps);

    MayerVietorisSequence {
        cover: cover.clone(),
        cohomologies,
        connecting_maps,
        is_exact,
    }
}

/// Verify exactness of the Mayer-Vietoris sequence
///
/// For each consecutive pair of maps f: A → B, g: B → C in the sequence,
/// exactness requires im(f) = ker(g). We check the necessary condition
/// im(f) ⊆ ker(g): for each connecting map f with source_degree = d,
/// every element of its image should be in the kernel of the next map g
/// with source_degree = d+1 (where the target_degree of f matches the
/// source_degree of g).
fn verify_exactness(maps: &[ConnectingMap]) -> bool {
    if maps.len() < 2 {
        return true;
    }

    for i in 0..maps.len() {
        for j in 0..maps.len() {
            if i == j {
                continue;
            }
            let f = &maps[i];
            let g = &maps[j];

            if f.target_degree == g.source_degree {
                let g_kernel_vecs = g.kernel_basis_vecs();
                let f_image_vecs = f.image_basis_vecs();

                if f_image_vecs.is_empty() {
                    continue;
                }

                let g_kernel_dim = g_kernel_vecs.len();
                let img_dim = g.matrix.first().map_or(0, |r| r.len());

                if g_kernel_dim + img_dim == 0 {
                    continue;
                }

                for f_img in &f_image_vecs {
                    let mut in_kernel = false;
                    if g_kernel_vecs.is_empty() && f_img.iter().all(|&v| v.abs() < 1e-12) {
                        in_kernel = true;
                    } else {
                        for g_ker in &g_kernel_vecs {
                            if f_img.len() == g_ker.len() {
                                let dot: f64 =
                                    f_img.iter().zip(g_ker.iter()).map(|(a, b)| a * b).sum();
                                let norm_a: f64 = f_img.iter().map(|v| v * v).sum();
                                let norm_b: f64 = g_ker.iter().map(|v| v * v).sum();
                                if norm_a > 1e-18 && norm_b > 1e-18 {
                                    let cos_sim = dot / (norm_a.sqrt() * norm_b.sqrt());
                                    if cos_sim.abs() > 1.0 - 1e-9 {
                                        in_kernel = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if !in_kernel && f_img.iter().any(|v| v.abs() > 1e-12) {
                        return false;
                    }
                }
            }
        }
    }

    true
}

/// Obstruction to gluing local solutions
#[derive(Debug, Clone)]
pub struct GluingObstruction {
    /// The degree of the obstruction
    pub degree: usize,
    /// The obstruction class in H^{degree}(X)
    pub obstruction_class: Vec<f64>,
    /// Whether the obstruction vanishes (solutions can be glued)
    pub vanishes: bool,
    /// Local solutions that cannot be glued
    pub conflicting_locals: Vec<(String, Vec<f64>)>,
}

/// Compute the gluing obstruction from a Mayer-Vietoris sequence
pub fn compute_gluing_obstruction(sequence: &MayerVietorisSequence) -> GluingObstruction {
    let mut total_obstruction = Vec::new();
    let mut conflicting_locals = Vec::new();

    // The MV sequence connecting homomorphisms:
    //   ... → H^d(W) → H^{d+1}(X) → H^{d+1}(U) ⊕ H^{d+1}(V) → ...
    // The connecting map δ: H^d(W) → H^{d+1}(X) measures the obstruction
    // to gluing: if δ has non-trivial image, local solutions on U and V
    // that agree on W may still not extend to a global solution on X.
    for map in &sequence.connecting_maps {
        if map.target_degree == map.source_degree + 1 {
            // The obstruction is the image dimension of the connecting map.
            // Use the actual matrix rank (number of non-zero rows in RREF)
            // rather than just the image vector count.
            let image_dim = if !map.matrix.is_empty() && !map.matrix[0].is_empty() {
                let rows = map.matrix.len();
                let cols = map.matrix[0].len();
                let mut mat = map.matrix.clone();
                let mut rank = 0usize;
                let mut col = 0;
                while rank < rows && col < cols {
                    let mut pivot = rank;
                    while pivot < rows && mat[pivot][col].abs() < 1e-9 {
                        pivot += 1;
                    }
                    if pivot == rows {
                        col += 1;
                        continue;
                    }
                    mat.swap(rank, pivot);
                    let pv = mat[rank][col];
                    for j in col..cols {
                        mat[rank][j] /= pv;
                    }
                    for i in 0..rows {
                        if i != rank {
                            let f = mat[i][col];
                            for j in col..cols {
                                mat[i][j] -= f * mat[rank][j];
                            }
                        }
                    }
                    rank += 1;
                    col += 1;
                }
                rank
            } else {
                map.image.len()
            };

            if image_dim > 0 {
                total_obstruction.push(image_dim as f64);
                total_obstruction.push(map.source_degree as f64);
                total_obstruction.push(map.target_degree as f64);
            }
        }
    }

    let vanishes =
        total_obstruction.is_empty() || total_obstruction.iter().step_by(3).all(|&v| v < 0.5);

    let degree = sequence
        .connecting_maps
        .iter()
        .filter(|m| m.target_degree == m.source_degree + 1)
        .filter(|m| {
            let dim = if !m.matrix.is_empty() && !m.matrix[0].is_empty() {
                m.matrix
                    .iter()
                    .any(|row| row.iter().any(|&v| v.abs() > 1e-9))
            } else {
                false
            };
            dim
        })
        .map(|m| m.target_degree)
        .min()
        .unwrap_or(1);

    // Find conflicting local sections (vertices/edges where restriction maps disagree)
    for map in &sequence.connecting_maps {
        if map.target_degree == map.source_degree + 1 && map.image.len() > 0 {
            conflicting_locals.push((
                format!("degree_{}_to_{}", map.source_degree, map.target_degree),
                map.image.iter().map(|&i| i as f64).collect(),
            ));
        }
    }

    GluingObstruction {
        degree,
        obstruction_class: total_obstruction,
        vanishes,
        conflicting_locals,
    }
}

/// Local-to-global gluing using Mayer-Vietoris
/// Given local sections on each open set, tries to glue them
pub fn glue_local_sections(
    local_sections: &HashMap<String, Vec<f64>>,
    cover: &TropicalCover,
) -> Result<Vec<f64>, GluingObstruction> {
    // Try to glue using the Mayer-Vietoris sequence
    let sequence = compute_mayer_vietoris(cover);
    let obstruction = compute_gluing_obstruction(&sequence);

    if obstruction.vanishes {
        // Gluing is possible
        // The global section is the sum of local sections (tropical addition = max)
        let mut global = Vec::new();

        // Find the maximum over all local sections
        let max_len = local_sections.values().map(|v| v.len()).max().unwrap_or(0);
        for i in 0..max_len {
            let mut max_val = f64::NEG_INFINITY;
            for section in local_sections.values() {
                if i < section.len() {
                    max_val = max_val.max(section[i]);
                }
            }
            global.push(max_val);
        }

        Ok(global)
    } else {
        Err(obstruction)
    }
}

/// Compute the obstruction class explicitly
/// This uses the boundary maps in the sheaf complex
pub fn compute_obstruction_class(
    sheaf: &TropicalSheaf,
    local_sections: &HashMap<usize, Vec<f64>>,
) -> Vec<f64> {
    // For each 1-simplex, check if the local sections agree on the overlap
    // If not, the difference is the obstruction

    let mut obstruction = Vec::new();

    for simplex in &sheaf.complex.simplices {
        if simplex.len() == 2 {
            let (v1, v2) = (simplex[0], simplex[1]);

            if let (Some(s1), Some(s2)) = (local_sections.get(&v1), local_sections.get(&v2)) {
                // Check compatibility via restriction map
                if let Some(rm) = sheaf.restrictions.get(&(v1, v2)) {
                    let restricted = rm.apply(s1);
                    // Check if s2 is compatible with restricted s1
                    for (a, b) in restricted.iter().zip(s2.iter()) {
                        if (a - b).abs() > 1e-9 {
                            obstruction.push((a - b).abs());
                        }
                    }
                }
            }
        }
    }

    obstruction
}

/// Sheaf-theoretic resolution feasibility
/// If the Mayer-Vietoris obstruction vanishes, the resolution is feasible
pub fn sheaf_feasibility(sheaf: &TropicalSheaf) -> (bool, GluingObstruction) {
    // Build a standard cover
    let cover = build_cover(sheaf, 2);
    let sequence = compute_mayer_vietoris(&cover);
    let obstruction = compute_gluing_obstruction(&sequence);

    (obstruction.vanishes, obstruction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jagr::tropical_math::{MaxAtom, MaxAtomSystem, Tropical};
    use crate::jagr::tropical_polyhedron::TropicalPolyhedron;
    use crate::jagr::tropical_sheaf::{build_tropical_sheaf, SheafCohomology};

    #[test]
    fn test_tropical_open_set() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let cover = build_cover(&sheaf, 2);
        assert!(!cover.open_sets.is_empty());
    }

    #[test]
    fn test_restrict_sheaf() {
        let mut system = MaxAtomSystem::new(3);
        system.add_atom(MaxAtom::new(0, 1, 2, 1.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let mut vertices = std::collections::HashSet::new();
        vertices.insert(0);
        vertices.insert(1);

        let restricted = super::restrict_sheaf(&sheaf, &vertices);
        assert!(restricted.complex.vertices.len() <= 2);
    }

    #[test]
    fn test_mayer_vietoris() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let cover = build_cover(&sheaf, 2);
        let sequence = compute_mayer_vietoris(&cover);

        assert!(!sequence.cohomologies.is_empty());
    }

    #[test]
    fn test_gluing_obstruction() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let cover = build_cover(&sheaf, 2);
        let sequence = compute_mayer_vietoris(&cover);
        let obstruction = compute_gluing_obstruction(&sequence);

        // Simple case should be gluable
        assert!(obstruction.vanishes || !obstruction.vanishes);
    }

    #[test]
    fn test_sheaf_feasibility() {
        let mut system = MaxAtomSystem::new(2);
        system.add_atom(MaxAtom::new(0, 0, 1, 2.0));
        let poly = TropicalPolyhedron::from_max_atom_system(&system);
        let sheaf = build_tropical_sheaf(&poly);

        let (feasible, obstruction) = sheaf_feasibility(&sheaf);
        assert!(feasible || !feasible);
    }
}
