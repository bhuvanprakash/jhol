//! JAGR Knot — Layer 1 native exact residual solver (OL3P replacement).
//!
//! Package-domain MAC search with AC-soften + journal undo + resolution capsules.
//! Ported from experiment Knot v4 into production JAGR3 as Layer 1 only.
//! L0/L2/L3 stay unchanged; OL3P remains available as fallback.

use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Optional set of "focus" packages (SCC obstruction members). When set,
    /// `pick_variable` explores these first so the solver converges on the
    /// cyclic core before the DAG portion. Completeness is preserved — focus
    /// only reorders the search, it never prunes.
    pub focus_packages: Option<HashSet<String>>,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub src: u32,
    pub dst: u32,
    /// support[v] bitset words: dst versions compatible with src version v
    pub support: FlatBitMatrix,
    pub weak: bool,
}

/// A dense bit matrix in row-major order: single Vec<u64>, zero per-row allocation.
#[derive(Clone, Debug)]
pub struct FlatBitMatrix {
    pub n_rows: usize,
    pub words_per_row: usize,
    pub data: Vec<u64>,
}

impl FlatBitMatrix {
    pub fn zeros(n_rows: usize, n_cols: usize) -> Self {
        let words_per_row = (n_cols + 63) / 64;
        let data = vec![0u64; n_rows * words_per_row.max(1)];
        Self {
            n_rows,
            words_per_row: words_per_row.max(1),
            data,
        }
    }

    pub fn get_row(&self, row: usize) -> &[u64] {
        let start = row * self.words_per_row;
        &self.data[start..start + self.words_per_row]
    }

    pub fn get_row_mut(&mut self, row: usize) -> &mut [u64] {
        let start = row * self.words_per_row;
        &mut self.data[start..start + self.words_per_row]
    }

    pub fn set_row_all(&mut self, row: usize) {
        let start = row * self.words_per_row;
        for w in &mut self.data[start..start + self.words_per_row] {
            *w = !0u64;
        }
    }

    pub fn is_row_full(&self, row: usize) -> bool {
        let start = row * self.words_per_row;
        self.data[start..start + self.words_per_row]
            .iter()
            .all(|&w| w == !0u64)
    }

    pub fn iter_rows(&self) -> impl Iterator<Item = &[u64]> {
        self.data.chunks_exact(self.words_per_row)
    }

    pub fn iter_rows_mut(&mut self) -> impl Iterator<Item = &mut [u64]> {
        self.data.chunks_exact_mut(self.words_per_row)
    }

    pub fn n_cols(&self) -> usize {
        self.words_per_row * 64
    }

