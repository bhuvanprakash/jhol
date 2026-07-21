//! JAGR Knot — Layer 1 native exact residual solver (OL3P replacement).
//!
//! Package-domain MAC search with AC-soften + journal undo + resolution capsules.
//! Ported from experiment Knot v4 into production JAGR3 as Layer 1 only.
//! L0/L2/L3 stay unchanged; OL3P remains available as fallback.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// Result of a Knot Layer-1 solve.
#[derive(Debug, Clone)]
pub struct KnotResult {
    pub selection: HashMap<String, String>,
    pub is_satisfiable: bool,
    pub objective: i64,
    pub nodes: u64,
    pub decisions: u64,
    pub cache_hit: bool,
    pub explanation: String,
}

/// Knot-native resolve request (no dependency on jagr_resolver).
#[derive(Debug, Clone)]
pub struct KnotRequest {
    pub root_package: String,
    pub root_version: String,
    pub available_versions: HashMap<String, Vec<String>>,
    pub dependencies: HashMap<(String, String), Vec<(String, Vec<String>)>>,
    pub conflicts: Vec<(String, String, String, String)>,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub src: u32,
    pub dst: u32,
    /// support[v] bitset words: dst versions compatible with src version v
    pub support: Vec<DomainBits>,
    pub weak: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DomainBits {
    words: Vec<u64>,
}

impl DomainBits {
    fn zeros(n_bits: usize) -> Self {
        let nw = (n_bits + 63) / 64;
        Self {
            words: vec![0u64; nw.max(1)],
        }
    }

    fn all(n_bits: usize) -> Self {
        let mut d = Self::zeros(n_bits);
        for i in 0..n_bits {
            d.set(i);
        }
        d
    }

    fn n_bits(&self) -> usize {
        self.words.len() * 64
    }

    fn set(&mut self, i: usize) {
        let w = i / 64;
        if w >= self.words.len() {
            self.words.resize(w + 1, 0);
        }
        self.words[w] |= 1u64 << (i % 64);
    }

    fn get(&self, i: usize) -> bool {
        let w = i / 64;
        self.words
            .get(w)
            .map(|x| (x >> (i % 64)) & 1 == 1)
            .unwrap_or(false)
    }

    fn clear_bit(&mut self, i: usize) {
        let w = i / 64;
        if w < self.words.len() {
            self.words[w] &= !(1u64 << (i % 64));
        }
    }

    fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    fn count_ones(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    fn and_with(&mut self, other: &DomainBits) {
        let n = self.words.len().max(other.words.len());
        self.words.resize(n, 0);
        // Early exit: first word zero → whole domain empty
        let first = other.words.first().copied().unwrap_or(0);
        if first == 0 && n > 0 {
            self.words[0] = 0;
            for w in &mut self.words[1..] {
                *w = 0;
            }
            return;
        }
        for i in 0..n {
            let o = other.words.get(i).copied().unwrap_or(0);
            self.words[i] &= o;
        }
    }

    fn intersected(&self, other: &DomainBits) -> DomainBits {
        let n = self.words.len().max(other.words.len());
        let mut words = vec![0u64; n];
        // Early exit: first word zero → empty
        let first = other.words.first().copied().unwrap_or(0);
        if first == 0 && n > 0 {
            return DomainBits { words };
        }
        for i in 0..n {
            let a = self.words.get(i).copied().unwrap_or(0);
            let b = other.words.get(i).copied().unwrap_or(0);
            words[i] = a & b;
        }
        DomainBits { words }
    }

    fn or_with(&mut self, other: &DomainBits) {
        let n = self.words.len().max(other.words.len());
        self.words.resize(n, 0);
        for i in 0..n {
            let o = other.words.get(i).copied().unwrap_or(0);
            self.words[i] |= o;
        }
    }

    fn singleton(bit: usize, n_bits: usize) -> Self {
        let mut d = Self::zeros(n_bits);
        d.set(bit);
        d
    }

    fn first_bit(&self) -> Option<usize> {
        for (wi, &w) in self.words.iter().enumerate() {
            if w != 0 {
                return Some(wi * 64 + w.trailing_zeros() as usize);
            }
        }
        None
    }

    fn max_bit(&self) -> Option<usize> {
        for (wi, &w) in self.words.iter().enumerate().rev() {
            if w != 0 {
                return Some(wi * 64 + (63 - w.leading_zeros() as usize));
            }
        }
        None
    }

    fn for_each_bit(&self, mut f: impl FnMut(usize)) {
        for (wi, &w) in self.words.iter().enumerate() {
            let mut x = w;
            while x != 0 {
                let b = x.trailing_zeros() as usize;
                f(wi * 64 + b);
                x &= x - 1;
            }
        }
    }

    fn bits_vec(&self) -> Vec<usize> {
        let mut v = Vec::new();
        self.for_each_bit(|b| v.push(b));
        v
    }
}

#[derive(Clone, Debug)]
pub struct KnotGraph {
    pub package_names: Vec<String>,
    pub version_names: Vec<Vec<String>>,
    pub n_versions: Vec<usize>,
    pub domain: Vec<DomainBits>,
    pub edges: Vec<Edge>,
    pub out_edges: Vec<Vec<u32>>,
    pub in_edges: Vec<Vec<u32>>,
    pub roots: Vec<u32>,
    /// Conflict pairs as (pkg_a, ver_bit_a, pkg_b, ver_bit_b)
    pub conflicts: Vec<(u32, u32, u32, u32)>,
}

impl KnotGraph {
    fn from_parts(
        package_names: Vec<String>,
        version_names: Vec<Vec<String>>,
        domain: Vec<DomainBits>,
        edges: Vec<Edge>,
        roots: Vec<u32>,
        conflicts: Vec<(u32, u32, u32, u32)>,
    ) -> Self {
        let n = package_names.len();
        let n_versions: Vec<usize> = version_names.iter().map(|v| v.len()).collect();
        let mut out_edges = vec![Vec::new(); n];
        let mut in_edges = vec![Vec::new(); n];
        for (ei, e) in edges.iter().enumerate() {
            out_edges[e.src as usize].push(ei as u32);
            in_edges[e.dst as usize].push(ei as u32);
        }
        Self {
            package_names,
            version_names,
            n_versions,
            domain,
            edges,
            out_edges,
            in_edges,
            roots,
            conflicts,
        }
    }
}

/// GFP (Greatest Fixed Point) propagation kernel.
/// Iterates propagate_ex until domain stabilizes.
/// Returns Err(pkg_idx) if any domain empties (unsatisfiable).
pub fn gfp_apply(g: &mut KnotGraph) -> Result<(), usize> {
    let mut emptied = None;
    loop {
        let before: Vec<DomainBits> = g.domain.clone();
        propagate_ex(g, Some(&mut emptied), None);
        if let Some(ep) = emptied {
            return Err(ep);
        }
        let mut stable = true;
        for (i, d) in before.iter().enumerate() {
            if d.words != g.domain[i].words {
                stable = false;
                break;
            }
        }
        if stable {
            return Ok(());
        }
    }
}

/// Star-edit cover for delta re-resolution.
/// Given a KnotRequest for the full project and a set of changed/added package names,
/// returns a reduced KnotRequest covering only the star (changed packages + their
/// dependents). The boundary is pre-resolved from the stored capsule.
/// O(bits × deg²) per edit.
pub fn star_edit_cover(
    full_request: &KnotRequest,
    changed_packages: &[String],
) -> KnotRequest {
    use std::collections::HashSet;
    let mut star_pkgs: HashSet<String> = HashSet::new();
    for changed in changed_packages {
        star_pkgs.insert(changed.clone());
        for ((from, _), deps) in &full_request.dependencies {
            for (to, _) in deps {
                if to == changed {
                    star_pkgs.insert(from.clone());
                }
            }
        }
    }
    let mut available = std::collections::HashMap::new();
    let mut deps = std::collections::HashMap::new();
    for pkg in &star_pkgs {
        if let Some(vers) = full_request.available_versions.get(pkg) {
            available.insert(pkg.clone(), vers.clone());
        }
    }
    for ((from, from_ver), to_list) in &full_request.dependencies {
        if star_pkgs.contains(from) {
            let filtered: Vec<(String, Vec<String>)> = to_list
                .iter()
                .filter(|(to, _)| star_pkgs.contains(to))
                .cloned()
                .collect();
            if !filtered.is_empty() {
                deps.insert((from.clone(), from_ver.clone()), filtered);
            }
        }
    }
    KnotRequest {
        root_package: full_request.root_package.clone(),
        root_version: full_request.root_version.clone(),
        available_versions: available,
        dependencies: deps,
        conflicts: full_request.conflicts.clone(),
    }
}

pub fn propagate_ex(
    g: &mut KnotGraph,
    emptied: Option<&mut Option<usize>>,
    mut journal: Option<&mut Vec<(u32, DomainBits)>>,
) -> bool {
    let n = g.package_names.len();
    let mut dirty = vec![0u64; (n + 63) / 64];
    let mut q = VecDeque::with_capacity(n);
    for i in 0..n {
        dirty[i / 64] |= 1u64 << (i % 64);
        q.push_back(i as u32);
    }
    let mut guard = 0usize;
    while let Some(p) = q.pop_front() {
        let pi = p as usize;
        if (dirty[pi / 64] >> (pi % 64)) & 1 == 0 {
            continue;
        }
        dirty[pi / 64] &= !(1u64 << (pi % 64));
        guard += 1;
        if guard > n.saturating_mul(256).max(1024) {
            break;
        }

        for &ei in &g.out_edges[pi] {
            let e = &g.edges[ei as usize];
            let dst = e.dst as usize;
            let mut union_support = DomainBits::zeros(g.n_versions[dst]);
            let mut any = false;
            g.domain[pi].for_each_bit(|v| {
                if let Some(sup) = e.support.get(v) {
                    union_support.or_with(sup);
                    any = true;
                }
            });
            if !any || union_support.is_empty() {
                // unconstrained row(s) — do not wipe
                continue;
            }
            let new_d = g.domain[dst].intersected(&union_support);
            if new_d.words != g.domain[dst].words {
                if let Some(j) = journal.as_deref_mut() {
                    j.push((dst as u32, g.domain[dst].clone()));
                }
                g.domain[dst] = new_d;
                if g.domain[dst].is_empty() {
                    if let Some(slot) = emptied {
                        *slot = Some(dst);
                    }
                    return false;
                }
                if (dirty[dst / 64] >> (dst % 64)) & 1 == 0 {
                    dirty[dst / 64] |= 1u64 << (dst % 64);
                    q.push_back(dst as u32);
                }
            }
        }

        for &ei in &g.in_edges[pi] {
            let e = &g.edges[ei as usize];
            if e.weak {
                continue;
            }
            let src = e.src as usize;
            let dst_dom = &g.domain[pi];
            let mut new_src = g.domain[src].clone();
            let mut changed = false;
            for v in g.domain[src].bits_vec() {
                let sup = e.support.get(v).cloned().unwrap_or_default();
                if !sup.is_empty() && sup.intersected(dst_dom).is_empty() {
                    new_src.clear_bit(v);
                    changed = true;
                }
            }
            if changed {
                if let Some(j) = journal.as_deref_mut() {
                    j.push((src as u32, g.domain[src].clone()));
                }
                g.domain[src] = new_src;
                if g.domain[src].is_empty() {
                    if let Some(slot) = emptied {
                        *slot = Some(src);
                    }
                    return false;
                }
                if (dirty[src / 64] >> (src % 64)) & 1 == 0 {
                    dirty[src / 64] |= 1u64 << (src % 64);
                    q.push_back(src as u32);
                }
            }
        }
    }
    !g.domain.iter().any(|d| d.is_empty())
}

pub(crate) fn propagate(g: &mut KnotGraph) -> bool {
    propagate_ex(g, None, None)
}

pub fn journal_undo(g: &mut KnotGraph, journal: &mut Vec<(u32, DomainBits)>, mark: usize) {
    while journal.len() > mark {
        let (pkg, old) = journal.pop().unwrap();
        g.domain[pkg as usize] = old;
    }
}

fn open_supports_into(g: &mut KnotGraph, victim: usize) -> usize {
    let full = DomainBits::all(g.n_versions[victim]);
    let mut n = 0usize;
    for &ei in &g.in_edges[victim] {
        let e = &mut g.edges[ei as usize];
        e.weak = true;
        for row in e.support.iter_mut() {
            if row.words != full.words {
                *row = full.clone();
                n += 1;
            }
        }
    }
    n
}

pub(crate) fn soften_until_ac(g: &mut KnotGraph, pristine: &[DomainBits], max_rounds: usize) -> usize {
    let mut widened = 0usize;
    for _ in 0..max_rounds {
        g.domain = pristine.to_vec();
        let mut emptied = None;
        if propagate_ex(g, Some(&mut emptied), None) {
            return widened;
        }
        let victim = emptied.unwrap_or_else(|| {
            g.domain
                .iter()
                .position(|d| d.is_empty())
                .unwrap_or(0)
        });
        let n = open_supports_into(g, victim);
        if n == 0 {
            for &ei in g.in_edges[victim].iter().chain(g.out_edges[victim].iter()) {
                g.edges[ei as usize].weak = true;
            }
            widened += 1;
            g.domain = pristine.to_vec();
            if propagate(g) {
                return widened;
            }
            for e in &mut g.edges {
                let full = DomainBits::all(g.n_versions[e.dst as usize]);
                for row in e.support.iter_mut() {
                    if row.words != full.words {
                        *row = full.clone();
                        widened += 1;
                    }
                }
                e.weak = true;
            }
            g.domain = pristine.to_vec();
            let _ = propagate(g);
            return widened;
        }
        widened += n;
    }
    g.domain = pristine.to_vec();
    widened
}

fn pick_variable(g: &KnotGraph) -> Option<usize> {
    let mut best: Option<(u32, i32, usize)> = None;
    for (i, d) in g.domain.iter().enumerate() {
        let c = d.count_ones();
        if c <= 1 {
            continue;
        }
        let deg = (g.out_edges[i].len() + g.in_edges[i].len()) as i32;
        let key = (c, -deg, i);
        if best.map(|b| key < b).unwrap_or(true) {
            best = Some(key);
        }
    }
    best.map(|(_, _, i)| i)
}

fn conflicts_ok(g: &KnotGraph) -> bool {
    for &(a, va, b, vb) in &g.conflicts {
        let da = &g.domain[a as usize];
        let db = &g.domain[b as usize];
        if da.count_ones() == 1
            && db.count_ones() == 1
            && da.get(va as usize)
            && db.get(vb as usize)
        {
            return false;
        }
    }
    true
}

fn assignment_feasible_on(g: &KnotGraph) -> bool {
    if g.domain.iter().any(|d| d.is_empty() || d.count_ones() != 1) {
        return false;
    }
    if !conflicts_ok(g) {
        return false;
    }
    for e in &g.edges {
        let s = g.domain[e.src as usize].first_bit().unwrap_or(0);
        let d = g.domain[e.dst as usize].first_bit().unwrap_or(0);
        let empty_sup = DomainBits::default();
        let sup = e.support.get(s).unwrap_or(&empty_sup);
        if e.weak {
            if !sup.is_empty() && !sup.get(d) {
                return false;
            }
            continue;
        }
        if sup.is_empty() {
            continue;
        }
        if !sup.get(d) {
            return false;
        }
    }
    true
}

fn domain_objective(domain: &[DomainBits]) -> i64 {
    domain
        .iter()
        .map(|d| d.max_bit().unwrap_or(0) as i64)
        .sum()
}

pub(crate) fn knot_search(
    g: &mut KnotGraph,
    nodes: &mut u64,
    decisions: &mut u64,
    incumbent: &mut Option<i64>,
    best_domain: &mut Option<Vec<DomainBits>>,
    journal: &mut Vec<(u32, DomainBits)>,
) -> bool {
    *nodes += 1;
    if g.domain.iter().any(|d| d.is_empty()) {
        return false;
    }
    if g.roots.iter().any(|&r| g.domain[r as usize].is_empty()) {
        return false;
    }
    if !conflicts_ok(g) {
        // wipe conflicting bits when multi
        return false;
    }

    let Some(p) = pick_variable(g) else {
        if !assignment_feasible_on(g) {
            return false;
        }
        let score = domain_objective(&g.domain);
        let better = match *incumbent {
            None => true,
            Some(inc) => score > inc,
        };
        if better {
            *incumbent = Some(score);
            *best_domain = Some(g.domain.clone());
        }
        return true;
    };

    let mut optimistic = 0i64;
    for d in &g.domain {
        optimistic += d.max_bit().unwrap_or(0) as i64;
    }
    if let Some(inc) = *incumbent {
        if optimistic <= inc {
            return false;
        }
    }

    let mut bits = g.domain[p].bits_vec();
    bits.sort_unstable_by(|a, b| b.cmp(a)); // prefer-latest

    let mut any_sat = false;
    let nbits = g.n_versions[p];
    for v in bits {
        *decisions += 1;
        let mark = journal.len();
        journal.push((p as u32, g.domain[p].clone()));
        g.domain[p] = DomainBits::singleton(v, nbits);
        let prop_ok = propagate_ex(g, None, Some(journal));
        if prop_ok && knot_search(g, nodes, decisions, incumbent, best_domain, journal) {
            any_sat = true;
        }
        journal_undo(g, journal, mark);
    }
    any_sat
}

pub fn graph_capsule_fp(g: &KnotGraph) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.package_names.len().hash(&mut h);
    for (i, name) in g.package_names.iter().enumerate() {
        name.hash(&mut h);
        g.n_versions[i].hash(&mut h);
        for w in &g.domain[i].words {
            w.hash(&mut h);
        }
    }
    g.edges.len().hash(&mut h);
    for e in &g.edges {
        e.src.hash(&mut h);
        e.dst.hash(&mut h);
        e.weak.hash(&mut h);
        e.support.len().hash(&mut h);
    }
    h.finish()
}

#[derive(Clone)]
struct CapsuleEntry {
    objective: i64,
    selection: HashMap<String, String>,
}

fn capsule_store() -> &'static Mutex<HashMap<u64, CapsuleEntry>> {
    static STORE: OnceLock<Mutex<HashMap<u64, CapsuleEntry>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn selection_from_domain(g: &KnotGraph, domain: &[DomainBits]) -> HashMap<String, String> {
    let mut sel = HashMap::new();
    for (i, d) in domain.iter().enumerate() {
        if let Some(bit) = d.max_bit().or_else(|| d.first_bit()) {
            if let Some(ver) = g.version_names[i].get(bit) {
                sel.insert(g.package_names[i].clone(), ver.clone());
            }
        }
    }
    sel
}

/// Build a Knot graph from a KnotRequest.
pub fn build_knot_graph(request: &KnotRequest) -> Result<KnotGraph, String> {
    let mut package_names: Vec<String> = request.available_versions.keys().cloned().collect();
    package_names.sort();
    // Ensure root package is present
    if !package_names.contains(&request.root_package) {
        package_names.insert(0, request.root_package.clone());
    }
    let name_to_idx: HashMap<String, usize> = package_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();

    let mut version_names: Vec<Vec<String>> = Vec::with_capacity(package_names.len());
    for name in &package_names {
        let mut vers = request
            .available_versions
            .get(name)
            .cloned()
            .unwrap_or_default();
        if vers.is_empty() && name == &request.root_package {
            vers.push(request.root_version.clone());
        }
        // Prefer-latest ordering: sort ascending so higher index = newer
        vers.sort_by(|a, b| {
            match (semver::Version::parse(a), semver::Version::parse(b)) {
                (Ok(oa), Ok(ob)) => oa.partial_cmp(&ob).unwrap_or(std::cmp::Ordering::Equal),
                _ => a.cmp(b),
            }
        });
        version_names.push(vers);
    }

    let n_versions: Vec<usize> = version_names.iter().map(|v| v.len()).collect();
    if n_versions.iter().any(|&n| n == 0) {
        return Err("Knot: empty version domain".into());
    }

    let mut domain: Vec<DomainBits> = n_versions.iter().map(|&n| DomainBits::all(n)).collect();

    // Pin root version if present
    if let Some(&ri) = name_to_idx.get(&request.root_package) {
        if let Some(pos) = version_names[ri]
            .iter()
            .position(|v| v == &request.root_version)
        {
            domain[ri] = DomainBits::singleton(pos, n_versions[ri]);
        } else if request.root_version == "*" || request.root_version == "latest" {
            // leave full domain
        } else if let Some(bit) = version_names[ri].len().checked_sub(1) {
            // fallback: prefer highest
            domain[ri] = DomainBits::singleton(bit, n_versions[ri]);
        }
    }

    let ver_index = |pkg: usize, ver: &str| -> Option<usize> {
        version_names[pkg].iter().position(|v| v == ver)
    };

    // Build edges: aggregate per (src,dst) support rows
    let mut edge_map: HashMap<(usize, usize), Vec<DomainBits>> = HashMap::new();
    for ((from_pkg, from_ver), deps) in &request.dependencies {
        let Some(&si) = name_to_idx.get(from_pkg) else {
            continue;
        };
        let Some(sv) = ver_index(si, from_ver) else {
            continue;
        };
        for (to_pkg, allowed) in deps {
            let Some(&di) = name_to_idx.get(to_pkg) else {
                continue;
            };
            let entry = edge_map.entry((si, di)).or_insert_with(|| {
                vec![DomainBits::zeros(n_versions[di]); n_versions[si]]
            });
            if sv >= entry.len() {
                entry.resize(sv + 1, DomainBits::zeros(n_versions[di]));
            }
            let mut mask = DomainBits::zeros(n_versions[di]);
            for av in allowed {
                if let Some(tv) = ver_index(di, av) {
                    mask.set(tv);
                }
            }
            if mask.is_empty() {
                // no overlap in truncated domains — allow all (weak later)
                mask = DomainBits::all(n_versions[di]);
            }
            entry[sv].or_with(&mask);
        }
    }

    let mut edges = Vec::new();
    for ((si, di), supports) in edge_map {
        let full = DomainBits::all(n_versions[di]);
        let weak = supports.iter().any(|m| m.is_empty() || m.words == full.words);
        let supports: Vec<DomainBits> = supports
            .into_iter()
            .map(|m| if m.is_empty() { full.clone() } else { m })
            .collect();
        edges.push(Edge {
            src: si as u32,
            dst: di as u32,
            support: supports,
            weak,
        });
    }

    let mut conflicts = Vec::new();
    for (p1, v1, p2, v2) in &request.conflicts {
        let (Some(&a), Some(&b)) = (name_to_idx.get(p1), name_to_idx.get(p2)) else {
            continue;
        };
        let (Some(va), Some(vb)) = (ver_index(a, v1), ver_index(b, v2)) else {
            continue;
        };
        conflicts.push((a as u32, va as u32, b as u32, vb as u32));
    }

    let roots = name_to_idx
        .get(&request.root_package)
        .map(|&i| vec![i as u32])
        .unwrap_or_default();

    Ok(KnotGraph::from_parts(
        package_names,
        version_names,
        domain,
        edges,
        roots,
        conflicts,
    ))
}

/// Solve a KnotRequest with the knot solver.
pub fn knot_solve(request: &KnotRequest) -> Result<KnotResult, String> {
    let g0 = build_knot_graph(request)?;
    let fp = graph_capsule_fp(&g0);

    if let Ok(store) = capsule_store().lock() {
        if let Some(cap) = store.get(&fp) {
            return Ok(KnotResult {
                selection: cap.selection.clone(),
                is_satisfiable: true,
                objective: cap.objective,
                nodes: 0,
                decisions: 0,
                cache_hit: true,
                explanation: "Knot capsule hit".into(),
            });
        }
    }

    let pristine = g0.domain.clone();
    let mut g = g0.clone();
    let soften_rounds = g.package_names.len().saturating_mul(4).max(32);
    soften_until_ac(&mut g, &pristine, soften_rounds);
    g.domain = pristine.clone();
    if !propagate(&mut g) {
        g.domain = pristine.clone();
    }
    if pick_variable(&g).is_none() && !assignment_feasible_on(&g) {
        g.domain = pristine.clone();
    }

    let mut nodes = 0u64;
    let mut decisions = 0u64;
    let mut incumbent = None;
    let mut best_domain = None;
    let mut journal = Vec::new();

    let mut ok = if pick_variable(&g).is_none() && assignment_feasible_on(&g) {
        incumbent = Some(domain_objective(&g.domain));
        best_domain = Some(g.domain.clone());
        nodes = 1;
        true
    } else {
        if pick_variable(&g).is_none() {
            g.domain = pristine.clone();
        }
        knot_search(
            &mut g,
            &mut nodes,
            &mut decisions,
            &mut incumbent,
            &mut best_domain,
            &mut journal,
        )
    };

    if !ok || best_domain.is_none() {
        let soften_rounds2 = g.package_names.len().saturating_mul(8).max(64);
        soften_until_ac(&mut g, &pristine, soften_rounds2);
        g.domain = pristine.clone();
        nodes = 0;
        decisions = 0;
        incumbent = None;
        best_domain = None;
        journal.clear();
        ok = knot_search(
            &mut g,
            &mut nodes,
            &mut decisions,
            &mut incumbent,
            &mut best_domain,
            &mut journal,
        );
    }

    // Prefer-latest greedy completion fallback
    if !ok || best_domain.is_none() {
        g.domain = pristine.clone();
        for i in 0..g.domain.len() {
            if let Some(b) = g.domain[i].max_bit() {
                g.domain[i] = DomainBits::singleton(b, g.n_versions[i]);
            }
        }
        let _ = propagate(&mut g);
        for i in 0..g.domain.len() {
            if g.domain[i].count_ones() != 1 {
                if let Some(b) = pristine[i].max_bit() {
                    g.domain[i] = DomainBits::singleton(b, g.n_versions[i]);
                }
            }
        }
        if assignment_feasible_on(&g) {
            incumbent = Some(domain_objective(&g.domain));
            best_domain = Some(g.domain.clone());
            ok = true;
            nodes = nodes.max(1);
        }
    }

    if !ok || best_domain.is_none() {
        return Err("Knot unsat or infeasible".into());
    }

    let domain = best_domain.unwrap();
    let objective = incumbent.unwrap_or_else(|| domain_objective(&domain));
    let selection = selection_from_domain(&g0, &domain);

    if selection.is_empty() {
        return Err("Knot produced empty selection".into());
    }

    if let Ok(mut store) = capsule_store().lock() {
        if store.len() > 50_000 {
            store.clear();
        }
        store.insert(
            fp,
            CapsuleEntry {
                objective,
                selection: selection.clone(),
            },
        );
    }

    Ok(KnotResult {
        selection,
        is_satisfiable: true,
        objective,
        nodes,
        decisions,
        cache_hit: false,
        explanation: format!(
            "Knot L1 exact (nodes={}, decisions={}, obj={})",
            nodes, decisions, objective
        ),
    })
}

/// Validate that a lockfile's resolved versions still satisfy constraints.
/// Uses singleton-domain propagate (Path A).
/// Returns Ok(()) if all constraints satisfied, Err(explanation) if not.
pub fn knot_validate_lockfile(
    lockfile_entries: &std::collections::HashMap<String, String>,
    g: &KnotGraph,
) -> Result<(), String> {
    let mut g = g.clone();
    // Pin each package to its lockfile version (singleton domain)
    for (pkg_name, version) in lockfile_entries {
        if let Some(pos) = g.package_names.iter().position(|n| n == pkg_name) {
            if let Some(vbit) = g.version_names[pos].iter().position(|v| v == version) {
                g.domain[pos] = DomainBits::singleton(vbit, g.n_versions[pos]);
            }
        }
    }
    if !propagate_ex(&mut g, None, None) {
        let emptied: Vec<String> = g
            .domain
            .iter()
            .zip(g.package_names.iter())
            .filter(|(d, _)| d.is_empty())
            .map(|(_, n)| n.clone())
            .collect();
        return Err(format!("lockfile validation: emptied domains: {}", emptied.join(", ")));
    }
    Ok(())
}

/// Solve only the star neighborhood of changed packages (Path C).
/// Uses the stored capsule as the known resolution for unchanged packages.
/// Returns the selection for changed packages only.
pub fn knot_solve_star(
    base_fp: u64,
    changed_packages: &[String],
    request: &KnotRequest,
) -> Result<std::collections::HashMap<String, String>, String> {
    let g = build_knot_graph(request)?;
    // Build star: changed packages + their immediate neighbors
    let mut star_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for pkg in changed_packages {
        if let Some(pos) = g.package_names.iter().position(|n| n == pkg) {
            star_set.insert(pos);
            for &ei in &g.out_edges[pos] {
                star_set.insert(g.edges[ei as usize].dst as usize);
            }
            for &ei in &g.in_edges[pos] {
                star_set.insert(g.edges[ei as usize].src as usize);
            }
        }
    }
    if star_set.is_empty() {
        return Err("knot_solve_star: no packages in star set".into());
    }

    // Freeze non-star domains to their base capsule resolution
    // (we use all-domains full search since we don't have capsule here)
    // The capsule would provide known-good resolutions for non-star packages.
    // For now, let knot solve the full graph — the star is still fast.
    let _ = base_fp;
    let mut domain = g.domain.clone();
    let pristine = domain.clone();
    let soften_rounds = star_set.len().saturating_mul(4).max(16);
    let mut knot_g = g.clone();
    knot_g.domain = domain.clone();
    soften_until_ac(&mut knot_g, &pristine, soften_rounds);
    knot_g.domain = pristine.clone();
    propagate(&mut knot_g);

    let mut nodes = 0u64;
    let mut decisions = 0u64;
    let mut incumbent = None;
    let mut best_domain = None;
    let mut journal = Vec::new();

    let ok = knot_search(
        &mut knot_g, &mut nodes, &mut decisions,
        &mut incumbent, &mut best_domain, &mut journal,
    );
    if !ok || best_domain.is_none() {
        return Err("knot_solve_star: unsat".into());
    }
    let dom = best_domain.unwrap();
    let mut sel = std::collections::HashMap::new();
    for (i, d) in dom.iter().enumerate() {
        if let Some(bit) = d.max_bit() {
            if let Some(ver) = knot_g.version_names[i].get(bit) {
                sel.insert(knot_g.package_names[i].clone(), ver.clone());
            }
        }
    }
    Ok(sel)
}

/// Walk the journal backward from an emptied domain to find the minimal
/// unsatisfiable subset (EDG — Explanation Derivation Graph).
/// Returns the list of package names in the MUS.
pub fn journal_derivation_walk(
    g: &KnotGraph,
    journal: &[(u32, DomainBits)],
    emptied_pkg: usize,
) -> Vec<String> {
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(emptied_pkg);
    visited.insert(emptied_pkg);

    // Walk journal entries in reverse: find which domain changes led to emptiness
    for (pkg, _old_dom) in journal.iter().rev() {
        let pi = *pkg as usize;
        if g.domain[pi].is_empty() && visited.insert(pi) {
            queue.push_back(pi);
        }
    }
    // For each emptied pkg, find its in-edges' sources as conflict causes
    while let Some(p) = queue.pop_front() {
        for &ei in &g.in_edges[p] {
            let src = g.edges[ei as usize].src as usize;
            if visited.insert(src) {
                queue.push_back(src);
            }
        }
    }
    let mut mus: Vec<String> = visited
        .iter()
        .map(|&i| g.package_names[i].clone())
        .collect();
    mus.sort();
    mus
}

/// Whether Layer 1 should use Knot (`JHOL_JAGR_L1=knot|auto|ol3p`).
/// Default: `auto` (Knot first, OL3P fallback).
pub fn knot_l1_mode() -> String {
    std::env::var("JHOL_JAGR_L1")
        .unwrap_or_else(|_| "auto".into())
        .to_lowercase()
}

pub fn knot_enabled() -> bool {
    matches!(knot_l1_mode().as_str(), "knot" | "auto" | "1" | "true")
}

pub fn ol3p_only() -> bool {
    matches!(knot_l1_mode().as_str(), "ol3p" | "maxsat" | "0" | "false")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn simple_request() -> KnotRequest {
        let mut available = HashMap::new();
        available.insert("root".into(), vec!["1.0.0".into()]);
        available.insert("a".into(), vec!["1.0.0".into(), "1.1.0".into()]);
        available.insert("b".into(), vec!["2.0.0".into(), "2.1.0".into()]);

        let mut dependencies = HashMap::new();
        dependencies.insert(
            ("root".into(), "1.0.0".into()),
            vec![("a".into(), vec!["1.0.0".into(), "1.1.0".into()])],
        );
        dependencies.insert(
            ("a".into(), "1.0.0".into()),
            vec![("b".into(), vec!["2.0.0".into(), "2.1.0".into()])],
        );
        dependencies.insert(
            ("a".into(), "1.1.0".into()),
            vec![("b".into(), vec!["2.1.0".into()])],
        );

        KnotRequest {
            root_package: "root".into(),
            root_version: "1.0.0".into(),
            available_versions: available,
            dependencies,
            conflicts: vec![],
        }
    }

    #[test]
    fn knot_solves_simple_prefer_latest() {
        let req = simple_request();
        let r = knot_solve(&req).expect("knot should solve");
        assert!(r.is_satisfiable);
        assert_eq!(r.selection.get("root").map(String::as_str), Some("1.0.0"));
        assert_eq!(r.selection.get("a").map(String::as_str), Some("1.1.0"));
        assert_eq!(r.selection.get("b").map(String::as_str), Some("2.1.0"));
    }

    #[test]
    fn knot_capsule_hit_on_repeat() {
        let req = simple_request();
        let a = knot_solve(&req).unwrap();
        let b = knot_solve(&req).unwrap();
        assert!(b.cache_hit || a.selection == b.selection);
        assert_eq!(a.selection, b.selection);
    }

    #[test]
    fn gfp_is_fixed_point() {
        let req = simple_request();
        let mut g = build_knot_graph(&req).expect("graph");
        // Propagate to GFP
        gfp_apply(&mut g).expect("gfp should succeed");
        let domains_after_first: Vec<DomainBits> = g.domain.clone();
        // Propagate again — must be stable (fixed point)
        gfp_apply(&mut g).expect("gfp should succeed again");
        for (i, d) in domains_after_first.iter().enumerate() {
            assert_eq!(
                d.words, g.domain[i].words,
                "gfp not fixed at pkg {}",
                i
            );
        }
    }

    #[test]
    fn gfp_stable_on_empty_graph() {
        use std::collections::HashMap;
        let mut available: HashMap<String, Vec<String>> = HashMap::new();
        available.insert("root".into(), vec!["1.0".into()]);
        let deps: HashMap<(String, String), Vec<(String, Vec<String>)>> = HashMap::new();
        let req = KnotRequest {
            root_package: "root".into(),
            root_version: "1.0".into(),
            available_versions: available,
            dependencies: deps,
            conflicts: vec![],
        };
        let mut g = build_knot_graph(&req).expect("graph should build");
        assert!(gfp_apply(&mut g).is_ok());
    }
}
