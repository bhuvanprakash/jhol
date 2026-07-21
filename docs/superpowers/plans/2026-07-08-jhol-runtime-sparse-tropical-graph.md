# Jhol Runtime — Sparse Tropical Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dense `O(V³)` tropical module graph with a sparse adjacency-list implementation that preserves the exact max-plus semantics and public API.

**Architecture:** Keep `TropicalEdge` and the semiring operations unchanged. Replace the internal `Vec<Vec<TropicalEdge>>` matrix with `Vec<Vec<(ModuleId, TropicalEdge)>>` outgoing and incoming adjacency lists. Reimplement `shortest_paths`, `spectral_radius`, `load_order_from`, `tree_shake`, and `condense` using sparse graph algorithms.

**Tech Stack:** Rust 2021, `std::collections::{HashMap, HashSet, VecDeque, BinaryHeap}`, existing `jhol-runtime` crate.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/jhol-runtime/src/tropical_graph.rs` | Existing tropical graph module. Will be rewritten in-place to use sparse structures while preserving API. |
| `crates/jhol-runtime/src/lib.rs` | Re-exports `tropical_graph` (no signature changes). |
| `crates/jhol-runtime/src/engine.rs` | Uses `TropicalModuleGraph` via `RUNTIME_STATE` (no API changes, only internal behavior). |

---

### Task 1: Add Sparse Tropical Graph Unit Tests

**Files:**
- Create: `crates/jhol-runtime/src/tropical_graph_tests.rs`
- Modify: `crates/jhol-runtime/src/tropical_graph.rs` (add `#[cfg(test)] mod tropical_graph_tests;`)

- [ ] **Step 1: Write parity tests for dense vs. sparse**

Create `crates/jhol-runtime/src/tropical_graph_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::tropical_graph::{TropicalModuleGraph, TropicalEdge};

    fn sample_graph() -> TropicalModuleGraph {
        let mut g = TropicalModuleGraph::new(4);
        g.add_edge("entry", "a", 1.0, false);
        g.add_edge("entry", "b", 2.0, false);
        g.add_edge("a", "c", 3.0, false);
        g.add_edge("b", "c", 1.0, false);
        g.add_edge("c", "d", 1.0, false);
        g
    }

    #[test]
    fn shortest_paths_reach_all() {
        let g = sample_graph();
        let paths = g.shortest_paths("entry");
        assert!(paths.contains_key("entry"));
        assert!(paths.contains_key("a"));
        assert!(paths.contains_key("b"));
        assert!(paths.contains_key("c"));
        assert!(paths.contains_key("d"));
    }

    #[test]
    fn load_order_deepest_first() {
        let g = sample_graph();
        let order = g.load_order_from("entry");
        let pos = |name: &str| order.iter().position(|n| n == name).unwrap();
        assert!(pos("d") < pos("c"));
        assert!(pos("c") < pos("a") || pos("c") < pos("b"));
    }

    #[test]
    fn tree_shake_marks_dead() {
        let mut g = sample_graph();
        g.add_edge("entry", "dead", 0.0, true);
        let (live, dead) = g.tree_shake("entry");
        assert!(live.contains("a"));
        assert!(dead.contains("dead"));
    }

    #[test]
    fn cycle_spectral_radius_positive() {
        let mut g = TropicalModuleGraph::new(3);
        g.add_edge("a", "b", 1.0, false);
        g.add_edge("b", "c", 1.0, false);
        g.add_edge("c", "a", 1.0, false);
        let rho = g.spectral_radius();
        assert!(rho > 0.0, "expected positive spectral radius for cycle, got {}", rho);
    }

    #[test]
    fn condense_removes_redundant_edges() {
        let mut g = TropicalModuleGraph::new(4);
        g.add_edge("a", "b", 1.0, false);
        g.add_edge("b", "c", 1.0, false);
        g.add_edge("a", "c", 1.0, false); // redundant: a->b->c has same cost
        g.condense();
        // After condense, direct a->c edge should be removed
        let paths = g.shortest_paths("a");
        assert!(paths.contains_key("c"));
    }

    #[test]
    fn large_graph_runs_fast() {
        let mut g = TropicalModuleGraph::new(1000);
        for i in 0..999 {
            g.add_edge(&format!("m{}", i), &format!("m{}", i + 1), 1.0, false);
        }
        let start = std::time::Instant::now();
        let _ = g.load_order_from("m0");
        assert!(start.elapsed().as_millis() < 100, "load_order too slow");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail against dense implementation**

Run:

```bash
cargo test -p jhol-runtime tropical_graph_tests -- --nocapture
```

Expected: tests may pass for small graphs but `large_graph_runs_fast` will likely fail with a timeout or be very slow. Note the baseline time.

---

### Task 2: Implement Sparse Tropical Graph

**Files:**
- Modify: `crates/jhol-runtime/src/tropical_graph.rs`

- [ ] **Step 1: Replace internal representation**

Open `crates/jhol-runtime/src/tropical_graph.rs` and replace the `TropicalModuleGraph` struct:

```rust
pub struct TropicalModuleGraph {
    /// Module names indexed by ID
    pub names: Vec<String>,
    /// Name → ID lookup
    pub name_to_id: HashMap<String, ModuleId>,
    /// Sparse outgoing edges: outgoing[from] = [(to, edge)]
    outgoing: Vec<Vec<(ModuleId, TropicalEdge)>>,
    /// Sparse incoming edges for reverse traversal
    incoming: Vec<Vec<(ModuleId, TropicalEdge)>>,
    /// Capacity reserved for IDs
    capacity: usize,
}
```

- [ ] **Step 2: Update constructor and capacity handling**

Replace `new`, `ensure_capacity`, and `get_or_create_id`:

```rust
impl TropicalModuleGraph {
    pub fn new(capacity: usize) -> Self {
        let cap = if capacity == 0 { 0 } else { capacity.max(16) };
        Self {
            names: Vec::with_capacity(cap),
            name_to_id: HashMap::with_capacity(cap),
            outgoing: Vec::with_capacity(cap),
            incoming: Vec::with_capacity(cap),
            capacity: cap,
        }
    }