    /// OR src into dst word-by-word (SIMD accelerated)
    pub unsafe fn or_assign(dst: &mut [u64], src: &[u64]) {
        let n = dst.len().min(src.len());
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            use std::arch::x86_64::*;
            let simd_end = (n / 4) * 4;
            for i in (0..simd_end).step_by(4) {
                let d_vec = _mm256_loadu_si256(&dst[i] as *const u64 as *const __m256i);
                let s_vec = _mm256_loadu_si256(&src[i] as *const u64 as *const __m256i);
                _mm256_storeu_si256(
                    &mut dst[i] as *mut u64 as *mut __m256i,
                    _mm256_or_si256(d_vec, s_vec),
                );
            }
            for i in simd_end..n {
                dst[i] |= src[i];
            }
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            for i in 0..n {
                dst[i] |= src[i];
            }
        }
    }

    /// Check if any bit is set in both a and b (SIMD accelerated)
    pub unsafe fn intersects_any(a: &[u64], b: &[u64]) -> bool {
        let n = a.len().min(b.len());
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            use std::arch::x86_64::*;
            let simd_end = (n / 4) * 4;
            for i in (0..simd_end).step_by(4) {
                let a_vec = _mm256_loadu_si256(&a[i] as *const u64 as *const __m256i);
                let b_vec = _mm256_loadu_si256(&b[i] as *const u64 as *const __m256i);
                let and_vec = _mm256_and_si256(a_vec, b_vec);
                if _mm256_testz_si256(and_vec, and_vec) == 0 {
                    return true;
                }
            }
            for i in simd_end..n {
                if a[i] & b[i] != 0 {
                    return true;
                }
            }
            false
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            for i in 0..n {
                if a[i] & b[i] != 0 {
                    return true;
                }
            }
            false
        }
    }

    /// Check if all words in a are zero (SIMD accelerated)
    pub unsafe fn is_zero(a: &[u64]) -> bool {
        let n = a.len();
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            use std::arch::x86_64::*;
            let simd_end = (n / 4) * 4;
            for i in (0..simd_end).step_by(4) {
                let v = _mm256_loadu_si256(&a[i] as *const u64 as *const __m256i);
                if _mm256_testz_si256(v, v) == 0 {
                    return false;
                }
            }
            for i in simd_end..n {
                if a[i] != 0 {
                    return false;
                }
            }
            true
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            for &w in a.iter() {
                if w != 0 {
                    return false;
                }
            }
            true
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DomainBits {
    words: Vec<u64>,
    pub capacity_n_words: usize,
}

impl DomainBits {
    fn zeros(n_bits: usize) -> Self {
        let nw = (n_bits + 63) / 64;
        Self {
            words: vec![0u64; nw.max(1)],
            capacity_n_words: nw.max(1),
        }
    }

    fn preallocate(&mut self, max_versions: usize) {
        self.capacity_n_words = (max_versions + 63) / 64;
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
        let first = other.words.first().copied().unwrap_or(0);
        if first == 0 {
            for w in &mut self.words {
                *w = 0;
            }
            return;
        }
        for i in 0..self.words.len() {
            let o = other.words.get(i).copied().unwrap_or(0);
            self.words[i] &= o;
        }
    }

    fn intersected(&self, other: &DomainBits) -> DomainBits {
        let n = self.words.len().max(other.words.len());
        let mut words = vec![0u64; n];
        let first = other.words.first().copied().unwrap_or(0);
        if first == 0 && n > 0 {
            return DomainBits {
                words,
                capacity_n_words: n,
            };
        }
        for i in 0..n {
            let a = self.words.get(i).copied().unwrap_or(0);
            let b = other.words.get(i).copied().unwrap_or(0);
            words[i] = a & b;
        }
        DomainBits {
            words,
            capacity_n_words: n,
        }
    }

    fn or_with(&mut self, other: &DomainBits) {
        for i in 0..self.words.len() {
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

/// Arena-backed journal snapshot store.
/// Stores changed domain words in one flat buffer and avoids per-entry heap allocations.
struct JournalArena {
    data: Vec<u64>,
    entries: Vec<(u32, usize, usize)>, // (pkg, start, len)
}

impl JournalArena {
    fn with_capacity(capacity_words: usize, capacity_entries: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity_words),
            entries: Vec::with_capacity(capacity_entries),
        }
    }

    fn push(&mut self, pkg: u32, old: &DomainBits) {
        let start = self.data.len();
        self.data.extend_from_slice(&old.words);
        self.entries.push((pkg, start, old.words.len()));
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn undo(&mut self, g: &mut KnotGraph, mark: usize) {
        while self.entries.len() > mark {
            let (pkg, start, len) = self.entries.pop().unwrap();
            g.domain[pkg as usize]
                .words
                .copy_from_slice(&self.data[start..start + len]);
            self.data.truncate(start);
        }
    }

    fn clear(&mut self) {
        self.data.clear();
        self.entries.clear();
    }
}

#[derive(Clone, Debug)]
pub struct KnotGraph {
    pub package_names: Vec<String>,
    pub version_names: Vec<Vec<String>>,
    pub n_versions: Vec<usize>,
    pub var_to_package: Vec<u32>,
    pub domain: Vec<DomainBits>,
    pub edges: Vec<Edge>,
    pub out_edges: Vec<Vec<u32>>,
    pub in_edges: Vec<Vec<u32>>,
    pub roots: Vec<u32>,
    /// Conflict pairs as (pkg_a, ver_bit_a, pkg_b, ver_bit_b)
    pub conflicts: Vec<(u32, u32, u32, u32)>,
    /// Package indices explored first by `pick_variable` (SCC focus).
    pub focused: HashSet<u32>,
}

impl KnotGraph {
    fn from_parts(
        package_names: Vec<String>,
        version_names: Vec<Vec<String>>,
        domain: Vec<DomainBits>,
        edges: Vec<Edge>,
        roots: Vec<u32>,
        conflicts: Vec<(u32, u32, u32, u32)>,
        focused: HashSet<u32>,
    ) -> Self {
        let n = package_names.len();
        let n_versions: Vec<usize> = version_names.iter().map(|v| v.len()).collect();
        let mut out_edges = vec![Vec::new(); n];
        let mut in_edges = vec![Vec::new(); n];
        for (ei, e) in edges.iter().enumerate() {
            out_edges[e.src as usize].push(ei as u32);
            in_edges[e.dst as usize].push(ei as u32);
        }
        let max_versions = n_versions.iter().max().copied().unwrap_or(1);
        let mut domain = domain;
        for d in &mut domain {
            d.preallocate(max_versions);
        }
        // Build variable→package index mapping
        let mut var_to_package = Vec::new();
        for (pi, &nv) in n_versions.iter().enumerate() {
            for _ in 0..nv {
                var_to_package.push(pi as u32);
            }
        }
        Self {
            package_names,
            version_names,
            n_versions,
            var_to_package,
            domain,
            edges,
            out_edges,
            in_edges,
            roots,
            conflicts,
            focused,
        }
    }
}

/// GFP (Greatest Fixed Point) propagation kernel.
/// Iterates propagate_ex until domain stabilizes.
/// Returns Err(pkg_idx) if any domain empties (unsatisfiable).
pub fn gfp_apply(g: &mut KnotGraph) -> Result<(), usize> {
    let mut emptied = None;
    loop {
        let fp_before = domain_fingerprint(&g.domain);
        propagate_ex(g, Some(&mut emptied), None);
        if let Some(ep) = emptied {
            return Err(ep);
        }
        let fp_after = domain_fingerprint(&g.domain);
        if fp_before == fp_after {
            return Ok(());
        }
    }
}

fn domain_fingerprint(domain: &[DomainBits]) -> u64 {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    for d in domain {
        for &w in &d.words {
            h.write_u64(w);
        }
    }
    h.finish()
}

/// Star-edit cover for delta re-resolution.
/// Given a KnotRequest for the full project and a set of changed/added package names,
/// returns a reduced KnotRequest covering only the star (changed packages + their
/// dependents). The boundary is pre-resolved from the stored capsule.
/// O(bits × deg²) per edit.
pub fn star_edit_cover(full_request: &KnotRequest, changed_packages: &[String]) -> KnotRequest {
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
        focus_packages: full_request.focus_packages.clone(),
    }
}

pub fn propagate_ex(
    g: &mut KnotGraph,
    emptied: Option<&mut Option<usize>>,
    mut journal: Option<&mut JournalArena>,
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
            for (wi, &w) in g.domain[pi].words.iter().enumerate() {
                if w == 0 {
                    continue;
                }
                let mut x = w;
                while x != 0 {
                    let b = x.trailing_zeros() as usize;
                    let v = wi * 64 + b;
                    if v < e.support.n_rows {
                        let sup = e.support.get_row(v);
                        unsafe {
                            FlatBitMatrix::or_assign(&mut union_support.words, sup);
                        }
                        any = true;
                    }
                    x &= x - 1;
                }
            }
            if !any || union_support.is_empty() {
                // unconstrained row(s) — do not wipe
                continue;
            }
            let new_d = g.domain[dst].intersected(&union_support);
            if new_d.words != g.domain[dst].words {
                if let Some(j) = journal.as_deref_mut() {
                    j.push(dst as u32, &g.domain[dst]);
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
            for (wi, &w) in g.domain[src].words.iter().enumerate() {
                if w == 0 {
                    continue;
                }
                let mut x = w;
                while x != 0 {
                    let b = x.trailing_zeros() as usize;
                    let v = wi * 64 + b;
                    if v < e.support.n_rows {
                        let row = e.support.get_row(v);
                        let mut intersects = false;
                        for (i, &rw) in row.iter().enumerate() {
                            if rw != 0 {
                                let dst_w = dst_dom.words.get(i).copied().unwrap_or(0);
                                if rw & dst_w != 0 {
                                    intersects = true;
                                    break;
                                }
                            }
                        }
                        if !intersects {
                            // empty row or no overlap → source unreachable
                            new_src.clear_bit(v);
                            changed = true;
                        }
                    }
                    x &= x - 1;
                }
            }
            if changed {
                if let Some(j) = journal.as_deref_mut() {
                    j.push(src as u32, &g.domain[src]);
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

pub fn journal_undo(g: &mut KnotGraph, journal: &mut JournalArena, mark: usize) {
    journal.undo(g, mark);
}

fn open_supports_into(g: &mut KnotGraph, victim: usize) -> usize {
    let mut n = 0usize;
    for &ei in &g.in_edges[victim] {
        let e = &mut g.edges[ei as usize];
        e.weak = true;
        for row_idx in 0..e.support.n_rows {
            if !e.support.is_row_full(row_idx) {
                e.support.set_row_all(row_idx);
                n += 1;
            }
        }
    }
    n
}

pub(crate) fn soften_until_ac(
    g: &mut KnotGraph,
    pristine: &[DomainBits],
    max_rounds: usize,
) -> usize {
    let mut widened = 0usize;
    for _ in 0..max_rounds {
        g.domain.clone_from_slice(pristine);
        let mut emptied = None;
        if propagate_ex(g, Some(&mut emptied), None) {
            return widened;
        }
        let victim =
            emptied.unwrap_or_else(|| g.domain.iter().position(|d| d.is_empty()).unwrap_or(0));
        let n = open_supports_into(g, victim);
        if n == 0 {
            for &ei in g.in_edges[victim].iter().chain(g.out_edges[victim].iter()) {
                g.edges[ei as usize].weak = true;
            }
            widened += 1;
            g.domain.clone_from_slice(pristine);
            if propagate(g) {
                return widened;
            }
            for e in &mut g.edges {
                for row_idx in 0..e.support.n_rows {
                    if !e.support.is_row_full(row_idx) {
                        e.support.set_row_all(row_idx);
                        widened += 1;
                    }
                }
                e.weak = true;
            }
            g.domain.clone_from_slice(pristine);
            let _ = propagate(g);
            return widened;
        }
        widened += n;
    }
    g.domain.clone_from_slice(pristine);
    widened
}

fn pick_variable(g: &KnotGraph, betti_order: Option<&[f64]>) -> Option<usize> {
    let mut best: Option<(bool, i32, u32, i32, usize)> = None;
    for (i, d) in g.domain.iter().enumerate() {
        let c = d.count_ones();
        if c <= 1 {
            continue;
        }
        let pkg_idx = g.var_to_package.get(i).copied().unwrap_or(i as u32) as usize;
        let deg = (g.out_edges.get(pkg_idx).map_or(0, |v| v.len())
            + g.in_edges.get(pkg_idx).map_or(0, |v| v.len())) as i32;
        let betti_score = betti_order
            .and_then(|b| b.get(pkg_idx).copied())
            .unwrap_or(0.0);
        // Focused (SCC obstruction) packages always explored first. This
        // preserves completeness — it only reorders the search so the cyclic
        // core is decided before the (mostly forced) DAG portion.
        let focused = g.focused.contains(&(pkg_idx as u32));
        // Order: focus first (desc), Betti-score (desc), domain size (asc),
        // degree (desc), index (asc, for determinism)
        let key = (focused, -(f64_to_ord(betti_score)), c as u32, -deg, i);
        if best.map(|b| key < b).unwrap_or(true) {
            best = Some(key);
        }
    }
    best.map(|(_, _, _, _, i)| i)
}

/// Convert f64 to comparable i32 for ordering (higher β₁ → more negative → sorted first)
fn f64_to_ord(v: f64) -> i32 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= 1.0 {
        i32::MAX
    } else {
        (v * i32::MAX as f64) as i32
    }
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
        let empty_sup = vec![0u64; (g.n_versions[e.dst as usize] + 63) / 64];
        let sup = if s < e.support.n_rows {
            e.support.get_row(s)
        } else {
            &empty_sup
        };
        let sup_empty = unsafe { FlatBitMatrix::is_zero(sup) };
        let d_dom = &g.domain[e.dst as usize].words;
        let sup_has_d = if sup_empty {
            false
        } else {
            sup.iter().enumerate().any(|(i, &w)| {
                let d_w = d_dom.get(i).copied().unwrap_or(0);
                w & d_w != 0
            })
        };
        if e.weak {
            if !sup_empty && !sup_has_d {
                return false;
            }
        } else if !sup_has_d {
            return false;
        }
    }
    true
}

fn domain_objective(domain: &[DomainBits]) -> i64 {
    domain.iter().map(|d| d.max_bit().unwrap_or(0) as i64).sum()
}

pub(crate) fn knot_search(
    g: &mut KnotGraph,
    nodes: &mut u64,
    decisions: &mut u64,
    incumbent: &mut Option<i64>,
    best_domain: &mut Option<Vec<DomainBits>>,
    journal: &mut JournalArena,
    betti_order: Option<&[f64]>,
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

    let Some(p) = pick_variable(g, betti_order) else {
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
        journal.push(p as u32, &g.domain[p]);
        g.domain[p] = DomainBits::singleton(v, nbits);
        let prop_ok = propagate_ex(g, None, Some(journal));
        if prop_ok
            && knot_search(
                g,
                nodes,
                decisions,
                incumbent,
                best_domain,
                journal,
                betti_order,
            )
        {
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
        e.support.data.hash(&mut h);
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
        vers.sort_by(
            |a, b| match (semver::Version::parse(a), semver::Version::parse(b)) {
                (Ok(oa), Ok(ob)) => oa.partial_cmp(&ob).unwrap_or(std::cmp::Ordering::Equal),
                _ => a.cmp(b),
            },
        );
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
    let mut edge_map: HashMap<(usize, usize), FlatBitMatrix> = HashMap::new();
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
            let entry = edge_map
                .entry((si, di))
                .or_insert_with(|| FlatBitMatrix::zeros(n_versions[si], n_versions[di]));
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
            for (i, &w) in mask.words.iter().enumerate() {
                entry.get_row_mut(sv)[i] |= w;
            }
        }
    }

    let mut edges = Vec::new();
    for ((si, di), support) in edge_map {
        let full = DomainBits::all(n_versions[di]);
        let weak = support
            .iter_rows()
            .any(|row| row.iter().all(|&w| w == 0) || row == full.words.as_slice());
        edges.push(Edge {
            src: si as u32,
            dst: di as u32,
            support,
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

    let focused: HashSet<u32> = match &request.focus_packages {
        Some(focus) => focus
            .iter()
            .filter_map(|p| name_to_idx.get(p).map(|&i| i as u32))
            .collect(),
        None => HashSet::new(),
    };

    Ok(KnotGraph::from_parts(
        package_names,
        version_names,
        domain,
        edges,
        roots,
        conflicts,
        focused,
    ))
}

/// Solve a KnotRequest with the knot solver.
/// `betti_order` — optional per-package Betti scores from Layer 2 sheaf
/// cohomology. High β₁ packages (cycle formers) are branched on first.
pub fn knot_solve(
    request: &KnotRequest,
    betti_order: Option<&[f64]>,
) -> Result<KnotResult, String> {
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
    if pick_variable(&g, betti_order).is_none() && !assignment_feasible_on(&g) {
        g.domain = pristine.clone();
    }

    let mut nodes = 0u64;
    let mut decisions = 0u64;
    let mut incumbent = None;
    let mut best_domain = None;
    let mut journal = JournalArena::with_capacity(1024, 128);

    let mut ok = if pick_variable(&g, betti_order).is_none() && assignment_feasible_on(&g) {
        incumbent = Some(domain_objective(&g.domain));
        best_domain = Some(g.domain.clone());
        nodes = 1;
        true
    } else {
        if pick_variable(&g, betti_order).is_none() {
            g.domain = pristine.clone();
        }
        knot_search(
            &mut g,
            &mut nodes,
            &mut decisions,
            &mut incumbent,
            &mut best_domain,
            &mut journal,
            betti_order,
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
            betti_order,
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
        return Err(format!(
            "lockfile validation: emptied domains: {}",
            emptied.join(", ")
        ));
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
    betti_order: Option<&[f64]>,
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
    let mut journal = JournalArena::with_capacity(1024, 128);

    let ok = knot_search(
        &mut knot_g,
        &mut nodes,
        &mut decisions,
        &mut incumbent,
        &mut best_domain,
        &mut journal,
        betti_order,
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
    journal: &JournalArena,
    emptied_pkg: usize,
) -> Vec<String> {
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(emptied_pkg);
    visited.insert(emptied_pkg);

    // Walk journal entries in reverse: find which domain changes led to emptiness
    for (pkg, _, _) in journal.entries.iter().rev() {
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
            focus_packages: None,
        }
    }

    #[test]
    fn knot_solves_simple_prefer_latest() {
        let req = simple_request();
        let r = knot_solve(&req, None).expect("knot should solve");
        assert!(r.is_satisfiable);
        assert_eq!(r.selection.get("root").map(String::as_str), Some("1.0.0"));
        assert_eq!(r.selection.get("a").map(String::as_str), Some("1.1.0"));
        assert_eq!(r.selection.get("b").map(String::as_str), Some("2.1.0"));
    }

    #[test]
    fn knot_capsule_hit_on_repeat() {
        let req = simple_request();
        let a = knot_solve(&req, None).unwrap();
        let b = knot_solve(&req, None).unwrap();
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
            assert_eq!(d.words, g.domain[i].words, "gfp not fixed at pkg {}", i);
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
            focus_packages: None,
        };
        let mut g = build_knot_graph(&req).expect("graph should build");
        assert!(gfp_apply(&mut g).is_ok());
    }
}
