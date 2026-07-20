//! Tropical Module Graph — sparse max-plus semiring algebra for module
//! resolution, cycle detection, load ordering, and tree-shaking.
//!
//! Internals are stored as adjacency lists so the common operations are
//! linear or near-linear in the number of edges instead of cubic in the
//! number of nodes. A dense compatibility matrix (`mat`) is also maintained
//! for callers that still rely on the old public field.

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;

const T_INF: f64 = f64::NEG_INFINITY;

/// Represents a module (file path, package name, or URL).
pub type ModuleId = u32;

/// Edge weight in the tropical semiring: (cost, is_optional).
/// cost = f64 where -inf means unreachable, >=0 means reachable.
/// is_optional = true means this import can be skipped if dead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TropicalEdge {
    pub cost: f64,
    pub is_optional: bool,
}

impl TropicalEdge {
    pub const UNREACHABLE: TropicalEdge = TropicalEdge {
        cost: T_INF,
        is_optional: false,
    };
    pub const DIRECT: TropicalEdge = TropicalEdge {
        cost: 0.0,
        is_optional: false,
    };
}

/// Tropical semiring addition (max-plus): max(a, b)
impl std::ops::Add for TropicalEdge {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        TropicalEdge {
            cost: self.cost.max(rhs.cost),
            is_optional: self.is_optional || rhs.is_optional,
        }
    }
}

/// Tropical semiring multiplication (max-plus): a + b
impl std::ops::Mul for TropicalEdge {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        if self.cost == T_INF || rhs.cost == T_INF {
            TropicalEdge::UNREACHABLE
        } else {
            TropicalEdge {
                cost: self.cost + rhs.cost,
                is_optional: self.is_optional || rhs.is_optional,
            }
        }
    }
}

/// Module dependency as a tropical edge in the graph.
#[derive(Clone, Debug)]
pub struct ModuleDep {
    pub from: ModuleId,
    pub to: ModuleId,
    pub edge: TropicalEdge,
}

/// The tropical module graph stored as sparse adjacency lists, with a dense
/// compatibility matrix (`mat`) kept in sync.
pub struct TropicalModuleGraph {
    /// Module names indexed by ID.
    pub names: Vec<String>,
    /// Name → ID lookup.
    pub name_to_id: HashMap<String, ModuleId>,
    /// Dense compatibility adjacency matrix. `mat[from][to]` is the tropical
    /// edge between the two modules, or `TropicalEdge::UNREACHABLE`.
    pub mat: Vec<Vec<TropicalEdge>>,
    /// Outgoing adjacency list: outgoing[from] = [(to, edge), ...]
    outgoing: Vec<Vec<(ModuleId, TropicalEdge)>>,
    /// Incoming adjacency list: incoming[to] = [(from, edge), ...]
    incoming: Vec<Vec<(ModuleId, TropicalEdge)>>,
    /// Current allocated matrix capacity.
    capacity: usize,
    /// Hard node limit (from `JHOL_TROPICAL_MAX_NODES`, default 16_384).
    max_nodes: usize,
}

impl TropicalModuleGraph {
    pub fn new(capacity: usize) -> Self {
        let cap = if capacity == 0 { 16 } else { capacity };
        let mat = if cap > 0 {
            vec![vec![TropicalEdge::UNREACHABLE; cap]; cap]
        } else {
            vec![]
        };
        Self {
            names: Vec::with_capacity(cap),
            name_to_id: HashMap::with_capacity(cap),
            mat,
            outgoing: Vec::with_capacity(cap),
            incoming: Vec::with_capacity(cap),
            capacity: cap,
            max_nodes: parse_max_nodes_cap(),
        }
    }

    fn ensure_capacity(&mut self, id: ModuleId) {
        let id_usize = id as usize;
        if id_usize >= self.capacity {
            let new_cap = (self.capacity * 2).max(id_usize + 16);
            for row in self.mat.iter_mut() {
                row.resize(new_cap, TropicalEdge::UNREACHABLE);
            }
            let old_cap = self.capacity;
            self.mat
                .resize(new_cap, vec![TropicalEdge::UNREACHABLE; new_cap]);
            for row in self.mat.iter_mut().take(old_cap) {
                row.resize(new_cap, TropicalEdge::UNREACHABLE);
            }
            self.outgoing.resize_with(new_cap, Vec::new);
            self.incoming.resize_with(new_cap, Vec::new);
            self.capacity = new_cap;
        }
    }