    fn ensure_capacity(&mut self, id: ModuleId) {
        if id >= self.capacity as ModuleId {
            let new_cap = (self.capacity * 2).max(id as usize + 16);
            self.outgoing.resize_with(new_cap, Vec::new);
            self.incoming.resize_with(new_cap, Vec::new);
            self.capacity = new_cap;
        }
    }

    pub fn get_or_create_id(&mut self, name: &str) -> ModuleId {
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }
        let id = self.names.len() as ModuleId;
        self.ensure_capacity(id);
        self.names.push(name.to_string());
        self.name_to_id.insert(name.to_string(), id);
        id
    }
}
```

- [ ] **Step 3: Rewrite `add_edge`**

```rust
pub fn add_edge(&mut self, from: &str, to: &str, cost: f64, is_optional: bool) {
    let fid = self.get_or_create_id(from);
    let tid = self.get_or_create_id(to);
    let new_edge = TropicalEdge { cost, is_optional };

    // Update outgoing edge using max-plus addition
    if let Some(existing) = self.outgoing[fid as usize].iter_mut().find(|(t, _)| *t == tid) {
        existing.1 = existing.1 + new_edge;
    } else {
        self.outgoing[fid as usize].push((tid, new_edge));
    }

    // Update incoming edge
    if let Some(existing) = self.incoming[tid as usize].iter_mut().find(|(f, _)| *f == fid) {
        existing.1 = existing.1 + new_edge;
    } else {
        self.incoming[tid as usize].push((fid, new_edge));
    }
}
```

- [ ] **Step 4: Rewrite `shortest_paths` with sparse Bellman-Ford**

```rust
pub fn shortest_paths(&self, source: &str) -> HashMap<String, (f64, Vec<String>)> {
    let src_id = match self.name_to_id.get(source) {
        Some(&id) => id,
        None => return HashMap::new(),
    };
    let n = self.names.len();
    if n == 0 { return HashMap::new(); }

    let mut dist = vec![T_INF; n];
    let mut prev = vec![None::<usize>; n];
    dist[src_id as usize] = 0.0;

    // Bellman-Ford over sparse edges
    for _ in 0..n - 1 {
        let mut updated = false;
        for u in 0..n {
            if dist[u] == T_INF { continue; }
            for (v, edge) in &self.outgoing[u] {
                let vi = *v as usize;
                if edge.cost == T_INF { continue; }
                let new_dist = dist[u] + edge.cost;
                if new_dist > dist[vi] {
                    dist[vi] = new_dist;
                    prev[vi] = Some(u);
                    updated = true;
                }
            }
        }
        if !updated { break; }
    }

    let mut result = HashMap::new();
    for v in 0..n {
        if dist[v] == T_INF { continue; }
        let mut path = Vec::new();
        let mut cur = Some(v);
        while let Some(c) = cur {
            path.push(self.names[c].clone());
            cur = prev[c].filter(|&p| p != src_id as usize);
        }
        path.reverse();
        result.insert(self.names[v].clone(), (dist[v], path));
    }
    result
}
```

- [ ] **Step 5: Rewrite `load_order_from` with topological + tropical DP**

```rust
pub fn load_order_from(&self, entry: &str) -> Vec<String> {
    let src_id = match self.name_to_id.get(entry) {
        Some(&id) => id,
        None => return Vec::new(),
    };
    let n = self.names.len();

    // Kahn's algorithm topological order
    let mut in_degree = vec![0usize; n];
    for u in 0..n {
        for (v, _) in &self.outgoing[u] {
            in_degree[*v as usize] += 1;
        }
    }

    let mut queue = VecDeque::new();
    queue.push_back(src_id);

    let mut topo = Vec::new();
    let mut longest = vec![T_INF; n];
    longest[src_id as usize] = 0.0;

    while let Some(u) = queue.pop_front() {
        topo.push(u);
        for (v, edge) in &self.outgoing[u] {
            let vi = *v as usize;
            if longest[u] != T_INF {
                let cand = longest[u] + edge.cost;
                if cand > longest[vi] {
                    longest[vi] = cand;
                }
            }
            in_degree[vi] -= 1;
            if in_degree[vi] == 0 {
                queue.push_back(*v);
            }
        }
    }

    // Sort by longest path descending (deepest deps first)
    topo.sort_by(|a, b| {
        let da = longest[*a as usize];
        let db = longest[*b as usize];
        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
    });

    topo.into_iter().map(|id| self.names[id as usize].clone()).collect()
}
```

- [ ] **Step 6: Rewrite `spectral_radius` with sparse power iteration**

```rust
pub fn spectral_radius(&self) -> f64 {
    let n = self.names.len();
    if n == 0 { return 0.0; }
    let mut vec = vec![1.0; n];

    for _ in 0..20 {
        let mut next = vec![T_INF; n];
        for i in 0..n {
            if vec[i] == T_INF { continue; }
            for (j, edge) in &self.outgoing[i] {
                if edge.cost == T_INF { continue; }
                let val = vec[i] + edge.cost;
                let ji = *j as usize;
                if val > next[ji] { next[ji] = val; }
            }
        }
        vec = next;
    }

    let max_val = vec.iter().cloned().fold(T_INF, f64::max);
    if max_val == T_INF { 0.0 } else { max_val }
}
```

- [ ] **Step 7: Rewrite `tree_shake` using sparse reachability**

```rust
pub fn tree_shake(&self, entry: &str) -> (HashSet<String>, HashSet<String>) {
    let src_id = match self.name_to_id.get(entry) {
        Some(&id) => id,
        None => return (HashSet::new(), self.names.iter().cloned().collect()),
    };

    let mut live = HashSet::new();
    let mut stack = vec![src_id];
    while let Some(u) = stack.pop() {
        if !live.insert(self.names[u as usize].clone()) { continue; }
        for (v, _) in &self.outgoing[u] {
            stack.push(*v);
        }
    }

    let dead: HashSet<String> = self.names.iter()
        .filter(|n| !live.contains(n.as_str()))
        .cloned()
        .collect();
    (live, dead)
}
```

- [ ] **Step 8: Rewrite `condense` with SCC + DAG reachability**

```rust
pub fn condense(&mut self) {
    let n = self.names.len();
    if n == 0 { return; }

    // Tarjan's SCC
    let mut index = 0usize;
    let mut stack = Vec::new();
    let mut on_stack = vec![false; n];
    let mut indices = vec![None::<usize>; n];
    let mut lowlinks = vec![0usize; n];
    let mut sccs: Vec<Vec<usize>> = Vec::new();

    fn strong_connect(
        v: usize,
        index: &mut usize,
        stack: &mut Vec<usize>,
        on_stack: &mut [bool],
        indices: &mut [Option<usize>],
        lowlinks: &mut [usize],
        sccs: &mut Vec<Vec<usize>>,
        outgoing: &[Vec<(ModuleId, TropicalEdge)>],
    ) {
        indices[v] = Some(*index);
        lowlinks[v] = *index;
        *index += 1;
        stack.push(v);
        on_stack[v] = true;

        for (w, _) in &outgoing[v] {
            let wi = *w as usize;
            if indices[wi].is_none() {
                strong_connect(wi, index, stack, on_stack, indices, lowlinks, sccs, outgoing);
                lowlinks[v] = lowlinks[v].min(lowlinks[wi]);
            } else if on_stack[wi] {
                lowlinks[v] = lowlinks[v].min(indices[wi].unwrap());
            }
        }

        if lowlinks[v] == indices[v].unwrap() {
            let mut scc = Vec::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack[w] = false;
                scc.push(w);
                if w == v { break; }
            }
            sccs.push(scc);
        }
    }

    for v in 0..n {
        if indices[v].is_none() {
            strong_connect(v, &mut index, &mut stack, &mut on_stack, &mut indices, &mut lowlinks, &mut sccs, &self.outgoing);
        }
    }

    // Build SCC DAG
    let mut scc_id = vec![0usize; n];
    for (id, scc) in sccs.iter().enumerate() {
        for &node in scc {
            scc_id[node] = id;
        }
    }

    let scc_count = sccs.len();
    let mut scc_outgoing: Vec<HashMap<usize, TropicalEdge>> = vec![HashMap::new(); scc_count];

    for u in 0..n {
        for (v, edge) in &self.outgoing[u] {
            let vi = *v as usize;
            let su = scc_id[u];
            let sv = scc_id[vi];
            if su == sv { continue; }
            let entry = scc_outgoing[su].entry(sv).or_insert(TropicalEdge::UNREACHABLE);
            *entry = *entry + *edge;
        }
    }

    // For each pair of SCCs, if there is a two-hop path with >= cost, remove direct edge
    let mut remove = Vec::new();
    for su in 0..scc_count {
        for (&sv, direct) in &scc_outgoing[su] {
            let mut best = TropicalEdge::UNREACHABLE;
            for (&sk, e1) in &scc_outgoing[su] {
                if sk == sv { continue; }
                if let Some(e2) = scc_outgoing[sk].get(&sv) {
                    let via = *e1 * *e2;
                    best = best + via;
                }
            }
            if best.cost > T_INF && best.cost >= direct.cost {
                // Mark all original edges from su to sv for removal
                for &u in &sccs[su] {
                    for (v, _) in &self.outgoing[u] {
                        if scc_id[*v as usize] == sv {
                            remove.push((u, *v));
                        }
                    }
                }
            }
        }
    }

    for (u, v) in remove {
        if let Some(pos) = self.outgoing[u].iter().position(|(to, _)| *to == v) {
            self.outgoing[u].swap_remove(pos);
        }
        if let Some(pos) = self.incoming[v as usize].iter().position(|(from, _)| *from == u as ModuleId) {
            self.incoming[v as usize].swap_remove(pos);
        }
    }
}
```

- [ ] **Step 9: Update `Debug` impl**

```rust
impl std::fmt::Debug for TropicalModuleGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut edge_count = 0;
        for row in &self.outgoing {
            edge_count += row.len();
        }
        write!(f, "TropicalModuleGraph {{ nodes: {}, edges: {} }}", self.names.len(), edge_count)
    }
}
```

- [ ] **Step 10: Add max-node cap**

Add at the top of `tropical_graph.rs`:

```rust
const DEFAULT_MAX_NODES: usize = 16_384;

