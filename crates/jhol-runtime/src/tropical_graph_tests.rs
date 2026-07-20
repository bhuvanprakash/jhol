use super::*;

#[test]
fn empty_graph_operations() {
    let g = TropicalModuleGraph::new(0);
    assert!(g.names.is_empty());
    assert!(g.shortest_paths("missing").is_empty());
    assert!(g.load_order_from("missing").is_empty());
    assert_eq!(g.spectral_radius(), 0.0);

    let (live, dead) = g.tree_shake("missing");
    assert!(live.is_empty());
    assert!(dead.is_empty());
}

#[test]
fn shortest_paths_reach_all() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("entry", "a", 0.0, false);
    g.add_edge("a", "b", 0.0, false);
    g.add_edge("entry", "c", 0.0, false);

    let paths = g.shortest_paths("entry");
    assert!(paths.contains_key("entry"));
    assert!(paths.contains_key("a"));
    assert!(paths.contains_key("b"));
    assert!(paths.contains_key("c"));
    assert_eq!(paths.len(), 4);
}

#[test]
fn single_node_graph() {
    let mut g = TropicalModuleGraph::new(16);
    let id = g.get_or_create_id("only");
    assert_eq!(id, 0);

    let paths = g.shortest_paths("only");
    assert_eq!(paths.len(), 1);
    assert!(paths.contains_key("only"));

    let order = g.load_order_from("only");
    assert_eq!(order, vec!["only"]);

    let (live, dead) = g.tree_shake("only");
    assert!(live.contains("only"));
    assert!(dead.is_empty());
}

#[test]
fn self_loop() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("a", "a", 1.0, false);

    assert!(g.has_edge("a", "a"));

    let paths = g.shortest_paths("a");
    assert_eq!(paths.len(), 1);
    assert!(paths.contains_key("a"));

    let order = g.load_order_from("a");
    assert!(order.contains(&"a".to_string()));

    assert!(g.spectral_radius() > 0.0);
}

#[test]
fn parallel_edges_combine_max_plus() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("a", "b", 1.0, false);
    g.add_edge("a", "b", 2.0, false);

    let paths = g.shortest_paths("a");
    let (cost, _) = paths.get("b").expect("b reachable");
    assert_eq!(*cost, 2.0);
}

#[test]
fn load_order_deepest_first() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("entry", "m1", 1.0, false);
    g.add_edge("m1", "m2", 1.0, false);
    g.add_edge("m2", "m3", 1.0, false);

    let order = g.load_order_from("entry");
    let pos = |name: &str| order.iter().position(|n| n == name).unwrap();

    assert!(pos("m3") < pos("m2"));
    assert!(pos("m2") < pos("m1"));
    assert!(pos("m1") < pos("entry"));
}

#[test]
fn cycle_in_load_order_includes_all_reachable() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("entry", "a", 1.0, false);
    g.add_edge("a", "b", 1.0, false);
    g.add_edge("b", "a", 1.0, false);

    let order = g.load_order_from("entry");
    assert!(order.contains(&"entry".to_string()));
    assert!(order.contains(&"a".to_string()));
    assert!(order.contains(&"b".to_string()));
    assert_eq!(order.len(), 3);
}

#[test]
fn tree_shake_marks_dead() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("entry", "a", 0.0, false);
    // `b` is isolated (optional self-loop only) and should be dead.
    g.add_edge("b", "b", 0.0, true);

    let (live, dead) = g.tree_shake("entry");
    assert!(live.contains("entry"));
    assert!(live.contains("a"));
    assert!(!live.contains("b"));
    assert!(dead.contains("b"));
}

#[test]
fn cycle_spectral_radius_positive() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("entry", "a", 1.0, false);
    g.add_edge("a", "b", 1.0, false);
    g.add_edge("b", "a", 1.0, false);

    let rho = g.spectral_radius();
    assert!(rho > 0.0, "expected positive spectral radius, got {}", rho);
}

#[test]
fn condense_removes_redundant_edges() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("entry", "a", 1.0, false);
    g.add_edge("a", "b", 1.0, false);
    g.add_edge("entry", "b", 1.0, false);

    g.condense();
    assert!(g.has_edge("entry", "a"));
    assert!(g.has_edge("a", "b"));
    assert!(!g.has_edge("entry", "b"));
}

#[test]
fn condense_collapses_two_node_scc() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("a", "b", 1.0, false);
    g.add_edge("b", "a", 1.0, false);

    g.condense();
    assert!(!g.has_edge("a", "b"));
    assert!(!g.has_edge("b", "a"));
}

#[test]
fn large_graph_runs_fast() {
    let mut g = TropicalModuleGraph::new(2048);
    for i in 0..1000 {
        let from = format!("n{}", i);
        let to = format!("n{}", i + 1);
        g.add_edge(&from, &to, 1.0, false);
    }

    let start = std::time::Instant::now();
    let order = g.load_order_from("n0");
    let elapsed = start.elapsed();

    assert_eq!(order.len(), 1001);
    assert!(
        elapsed.as_millis() < 1000,
        "load_order took {} ms",
        elapsed.as_millis()
    );
}

#[test]
#[should_panic(expected = "tropical edge cost must be non-negative finite")]
fn rejects_negative_cost() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("a", "b", -1.0, false);
}

#[test]
#[should_panic(expected = "tropical edge cost must be non-negative finite")]
fn rejects_nan_cost() {
    let mut g = TropicalModuleGraph::new(16);
    g.add_edge("a", "b", f64::NAN, false);
}