    pub fn get_or_create_id(&mut self, name: &str) -> ModuleId {
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }
        if self.names.len() >= self.max_nodes {
            panic!(
                "TropicalModuleGraph node limit {} exceeded; set JHOL_TROPICAL_MAX_NODES to raise it",
                self.max_nodes
            );
        }
        let id = self.names.len() as ModuleId;
        self.ensure_capacity(id);
        if self.outgoing.len() <= id as usize {
            self.outgoing.push(Vec::new());
            self.incoming.push(Vec::new());
        }
        self.names.push(name.to_string());
        self.name_to_id.insert(name.to_string(), id);
        id
    }

    pub fn add_edge(&mut self, from: &str, to: &str, cost: f64, is_optional: bool) {
        if cost.is_nan() || cost < 0.0 {
            panic!(
                "tropical edge cost must be non-negative finite, got {}",
                cost
            );
        }
        let fid = self.get_or_create_id(from);
        let tid = self.get_or_create_id(to);
        let new_edge = TropicalEdge { cost, is_optional };

        // Keep the dense matrix in sync.
        let existing_mat = self.mat[fid as usize][tid as usize];
        self.mat[fid as usize][tid as usize] = existing_mat + new_edge;

        // Combine parallel edges with tropical (max-plus) addition.
        let out = &mut self.outgoing[fid as usize];
        if let Some(existing) = out.iter_mut().find(|(t, _)| *t == tid) {
            existing.1 = existing.1 + new_edge;
        } else {
            out.push((tid, new_edge));
        }

        let inc = &mut self.incoming[tid as usize];
        if let Some(existing) = inc.iter_mut().find(|(f, _)| *f == fid) {
            existing.1 = existing.1 + new_edge;
        } else {
            inc.push((fid, new_edge));
        }
    }

    /// Returns true if a finite edge exists between `from` and `to`.
    pub fn has_edge(&self, from: &str, to: &str) -> bool {
        match (self.name_to_id.get(from), self.name_to_id.get(to)) {
            (Some(&f), Some(&t)) => self.mat[f as usize][t as usize].cost.is_finite(),
            _ => false,
        }
    }

    /// Sparse tropical shortest path (Bellman-Ford max-plus).
    /// Returns cost + path for each reachable module from `source`.
    pub fn shortest_paths(&self, source: &str) -> HashMap<String, (f64, Vec<String>)> {
        let src_id = match self.name_to_id.get(source) {
            Some(&id) => id,
            None => return HashMap::new(),
        };
        let n = self.names.len();
        let mut dist = vec![T_INF; n];
        let mut prev = vec![None::<usize>; n];
        dist[src_id as usize] = 0.0;

        if n > 1 {
            for _ in 0..n - 1 {
                for u in 0..n {
                    let du = dist[u];
                    if !du.is_finite() {
                        continue;
                    }
                    for (v, edge) in &self.outgoing[u] {
                        if !edge.cost.is_finite() {
                            continue;
                        }
                        let v = *v as usize;
                        let new_dist = du + edge.cost;
                        // Max-plus: larger (longer) path wins.
                        if new_dist > dist[v] {
                            dist[v] = new_dist;
                            prev[v] = Some(u);
                        }
                    }
                }
            }
        }

        let mut result = HashMap::new();
        for v in 0..n {
            if !dist[v].is_finite() {
                continue;
            }
            let mut path = vec![self.names[v].clone()];
            let mut cur = prev[v];
            while let Some(p) = cur {
                if p == src_id as usize {
                    break;
                }
                path.push(self.names[p].clone());
                cur = prev[p];
            }
            path.reverse();
            result.insert(self.names[v].clone(), (dist[v], path));
        }
        result
    }

    /// Spectral radius approximation via sparse max-plus power iteration.
    /// Returns the largest eigenvalue (spectral radius).
    /// If > 0, there's a cycle in the module graph.
    pub fn spectral_radius(&self) -> f64 {
        let n = self.names.len();
        if n == 0 {
            return 0.0;
        }
        let mut vec = vec![1.0; n];
        for _ in 0..20 {
            let mut next = vec![T_INF; n];
            for (i, vi) in vec.iter().enumerate() {
                let vi: f64 = *vi;
                if !vi.is_finite() {
                    continue;
                }
                for (j, edge) in &self.outgoing[i] {
                    if !edge.cost.is_finite() {
                        continue;
                    }
                    let val = vi + edge.cost;
                    let j = *j as usize;
                    if val > next[j] {
                        next[j] = val;
                    }
                }
            }
            vec = next;
        }
        let max_val = vec.iter().copied().fold(T_INF, f64::max);
        if max_val == T_INF {
            0.0
        } else {
            max_val
        }
    }

    /// Topological load order from a tropical longest-path DP.
    /// Returns modules in dependency order (deepest deps first).
    pub fn load_order_from(&self, entry: &str) -> Vec<String> {
        let src_id = match self.name_to_id.get(entry) {
            Some(&id) => id,
            None => return Vec::new(),
        };
        let n = self.names.len();
        let reachable = self.reachable_module_ids(src_id);

        // Kahn's topological sort over the reachable sub-graph. We start from
        // sinks (modules with no outgoing dependencies) so deepest deps appear
        // first after the final cost-based sort.
        let mut out_deg = vec![0usize; n];
        for &u in &reachable {
            let u = u as usize;
            out_deg[u] = self.outgoing[u]
                .iter()
                .filter(|(v, _)| reachable.contains(v))
                .count();
        }

        let mut queue = VecDeque::new();
        for &u in &reachable {
            if out_deg[u as usize] == 0 {
                queue.push_back(u);
            }
        }

        let mut topo = Vec::with_capacity(reachable.len());
        let mut in_topological_order = vec![false; n];
        while let Some(u) = queue.pop_front() {
            let u = u as usize;
            topo.push(u);
            in_topological_order[u] = true;
            for (pred, _) in &self.incoming[u] {
                if !reachable.contains(pred) {
                    continue;
                }
                let p = *pred as usize;
                out_deg[p] = out_deg[p].saturating_sub(1);
                if out_deg[p] == 0 {
                    queue.push_back(*pred);
                }
            }
        }

        // Longest-path DP in reverse topological order.
        let mut dist = vec![T_INF; n];
        dist[src_id as usize] = 0.0;
        for &u in topo.iter().rev() {
            if !dist[u].is_finite() {
                continue;
            }
            for (v, edge) in &self.outgoing[u] {
                if !reachable.contains(v) {
                    continue;
                }
                let v = *v as usize;
                let nd = dist[u] + edge.cost;
                if nd > dist[v] {
                    dist[v] = nd;
                }
            }
        }

        let mut order: Vec<(f64, String)> = Vec::with_capacity(reachable.len());
        for &u in &reachable {
            let u = u as usize;
            if in_topological_order[u] {
                order.push((dist[u], self.names[u].clone()));
            }
        }
        order.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });

        let mut result: Vec<String> = order.into_iter().map(|(_, name)| name).collect();

        // Append nodes reachable only through cycles in a deterministic order.
        let mut cyclic: Vec<String> = reachable
            .iter()
            .filter(|&&u| !in_topological_order[u as usize])
            .map(|&u| self.names[u as usize].clone())
            .collect();
        cyclic.sort();
        result.extend(cyclic);
        result
    }

    /// Tree-shake — mark modules not reachable from entry as dead.
    pub fn tree_shake(&self, entry: &str) -> (HashSet<String>, HashSet<String>) {
        let src_id = match self.name_to_id.get(entry) {
            Some(&id) => id,
            None => {
                let all: HashSet<String> = self.names.iter().cloned().collect();
                return (HashSet::new(), all);
            }
        };
        let live_ids = self.reachable_module_ids(src_id);
        let live: HashSet<String> = live_ids
            .iter()
            .map(|&id| self.names[id as usize].clone())
            .collect();
        let dead: HashSet<String> = self
            .names
            .iter()
            .filter(|n| !live.contains(n.as_str()))
            .cloned()
            .collect();
        (live, dead)
    }

    /// Condense graph to minimal transitive closure.
    /// Removes edges that are redundant (path via intermediate nodes has same
    /// or better cost).
    pub fn condense(&mut self) {
        let n = self.names.len();
        if n == 0 {
            return;
        }

        let scc_id = self.kosaraju_scc();
        let scc_count = *scc_id.iter().max().unwrap_or(&0) as usize + 1;
        let mut scc_size = vec![0usize; scc_count];
        for &id in &scc_id {
            scc_size[id as usize] += 1;
        }

        // Build the condensation DAG with max-plus combined edge costs.
        let mut dag: Vec<Vec<(usize, f64)>> = vec![Vec::new(); scc_count];
        let mut dag_edge_set: Vec<HashSet<usize>> = vec![HashSet::new(); scc_count];
        for u in 0..n {
            let su = scc_id[u] as usize;
            for (v, edge) in &self.outgoing[u] {
                let sv = scc_id[*v as usize] as usize;
                if su == sv {
                    continue;
                }
                if dag_edge_set[su].insert(sv) {
                    dag[su].push((sv, edge.cost));
                } else if let Some(existing) = dag[su].iter_mut().find(|(t, _)| *t == sv) {
                    existing.1 = existing.1.max(edge.cost);
                }
            }
        }

        let dag_topo = topological_sort(&dag);

        // Identify redundant original edges.
        let mut redundant: HashSet<(ModuleId, ModuleId)> = HashSet::new();
        for u in 0..n {
            let su = scc_id[u] as usize;
            for (v, edge) in &self.outgoing[u] {
                let sv = scc_id[*v as usize] as usize;
                if su == sv {
                    if scc_size[su] > 1 {
                        redundant.insert((u as ModuleId, *v));
                    }
                    continue;
                }
                if has_alternative_path(&dag, &dag_topo, su, sv, edge.cost) {
                    redundant.insert((u as ModuleId, *v));
                }
            }
        }

        // Filter adjacency lists in place and keep the dense matrix in sync.
        for u in 0..n {
            self.outgoing[u].retain(|(v, _)| !redundant.contains(&(u as ModuleId, *v)));
        }
        for v in 0..n {
            self.incoming[v].retain(|(u, _)| !redundant.contains(&(*u, v as ModuleId)));
        }
        for (u, v) in &redundant {
            self.mat[*u as usize][*v as usize] = TropicalEdge::UNREACHABLE;
        }
    }

    fn reachable_module_ids(&self, src: ModuleId) -> HashSet<ModuleId> {
        let mut seen = HashSet::new();
        let mut stack = vec![src];
        while let Some(u) = stack.pop() {
            if !seen.insert(u) {
                continue;
            }
            for (v, _) in &self.outgoing[u as usize] {
                if !seen.contains(v) {
                    stack.push(*v);
                }
            }
        }
        seen
    }

    fn kosaraju_scc(&self) -> Vec<ModuleId> {
        let n = self.names.len();
        let mut visited = vec![false; n];
        let mut order = Vec::with_capacity(n);

        // First pass: order by finishing times on the original graph.
        for start in 0..n {
            if visited[start] {
                continue;
            }
            let mut stack = vec![(start, false)];
            while let Some((u, processed)) = stack.pop() {
                if processed {
                    order.push(u);
                    continue;
                }
                if visited[u] {
                    continue;
                }
                visited[u] = true;
                stack.push((u, true));
                for (v, _) in &self.outgoing[u] {
                    let v = *v as usize;
                    if !visited[v] {
                        stack.push((v, false));
                    }
                }
            }
        }

        // Second pass: assign components on the reversed graph.
        let mut comp = vec![ModuleId::MAX; n];
        let mut label: ModuleId = 0;
        for &start in order.iter().rev() {
            if comp[start] != ModuleId::MAX {
                continue;
            }
            let mut stack = vec![start];
            while let Some(u) = stack.pop() {
                if comp[u] != ModuleId::MAX {
                    continue;
                }
                comp[u] = label;
                for (pred, _) in &self.incoming[u] {
                    let p = *pred as usize;
                    if comp[p] == ModuleId::MAX {
                        stack.push(p);
                    }
                }
            }
            label += 1;
        }
        comp
    }
}