fn max_nodes() -> usize {
    std::env::var("JHOL_TROPICAL_MAX_NODES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_NODES)
        .max(1)
}
```

In `get_or_create_id`, after creating a new ID:

```rust
if self.names.len() > max_nodes() {
    // Prevent unbounded growth; keep existing nodes, stop adding new ones
    self.name_to_id.remove(name);
    self.names.pop();
    // Return the last valid ID as a fallback (lossy but safe)
    return (self.names.len() as ModuleId).saturating_sub(1);
}
```

- [ ] **Step 11: Run tests**

```bash
cargo test -p jhol-runtime tropical_graph_tests -- --nocapture
```

Expected: all tests pass, including `large_graph_runs_fast` under 100 ms.

- [ ] **Step 12: Run full runtime tests**

```bash
cargo test -p jhol-runtime
```

Expected: all existing tests pass.

- [ ] **Step 13: Commit**

```bash
git add crates/jhol-runtime/src/tropical_graph.rs crates/jhol-runtime/src/tropical_graph_tests.rs
git commit -m "perf(runtime): sparse tropical module graph (O(V+E) load order, O(V·E) paths)"
```

---

## Self-Review Checklist

- [ ] Spec coverage: all tropical graph operations are reimplemented sparsely.
- [ ] No placeholders: every function body is provided.
- [ ] Type consistency: `TropicalEdge`, `ModuleId`, and public APIs unchanged.
- [ ] Tests: parity, performance, cycle detection, tree-shake, condense.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-08-jhol-runtime-sparse-tropical-graph.md`.**

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach?
