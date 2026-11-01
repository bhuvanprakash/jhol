//! JAGR ↔ GRM bridge coverage (topology stress and resolution memory).

use jhol_core::jagr_infinite::chart::{VersionBitmap, VersionInterval, VersionOrdinal};
use jhol_core::jagr_infinite::holographic_rg::compute_wilsonian_rg_decimation;
use jhol_core::jagr_infinite::ricci_curvature::compute_ricci_curvatures;
use jhol_core::jagr_infinite::sheaf_laplacian::compute_sheaf_invariants;
use std::collections::{HashMap, HashSet};

#[test]
fn test_brutal_bitmap_boundary_and_overflow() {
    // Test power-of-two, 64-bit word boundaries, and large offsets
    let test_spans = [0, 1, 63, 64, 65, 127, 128, 129, 255, 256, 512, 1023, 1024];

    for &span in &test_spans {
        let lo = VersionOrdinal(10);
        let hi = VersionOrdinal(10 + span);
        let interval = VersionInterval::new(lo, hi);

        let mut bmp = VersionBitmap::empty(VersionOrdinal(0), VersionOrdinal(2000));
        assert!(bmp.is_empty());
        assert_eq!(bmp.count(), 0);

        // set_interval
        bmp.set_interval(interval);
        assert!(!bmp.is_empty());
        assert_eq!(bmp.count() as u16, span + 1);
        assert!(bmp.contains_interval(&interval));

        // pick_highest and pick_one
        assert_eq!(bmp.pick_highest(), Some(hi));
        assert_eq!(bmp.pick_one(), Some(lo));

        // Test mask_to_interval
        let mid_lo = VersionOrdinal(10 + span / 4);
        let mid_hi = VersionOrdinal(10 + (span * 3) / 4);
        let sub_interval = VersionInterval::new(mid_lo, mid_hi);

        bmp.mask_to_interval(sub_interval);
        assert!(bmp.contains_interval(&sub_interval));
        assert_eq!(bmp.pick_one(), Some(mid_lo));
        assert_eq!(bmp.pick_highest(), Some(mid_hi));
    }
}

#[test]
fn test_brutal_bitmap_exact_64_bit_crossing() {
    // Specifically target the boundary words at bits 63 and 64
    for offset in [0, 63, 64, 127, 128] {
        for len in [1, 63, 64, 65, 128] {
            let lo = VersionOrdinal(offset);
            let hi = VersionOrdinal(offset + len - 1);
            let interval = VersionInterval::new(lo, hi);

            let mut bmp = VersionBitmap::empty(VersionOrdinal(0), VersionOrdinal(500));
            bmp.set_interval(interval);
            assert!(bmp.contains_interval(&interval));
            assert_eq!(bmp.count(), len as u32);
        }
    }
}

#[test]
fn test_brutal_bitmap_simd_set_operations() {
    let mut b1 = VersionBitmap::empty(VersionOrdinal(0), VersionOrdinal(300));
    let mut b2 = VersionBitmap::empty(VersionOrdinal(0), VersionOrdinal(300));

    // Even versions in b1, odd in b2
    for i in (0..=300).step_by(2) {
        b1.set_interval(VersionInterval::new(VersionOrdinal(i), VersionOrdinal(i)));
    }
    for i in (1..=300).step_by(2) {
        b2.set_interval(VersionInterval::new(VersionOrdinal(i), VersionOrdinal(i)));
    }

    assert_eq!(b1.count(), 151);
    assert_eq!(b2.count(), 150);

    // Disjoint intersection
    let mut intersect = b1.clone();
    intersect.intersect(&b2);
    assert!(intersect.is_empty());
    assert_eq!(intersect.count(), 0);

    // Full union
    let mut union = b1.clone();
    union.union(&b2);
    assert_eq!(union.count(), 301);
}

#[test]
fn test_brutal_wilsonian_rg_deep_dag() {
    // Construct a deep 100-level DAG:
    // Root -> L1 -> L2 -> ... -> L99 -> 50 leaves
    let mut nodes = HashSet::new();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();

    nodes.insert("root".to_string());
    let mut prev = "root".to_string();

    for level in 1..=100 {
        let curr = format!("L{}", level);
        nodes.insert(curr.clone());
        adj.entry(prev).or_default().push(curr.clone());
        prev = curr;
    }

    // Attach 50 leaves to L100
    for leaf in 0..50 {
        let leaf_name = format!("leaf_{}", leaf);
        nodes.insert(leaf_name.clone());
        adj.entry(prev.clone()).or_default().push(leaf_name);
    }

    let rg = compute_wilsonian_rg_decimation(&nodes, &adj);
    assert_eq!(rg.initial_nodes, 151);
    // Wilsonian flow collapses the entire unconstrained non-conflicting leaf tail!
    assert!(rg.eliminated_uv_modes >= 50);
    assert!(rg.coarse_grained_nodes <= 101);
    assert!(rg.decimation_time_us < 10000.0); // Under 10ms
}

