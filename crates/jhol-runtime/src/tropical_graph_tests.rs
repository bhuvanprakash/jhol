use super::*;

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
    let debug = format!("{:?}", g);
    assert!(debug.contains("entry -> a"));
    assert!(debug.contains("a -> b"));
    assert!(
        !debug.contains("entry -> b"),
        "transitive edge entry->b should be removed: {}",
        debug
    );
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
        elapsed.as_millis() < 100,
        "load_order took {} ms",
        elapsed.as_millis()
    );
}