fn parse_max_nodes_cap() -> usize {
    const DEFAULT_CAP: usize = 16_384;
    match env::var("JHOL_TROPICAL_MAX_NODES") {
        Ok(s) => s.trim().parse::<usize>().unwrap_or(DEFAULT_CAP).max(16),
        Err(_) => DEFAULT_CAP,
    }
}

fn has_alternative_path(
    dag: &[Vec<(usize, f64)>],
    topo: &[usize],
    source: usize,
    target: usize,
    direct_cost: f64,
) -> bool {
    if source == target {
        return false;
    }
    let src_pos = topo.iter().position(|&x| x == source).unwrap_or(0);
    let mut dist = vec![T_INF; dag.len()];
    dist[source] = 0.0;
    for &u in &topo[src_pos..] {
        if !dist[u].is_finite() {
            continue;
        }
        for (v, w) in &dag[u] {
            // Skip the direct edge whose redundancy we are checking.
            if u == source && *v == target {
                continue;
            }
            let nd = dist[u] + *w;
            if nd > dist[*v] {
                dist[*v] = nd;
            }
        }
    }
    dist[target] >= direct_cost
}

impl std::fmt::Debug for TropicalModuleGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut deps = Vec::new();
        for (from, outs) in self.outgoing.iter().enumerate() {
            for (to, edge) in outs {
                deps.push(format!(
                    "{} -> {} (cost={})",
                    self.names[from], self.names[*to as usize], edge.cost
                ));
            }
        }
        write!(
            f,
            "TropicalModuleGraph {{ nodes: {}, edges: [{}] }}",
            self.names.len(),
            deps.join(", ")
        )
    }
}

fn topological_sort(graph: &[Vec<(usize, f64)>]) -> Vec<usize> {
    let n = graph.len();
    let mut indeg = vec![0usize; n];
    for neighbors in graph {
        for (v, _) in neighbors {
            indeg[*v] += 1;
        }
    }
    let mut queue = VecDeque::new();
    for (u, &deg) in indeg.iter().enumerate() {
        if deg == 0 {
            queue.push_back(u);
        }
    }
    let mut order = Vec::with_capacity(n);
    while let Some(u) = queue.pop_front() {
        order.push(u);
        for (v, _) in &graph[u] {
            indeg[*v] = indeg[*v].saturating_sub(1);
            if indeg[*v] == 0 {
                queue.push_back(*v);
            }
        }
    }
    order
}

#[cfg(test)]
#[path = "tropical_graph_tests.rs"]
mod tropical_graph_tests;