#[test]
fn test_brutal_wilsonian_rg_star_topology() {
    // Hub with 200 peripheral leaf packages
    let mut nodes = HashSet::new();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();

    nodes.insert("hub".to_string());
    for i in 0..200 {
        let leaf = format!("spoke_{}", i);
        nodes.insert(leaf.clone());
        adj.entry("hub".to_string()).or_default().push(leaf);
    }

    let rg = compute_wilsonian_rg_decimation(&nodes, &adj);
    assert_eq!(rg.initial_nodes, 201);
    assert_eq!(rg.eliminated_uv_modes, 201);
    assert_eq!(rg.coarse_grained_nodes, 1);
    assert!(rg.compression_ratio >= 200.0);
}

#[test]
fn test_brutal_wilsonian_rg_pure_cycle() {
    // Pure cyclic graph: A -> B -> C -> D -> A (no leaves, out-degree never 0)
    let nodes: HashSet<String> = ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect();
    let adj: HashMap<String, Vec<String>> = HashMap::from([
        ("A".to_string(), vec!["B".to_string()]),
        ("B".to_string(), vec!["C".to_string()]),
        ("C".to_string(), vec!["D".to_string()]),
        ("D".to_string(), vec!["A".to_string()]),
    ]);

    let rg = compute_wilsonian_rg_decimation(&nodes, &adj);
    assert_eq!(rg.initial_nodes, 4);
    assert_eq!(rg.eliminated_uv_modes, 0); // Cycles are irreducible IR modes!
    assert_eq!(rg.coarse_grained_nodes, 4);
}

#[test]
fn test_brutal_forman_ricci_bottleneck_detection() {
    // Construct a double-star network joined by a single bridge edge (u, v):
    // 50 left nodes -> u -> v -> 50 right nodes
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();

    for i in 0..50 {
        adj.entry(format!("L{}", i))
            .or_default()
            .push("u".to_string());
    }
    adj.entry("u".to_string())
        .or_default()
        .push("v".to_string());
    for i in 0..50 {
        adj.entry("v".to_string())
            .or_default()
            .push(format!("R{}", i));
    }

    let ricci = compute_ricci_curvatures(&adj);
    assert_eq!(ricci.total_edges, 101);
    // The bridge edge u -> v connects two high-degree hubs, producing extreme negative curvature!
    assert!(ricci.min_curvature <= -50);
    let top = ricci.top_bottleneck.as_ref().unwrap();
    assert_eq!(top.0, "u");
    assert_eq!(top.1, "v");
}

#[test]
fn test_brutal_forman_ricci_complete_graph() {
    // Complete graph K_5 (all pairs connected)
    // In complete graphs, triangles Δ(u, v) = 3 for every edge
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let names = ["A", "B", "C", "D", "E"];

    for &u in &names {
        for &v in &names {
            if u != v {
                adj.entry(u.to_string()).or_default().push(v.to_string());
            }
        }
    }

    let ricci = compute_ricci_curvatures(&adj);
    assert_eq!(ricci.total_edges, 20);
    // Positive or zero curvature because triangles compensate degrees
    assert!(ricci.min_curvature >= -20);
}

#[test]
fn test_brutal_sheaf_laplacian_topological_invariants() {
    // Complex with:
    // 1. Component 1: Tree (3 nodes, 2 edges, β₀=1, β₁=0)
    // 2. Component 2: Dual Cycles + Back-edge (4 nodes, 6 edges, β₀=1, β₁=3, β₂=1)
    let nodes: HashSet<String> = [
        "T1", "T2", "T3", // Tree
        "C1", "C2", "C3", "C4", // Cycle complex
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let adj: HashMap<String, Vec<String>> = HashMap::from([
        ("T1".to_string(), vec!["T2".to_string()]),
        ("T2".to_string(), vec!["T3".to_string()]),
        ("C1".to_string(), vec!["C2".to_string(), "C3".to_string()]),
        ("C2".to_string(), vec!["C4".to_string(), "C3".to_string()]), // combined: 2 edges
        ("C3".to_string(), vec!["C4".to_string()]),
        ("C4".to_string(), vec!["C1".to_string()]), // back edge closes outer cycle: 8 total edges
    ]);

    let inv = compute_sheaf_invariants(&nodes, &adj);
    // 2 disconnected components: {T1, T2, T3} and {C1, C2, C3, C4}
    assert_eq!(inv.betti_0, 2);
    // 1D cycle rank: |E| - |V| + β₀ = 8 - 7 + 2 = 3
    assert_eq!(inv.betti_1, 3);
    // Triangles (β₂) >= 1 (C1, C2, C3 form a directed triangle)
    assert!(inv.betti_2 >= 1);
    // Disconnected graph has Fiedler value = 0.0
    assert_eq!(inv.fiedler_value, 0.0);
}

#[test]
fn test_brutal_empty_and_degenerate_graphs() {
    // Empty graph
    let empty_nodes = HashSet::new();
    let empty_adj = HashMap::new();

    let rg = compute_wilsonian_rg_decimation(&empty_nodes, &empty_adj);
    assert_eq!(rg.initial_nodes, 0);

    let ricci = compute_ricci_curvatures(&empty_adj);
    assert_eq!(ricci.total_edges, 0);
    assert_eq!(ricci.bottleneck_count, 0);

    let sheaf = compute_sheaf_invariants(&empty_nodes, &empty_adj);
    assert_eq!(sheaf.betti_0, 0);
    assert_eq!(sheaf.betti_1, 0);
    assert_eq!(sheaf.betti_2, 0);
}
