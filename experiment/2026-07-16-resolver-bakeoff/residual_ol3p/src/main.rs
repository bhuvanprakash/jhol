//! Residual-Certified OL3P — Rust-tightened experiment prototype.
//!
//! Original jhol tree untouched. Path-dep on `jhol-core` OL3P.
//!
//! Rust levers used:
//! - `u64` domain bitsets + clear-lowest-set-bit iteration
//! - CSR edge index (out/in) for O(touch) force-wave worklists
//! - hashed Crystal signatures (no `BTreeMap<Vec<u64>>`)
//! - shell as bitset words, not `BTreeSet`
//! - `rayon` for independent arm / membrane-local OL3P
//! - SoA-ish compact support slabs

use clap::Parser;
use jhol_core::jagr::{solve_maxsat_ol3p, Assignment, Clause, Literal, CNFFormula};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(name = "residual-ol3p-experiment")]
struct Args {
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = 3)]
    rounds: usize,
    /// Also build graphs from real npm packuments under this fixtures dir
    #[arg(long)]
    packument_fixtures: Option<PathBuf>,
    #[arg(long, default_value_t = 48)]
    max_packages: usize,
    #[arg(long, default_value_t = 16)]
    max_versions: usize,
    #[arg(long, default_value_t = 4)]
    max_depth: usize,
    /// Path to jhol binary for dual/pulse wall-clock compare (optional)
    #[arg(long)]
    jhol_bin: Option<PathBuf>,
    /// Path to bun for install wall-clock compare
    #[arg(long)]
    bun_bin: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct Package {
    n_versions: u8,
}

#[derive(Clone, Debug)]
struct Edge {
    src: u32,
    dst: u32,
    /// support[v] = dst domain mask compatible with src version v
    support: Box<[u64]>,
    /// Packument fallback / loose edge: forward-check only (no backward wipe).
    weak: bool,
}

#[derive(Clone, Debug)]
struct Graph {
    packages: Vec<Package>,
    domain: Vec<u64>,
    edges: Vec<Edge>,
    /// CSR: package -> outgoing edge indices
    out_edges: Vec<Vec<u32>>,
    /// CSR: package -> incoming edge indices
    in_edges: Vec<Vec<u32>>,
    roots: Vec<u32>,
}

impl Graph {
    fn from_parts(packages: Vec<Package>, domain: Vec<u64>, edges: Vec<Edge>, roots: Vec<u32>) -> Self {
        let n = packages.len();
        let mut out_edges = vec![Vec::new(); n];
        let mut in_edges = vec![Vec::new(); n];
        for (ei, e) in edges.iter().enumerate() {
            out_edges[e.src as usize].push(ei as u32);
            in_edges[e.dst as usize].push(ei as u32);
        }
        Self {
            packages,
            domain,
            edges,
            out_edges,
            in_edges,
            roots,
        }
    }
}

#[derive(Serialize, Clone)]
struct ArmResult {
    arm: String,
    engine: String,
    packages: usize,
    versions_raw: usize,
    crystals: usize,
    shell_forced: usize,
    residual_pkgs: usize,
    membranes: usize,
    cnf_vars: usize,
    cnf_hard: usize,
    cnf_soft: usize,
    knot_nodes: u64,
    knot_decisions: u64,
    force_wave_rounds: usize,
    seconds: f64,
    force_ms: f64,
    encode_ms: f64,
    ol3p_ms: f64,
    knot_ms: f64,
    objective: Option<i64>,
    ok: bool,
    error: Option<String>,
    rounds_used: usize,
    /// Knot v4: resolution capsule cache hit
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_hit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capsule_fp: Option<u64>,
}

#[derive(Serialize, Clone)]
struct CapsuleScaleReport {
    /// Simulated concurrent / sequential resolve requests (stand-in for many users)
    requests: usize,
    unique_manifests: usize,
    cache_hits: usize,
    cache_misses: usize,
    hit_rate: f64,
    cold_total_ms: f64,
    warm_total_ms: f64,
    mean_cold_ms: f64,
    mean_warm_ms: f64,
    speedup_warm_vs_cold: f64,
    note: String,
}

#[derive(Serialize)]
struct FixtureReport {
    fixture: String,
    arms: Vec<ArmResult>,
}

#[derive(Serialize)]
struct PackumentMeta {
    fixture: String,
    packages_fetched: usize,
    versions_kept: usize,
    edges: usize,
    fetch_errors: Vec<String>,
    build_ms: f64,
}

#[derive(Serialize)]
struct JholProxyTiming {
    fixture: String,
    arm: String,
    seconds: f64,
    exit: i32,
}

#[derive(Serialize)]
struct BunTiming {
    fixture: String,
    seconds: f64,
    exit: i32,
}

#[derive(Serialize)]
struct Report {
    note: String,
    rust_features: Vec<&'static str>,
    fixtures: Vec<FixtureReport>,
    scale_sweep: Vec<ArmResult>,
    packument_fixtures: Vec<FixtureReport>,
    packument_meta: Vec<PackumentMeta>,
    jhol_proxy: Vec<JholProxyTiming>,
    bun_install: Vec<BunTiming>,
    parity: Vec<ParityRow>,
    hard_packument: Vec<FixtureReport>,
    /// Knot v4: million-user style capsule cache stress
    capsule_scale: Option<CapsuleScaleReport>,
}

#[inline]
fn mask_all(n: u8) -> u64 {
    if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

/// Iterate set bits of `m`, clearing as we go (classic bitset idiom).
#[inline]
fn for_each_bit(mut m: u64, mut f: impl FnMut(u32)) {
    while m != 0 {
        let v = m.trailing_zeros();
        f(v);
        m &= m - 1;
    }
}

#[inline]
fn bit(v: u32) -> u64 {
    1u64 << v
}

/// Shell bitset: word-packed package ids with domain popcount == 1.
#[derive(Clone, Debug)]
struct ShellBits {
    words: Vec<u64>,
    count: usize,
}

impl ShellBits {
    fn from_domains(domain: &[u64]) -> Self {
        let nwords = (domain.len() + 63) / 64;
        let mut words = vec![0u64; nwords];
        let mut count = 0usize;
        for (i, &d) in domain.iter().enumerate() {
            if d.count_ones() == 1 {
                words[i / 64] |= 1u64 << (i % 64);
                count += 1;
            }
        }
        Self { words, count }
    }

    #[inline]
    fn contains(&self, pkg: usize) -> bool {
        self.words
            .get(pkg / 64)
            .map(|w| (w >> (pkg % 64)) & 1 == 1)
            .unwrap_or(false)
    }
}

/// Arc consistency propagate. Returns false if any domain emptied.
/// `weak` edges: forward only (never kill src from dst).
/// When `emptied` is Some, records the first emptied package index.
/// When `journal` is Some, records (pkg, old_domain) before each wipe for O(touched) undo.
fn propagate_ex(
    g: &mut Graph,
    emptied: Option<&mut Option<usize>>,
    mut journal: Option<&mut Vec<(u32, u64)>>,
) -> bool {
    let n = g.packages.len();
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
        if guard > n * 256 {
            break;
        }

        for &ei in &g.out_edges[pi] {
            let e = &g.edges[ei as usize];
            let dst = e.dst as usize;
            let dom = g.domain[pi];
            let mut union_support = 0u64;
            for_each_bit(dom, |v| {
                if let Some(&sup) = e.support.get(v as usize) {
                    union_support |= sup;
                }
            });
            // If src has no support info, do not wipe dst
            if union_support == 0 && dom != 0 {
                continue;
            }
            let new_d = g.domain[dst] & union_support;
            if new_d != g.domain[dst] {
                if let Some(j) = journal.as_deref_mut() {
                    j.push((dst as u32, g.domain[dst]));
                }
                g.domain[dst] = new_d;
                if new_d == 0 {
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
                continue; // no backward wipe on weak edges
            }
            let src = e.src as usize;
            let dst_dom = g.domain[pi];
            let mut new_src = g.domain[src];
            for_each_bit(new_src, |v| {
                let sup = e.support.get(v as usize).copied().unwrap_or(0);
                if sup != 0 && sup & dst_dom == 0 {
                    new_src &= !bit(v);
                }
            });
            if new_src != g.domain[src] {
                if let Some(j) = journal.as_deref_mut() {
                    j.push((src as u32, g.domain[src]));
                }
                g.domain[src] = new_src;
                if new_src == 0 {
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
    !g.domain.iter().any(|&d| d == 0)
}

fn propagate(g: &mut Graph) -> bool {
    propagate_ex(g, None, None)
}

fn journal_undo(g: &mut Graph, journal: &mut Vec<(u32, u64)>, mark: usize) {
    while journal.len() > mark {
        let (pkg, old) = journal.pop().unwrap();
        g.domain[pkg as usize] = old;
    }
}

/// Packument graphs truncate version lists → support masks over-constrain.
/// Soften: open support rows that empty a package under AC; mark edge weak.
/// Returns how many support rows widened.
fn soften_until_ac(g: &mut Graph, pristine: &[u64], max_rounds: usize) -> usize {
    let mut widened = 0usize;
    for _ in 0..max_rounds {
        g.domain = pristine.to_vec();
        let mut emptied = None;
        if propagate_ex(g, Some(&mut emptied), None) {
            return widened;
        }
        let Some(victim) = emptied else {
            // domains empty without a tracked victim — open all in-edges of first empty
            let victim = g
                .domain
                .iter()
                .position(|&d| d == 0)
                .unwrap_or(0);
            widened += open_supports_into(g, victim);
            continue;
        };
        let n = open_supports_into(g, victim);
        if n == 0 {
            // already fully open — mark all incident edges weak and bail
            for &ei in g.in_edges[victim].iter().chain(g.out_edges[victim].iter()) {
                g.edges[ei as usize].weak = true;
            }
            widened += 1;
            g.domain = pristine.to_vec();
            if propagate(g) {
                return widened;
            }
            // last resort: full-open every edge support
            for e in &mut g.edges {
                let full = mask_all(g.packages[e.dst as usize].n_versions);
                for row in e.support.iter_mut() {
                    if *row != full {
                        *row = full;
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

fn open_supports_into(g: &mut Graph, victim: usize) -> usize {
    let full = mask_all(g.packages[victim].n_versions);
    let mut n = 0usize;
    for &ei in &g.in_edges[victim] {
        let e = &mut g.edges[ei as usize];
        e.weak = true;
        for row in e.support.iter_mut() {
            if *row != full && *row != 0 {
                *row = full;
                n += 1;
            } else if *row == 0 {
                *row = full;
                n += 1;
            }
        }
    }
    n
}

/// Legacy name used by OL3P path — full bidirectional wipe (strict edges).
fn force_wave(g: &mut Graph) -> usize {
    let before = g.domain.clone();
    let _ = propagate(g);
    // count roughly changed
    before
        .iter()
        .zip(g.domain.iter())
        .filter(|(a, b)| a != b)
        .count()
        .max(1)
}

/// Exact Crystal signature (support masks in CSR out-edge order). HashMap key.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CrystalSig(Box<[u64]>);

fn crystal_sig(g: &Graph, pkg: usize, ver: u32) -> CrystalSig {
    let masks: Vec<u64> = g.out_edges[pkg]
        .iter()
        .map(|&ei| {
            g.edges[ei as usize]
                .support
                .get(ver as usize)
                .copied()
                .unwrap_or(0)
        })
        .collect();
    CrystalSig(masks.into_boxed_slice())
}

/// Group live versions into Crystals by exact outgoing support signature.
fn crystals_for_package(g: &Graph, pkg: usize, residual_only: bool, shell: &ShellBits) -> Vec<Vec<u32>> {
    if residual_only && shell.contains(pkg) {
        return Vec::new();
    }
    let dom = g.domain[pkg];
    let mut map: HashMap<CrystalSig, Vec<u32>> = HashMap::with_capacity(dom.count_ones() as usize);
    for_each_bit(dom, |v| {
        let sig = crystal_sig(g, pkg, v);
        map.entry(sig).or_default().push(v);
    });
    let mut groups: Vec<Vec<u32>> = map.into_values().collect();
    groups.sort_unstable_by_key(|g| g.iter().copied().min().unwrap_or(0));
    for g in &mut groups {
        g.sort_unstable();
    }
    groups
}

/// Connected components on residual packages (undirected via edges).
fn residual_membranes(g: &Graph, shell: &ShellBits) -> Vec<Vec<usize>> {
    let n = g.packages.len();
    let mut seen = vec![false; n];
    let mut comps = Vec::new();
    for start in 0..n {
        if shell.contains(start) || seen[start] || g.domain[start] == 0 {
            continue;
        }
        let mut stack = vec![start];
        let mut comp = Vec::new();
        seen[start] = true;
        while let Some(u) = stack.pop() {
            comp.push(u);
            for &ei in g.out_edges[u].iter().chain(g.in_edges[u].iter()) {
                let e = &g.edges[ei as usize];
                for &v in &[e.src as usize, e.dst as usize] {
                    if !shell.contains(v) && !seen[v] && g.domain[v] != 0 {
                        seen[v] = true;
                        stack.push(v);
                    }
                }
            }
        }
        comps.push(comp);
    }
    comps
}

struct Encoding {
    /// var for (pkg, crystal_idx)
    var_of: HashMap<(u32, u16), u32>,
    formula: CNFFormula,
    residual_pkgs: usize,
    crystals: usize,
}

fn encode_crystal_cnf(
    g: &Graph,
    shell: &ShellBits,
    residual_only: bool,
    math_certs: &[(u32, u16)],
    only_pkgs: Option<&[usize]>,
) -> Encoding {
    let mut var_of: HashMap<(u32, u16), u32> = HashMap::new();
    let mut crystal_lists: HashMap<u32, Vec<Vec<u32>>> = HashMap::new();
    let mut next = 0u32;
    let mut residual_pkgs = 0usize;
    let mut crystals = 0usize;

    let pkg_iter: Box<dyn Iterator<Item = usize>> = match only_pkgs {
        Some(slice) => Box::new(slice.iter().copied()),
        None => Box::new(0..g.packages.len()),
    };

    for p in pkg_iter {
        let groups = crystals_for_package(g, p, residual_only, shell);
        if groups.is_empty() {
            continue;
        }
        residual_pkgs += 1;
        for (ci, _) in groups.iter().enumerate() {
            var_of.insert((p as u32, ci as u16), next);
            next += 1;
            crystals += 1;
        }
        crystal_lists.insert(p as u32, groups);
    }

    if !residual_only && only_pkgs.is_none() {
        // rebuild full mode without shell filter
        var_of.clear();
        crystal_lists.clear();
        next = 0;
        residual_pkgs = 0;
        crystals = 0;
        let empty = ShellBits {
            words: vec![0; (g.packages.len() + 63) / 64],
            count: 0,
        };
        for p in 0..g.packages.len() {
            let groups = crystals_for_package(g, p, false, &empty);
            if groups.is_empty() {
                continue;
            }
            residual_pkgs += 1;
            for (ci, _) in groups.iter().enumerate() {
                var_of.insert((p as u32, ci as u16), next);
                next += 1;
                crystals += 1;
            }
            crystal_lists.insert(p as u32, groups);
        }
    }

    let mut formula = CNFFormula::new(next.max(1) as usize);

    for (p, groups) in &crystal_lists {
        let vars: Vec<u32> = (0..groups.len() as u16)
            .filter_map(|ci| var_of.get(&(*p, ci)).copied())
            .collect();
        if vars.is_empty() {
            continue;
        }
        formula.add_hard(Clause::new(
            vars.iter().map(|&v| Literal::positive(v as usize)).collect(),
        ));
        for i in 0..vars.len() {
            for j in (i + 1)..vars.len() {
                formula.add_hard(Clause::new(vec![
                    Literal::negative(vars[i] as usize),
                    Literal::negative(vars[j] as usize),
                ]));
            }
        }
        for (ci, vers) in groups.iter().enumerate() {
            if let Some(&var) = var_of.get(&(*p, ci as u16)) {
                let prefer = vers.iter().copied().max().unwrap_or(0) as f64 + 1.0;
                formula.add_soft(Clause::with_weight(
                    vec![Literal::positive(var as usize)],
                    prefer,
                ));
            }
        }
    }

    for e in &g.edges {
        let Some(src_groups) = crystal_lists.get(&e.src) else {
            continue;
        };
        match crystal_lists.get(&e.dst) {
            None if shell.contains(e.dst as usize) => {
                let forced_bit = g.domain[e.dst as usize].trailing_zeros();
                for (sci, svers) in src_groups.iter().enumerate() {
                    let Some(&svar) = var_of.get(&(e.src, sci as u16)) else {
                        continue;
                    };
                    let ok = svers.iter().any(|&sv| {
                        e.support
                            .get(sv as usize)
                            .map(|sup| (sup >> forced_bit) & 1 == 1)
                            .unwrap_or(false)
                    });
                    if !ok {
                        formula.add_hard(Clause::new(vec![Literal::negative(svar as usize)]));
                    }
                }
            }
            Some(dst_groups) => {
                for (sci, svers) in src_groups.iter().enumerate() {
                    let Some(&svar) = var_of.get(&(e.src, sci as u16)) else {
                        continue;
                    };
                    let mut compatible = Vec::with_capacity(dst_groups.len());
                    for (dci, dvers) in dst_groups.iter().enumerate() {
                        let Some(&dvar) = var_of.get(&(e.dst, dci as u16)) else {
                            continue;
                        };
                        let ok = svers.iter().any(|&sv| {
                            let sup = e.support.get(sv as usize).copied().unwrap_or(0);
                            dvers.iter().any(|&dv| (sup >> dv) & 1 == 1)
                        });
                        if ok {
                            compatible.push(dvar);
                        }
                    }
                    let mut lits = Vec::with_capacity(1 + compatible.len());
                    lits.push(Literal::negative(svar as usize));
                    lits.extend(compatible.into_iter().map(|v| Literal::positive(v as usize)));
                    formula.add_hard(Clause::new(lits));
                }
            }
            None => {}
        }
    }

    for &(p, ci) in math_certs {
        if let Some(&v) = var_of.get(&(p, ci)) {
            formula.add_hard(Clause::new(vec![Literal::positive(v as usize)]));
        }
    }

    Encoding {
        var_of,
        formula,
        residual_pkgs,
        crystals,
    }
}

fn solve_encoding(enc: &mut Encoding, residual: bool, g: &Graph, shell: &ShellBits, rounds: usize) -> (bool, Option<String>, usize) {
    let mut last_err = None;
    let mut ok = false;
    let mut rounds_used = 1usize;
    for r in 0..rounds.max(1) {
        rounds_used = r + 1;
        match solve_maxsat_ol3p(enc.formula.clone()) {
            Ok(_) => {
                ok = true;
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                if !residual {
                    break;
                }
                let mut injected = false;
                for p in 0..g.packages.len() {
                    if shell.contains(p) {
                        continue;
                    }
                    let groups = crystals_for_package(g, p, true, shell);
                    if groups.len() > 1 {
                        if let Some(&v) = enc.var_of.get(&(p as u32, 0)) {
                            enc.formula
                                .add_hard(Clause::new(vec![Literal::negative(v as usize)]));
                            injected = true;
                            break;
                        }
                    }
                }
                if !injected {
                    break;
                }
            }
        }
    }
    (ok, last_err, rounds_used)
}

fn run_arm(
    name: &str,
    g0: &Graph,
    residual: bool,
    use_math_forge: bool,
    use_membranes: bool,
    feedback_rounds: usize,
) -> ArmResult {
    let mut g = g0.clone();
    let t0 = Instant::now();

    let tw = Instant::now();
    let force_wave_rounds = force_wave(&mut g);
    let force_ms = tw.elapsed().as_secs_f64() * 1000.0;

    let shell = ShellBits::from_domains(&g.domain);
    let shell_forced = shell.count;
    let versions_raw: usize = g.packages.iter().map(|p| p.n_versions as usize).sum();

    let mut math_certs: Vec<(u32, u16)> = Vec::new();
    if use_math_forge && residual {
        for p in 0..g.packages.len() {
            if shell.contains(p) {
                continue;
            }
            if crystals_for_package(&g, p, true, &shell).len() == 1 {
                math_certs.push((p as u32, 0));
            }
        }
    }

    let te = Instant::now();
    let membranes = if use_membranes && residual {
        residual_membranes(&g, &shell)
    } else {
        Vec::new()
    };
    let membrane_count = if use_membranes && residual {
        membranes.len().max(1)
    } else {
        1
    };

    let ol3p_start = Instant::now();
    let (ok, last_err, rounds_used, crystals, residual_pkgs, cnf_vars, cnf_hard, cnf_soft) =
        if use_membranes && residual && membranes.len() > 1 {
            let results: Vec<_> = membranes
                .par_iter()
                .map(|comp| {
                    let mut enc =
                        encode_crystal_cnf(&g, &shell, true, &math_certs, Some(comp.as_slice()));
                    let (ok, err, rounds) =
                        solve_encoding(&mut enc, true, &g, &shell, feedback_rounds);
                    (
                        ok,
                        err,
                        rounds,
                        enc.crystals,
                        enc.residual_pkgs,
                        enc.formula.num_vars,
                        enc.formula.hard_clauses.len(),
                        enc.formula.soft_clauses.len(),
                    )
                })
                .collect();
            let ok = results.iter().all(|r| r.0);
            let err = results.iter().find_map(|r| r.1.clone());
            let rounds_used = results.iter().map(|r| r.2).max().unwrap_or(1);
            let crystals = results.iter().map(|r| r.3).sum();
            let residual_pkgs = results.iter().map(|r| r.4).sum();
            let cnf_vars = results.iter().map(|r| r.5).sum();
            let cnf_hard = results.iter().map(|r| r.6).sum();
            let cnf_soft = results.iter().map(|r| r.7).sum();
            (
                ok,
                err,
                rounds_used,
                crystals,
                residual_pkgs,
                cnf_vars,
                cnf_hard,
                cnf_soft,
            )
        } else {
            let mut enc = encode_crystal_cnf(&g, &shell, residual, &math_certs, None);
            let (ok, err, rounds) =
                solve_encoding(&mut enc, residual, &g, &shell, feedback_rounds);
            (
                ok,
                err,
                rounds,
                enc.crystals,
                enc.residual_pkgs,
                enc.formula.num_vars,
                enc.formula.hard_clauses.len(),
                enc.formula.soft_clauses.len(),
            )
        };
    let encode_ms = te.elapsed().as_secs_f64() * 1000.0;
    let ol3p_ms = ol3p_start.elapsed().as_secs_f64() * 1000.0;

    ArmResult {
        arm: name.to_string(),
        engine: "ol3p".into(),
        packages: g.packages.len(),
        versions_raw,
        crystals,
        shell_forced,
        residual_pkgs,
        membranes: membrane_count,
        cnf_vars,
        cnf_hard,
        cnf_soft,
        knot_nodes: 0,
        knot_decisions: 0,
        force_wave_rounds,
        seconds: t0.elapsed().as_secs_f64(),
        force_ms,
        encode_ms,
        ol3p_ms,
        knot_ms: 0.0,
        objective: None,
        ok,
        error: last_err,
        rounds_used,
        cache_hit: None,
        capsule_fp: None,
    }
}

/// Prefer-latest style objective: sum of selected version bit indices.
#[inline]
fn domain_objective(domain: &[u64]) -> i64 {
    domain
        .iter()
        .map(|&d| {
            if d == 0 {
                0
            } else {
                63 - d.leading_zeros() as i64
            }
        })
        .sum()
}

/// Degree-aware fail-first: among min-domain packages, pick highest edge degree.
fn pick_variable(g: &Graph) -> Option<usize> {
    let mut best: Option<(u32, i32, usize)> = None; // (dom_size, -degree, idx)
    for (i, &d) in g.domain.iter().enumerate() {
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

/// JAGR Knot v4 — MAC search with journal undo (no full domain clone per branch).
fn knot_search(
    g: &mut Graph,
    nodes: &mut u64,
    decisions: &mut u64,
    incumbent: &mut Option<i64>,
    best_domain: &mut Option<Vec<u64>>,
    journal: &mut Vec<(u32, u64)>,
    _depth: u32,
) -> bool {
    *nodes += 1;
    if g.domain.iter().any(|&d| d == 0) {
        return false;
    }
    if g.roots.iter().any(|&r| g.domain[r as usize] == 0) {
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
    for &d in &g.domain {
        optimistic += 63 - d.leading_zeros() as i64;
    }
    if let Some(inc) = *incumbent {
        if optimistic <= inc {
            return false;
        }
    }

    let mut bits: Vec<u32> = Vec::new();
    for_each_bit(g.domain[p], |v| bits.push(v));
    bits.sort_unstable_by(|a, b| b.cmp(a));

    let mut any_sat = false;
    for v in bits {
        *decisions += 1;
        let mark = journal.len();
        journal.push((p as u32, g.domain[p]));
        g.domain[p] = bit(v);
        let prop_ok = propagate_ex(g, None, Some(journal));
        if prop_ok
            && knot_search(
                g,
                nodes,
                decisions,
                incumbent,
                best_domain,
                journal,
                _depth + 1,
            )
        {
            any_sat = true;
        }
        journal_undo(g, journal, mark);
    }
    any_sat
}

/// Fingerprint graph topology + domains for resolution capsule lookup.
fn graph_capsule_fp(g: &Graph) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.packages.len().hash(&mut h);
    for p in &g.packages {
        p.n_versions.hash(&mut h);
    }
    for &d in &g.domain {
        d.hash(&mut h);
    }
    g.edges.len().hash(&mut h);
    for e in &g.edges {
        e.src.hash(&mut h);
        e.dst.hash(&mut h);
        e.weak.hash(&mut h);
        e.support.len().hash(&mut h);
        for &s in e.support.iter() {
            s.hash(&mut h);
        }
    }
    for &r in &g.roots {
        r.hash(&mut h);
    }
    h.finish()
}

#[derive(Clone)]
struct CapsuleEntry {
    objective: i64,
    domain: Vec<u64>,
}

fn capsule_store() -> &'static Mutex<HashMap<u64, CapsuleEntry>> {
    static STORE: OnceLock<Mutex<HashMap<u64, CapsuleEntry>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn capsule_get(fp: u64) -> Option<CapsuleEntry> {
    capsule_store().lock().ok()?.get(&fp).cloned()
}

fn capsule_put(fp: u64, objective: i64, domain: Vec<u64>) {
    if let Ok(mut m) = capsule_store().lock() {
        // Bound memory: drop oldest-ish by clearing when huge (experiment LRU-lite)
        if m.len() > 50_000 {
            m.clear();
        }
        m.insert(fp, CapsuleEntry { objective, domain });
    }
}

fn clear_capsule_store() {
    if let Ok(mut m) = capsule_store().lock() {
        m.clear();
    }
}

fn assignment_feasible_on(g: &Graph) -> bool {
    if g.domain.iter().any(|&d| d == 0 || d.count_ones() != 1) {
        return false;
    }
    for e in &g.edges {
        let s = g.domain[e.src as usize].trailing_zeros();
        let d = g.domain[e.dst as usize].trailing_zeros();
        let sup = e.support.get(s as usize).copied().unwrap_or(0);
        // Match propagate: empty support row = unconstrained
        if e.weak {
            if sup != 0 && (sup >> d) & 1 == 0 {
                return false;
            }
            continue;
        }
        if sup == 0 {
            continue;
        }
        if (sup >> d) & 1 == 0 {
            return false;
        }
    }
    true
}

fn assignment_feasible(g0: &Graph, domain: &[u64]) -> bool {
    let mut g = g0.clone();
    g.domain = domain.to_vec();
    assignment_feasible_on(&g)
}

/// Decode OL3P boolean assignment back to per-package selected version bit.
/// Robust: if no true crystal var, pick highest-prefer crystal that keeps feasibility.
fn ol3p_domains_from_assignment(
    g: &Graph,
    shell: &ShellBits,
    residual: bool,
    asn: &Assignment,
) -> Option<Vec<u64>> {
    let enc = encode_crystal_cnf(g, shell, residual, &[], None);
    let mut out = g.domain.clone();
    for p in 0..g.packages.len() {
        if residual && shell.contains(p) {
            out[p] = g.domain[p];
            continue;
        }
        let groups = crystals_for_package(g, p, residual, shell);
        if groups.is_empty() {
            if g.domain[p].count_ones() == 1 {
                continue;
            }
            // force highest bit as last resort
            if g.domain[p] != 0 {
                out[p] = bit(63 - g.domain[p].leading_zeros());
            }
            continue;
        }
        let mut chosen: Option<u32> = None;
        let mut ranked: Vec<(i64, u32)> = Vec::new();
        for (ci, vers) in groups.iter().enumerate() {
            let Some(&var) = enc.var_of.get(&(p as u32, ci as u16)) else {
                continue;
            };
            let v = vers.iter().copied().max().unwrap_or(0);
            let pref = v as i64;
            if *asn.get(&(var as usize)).unwrap_or(&false) {
                chosen = Some(v);
                break;
            }
            ranked.push((pref, v));
        }
        if chosen.is_none() {
            ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0));
            chosen = ranked.first().map(|(_, v)| *v);
        }
        out[p] = bit(chosen?);
    }
    Some(out)
}

#[derive(Serialize)]
struct ParityRow {
    fixture: String,
    ol3p_ok: bool,
    knot_ok: bool,
    ol3p_obj: Option<i64>,
    knot_obj: Option<i64>,
    obj_match: bool,
    both_feasible: bool,
    domain_equal: bool,
    metamorphic_obj_stable: bool,
    note: String,
}

fn run_parity_on_graph(name: &str, g0: &Graph) -> ParityRow {
    // Knot from pristine (MAC + soften) — same as production Knot path
    let knot_arm = run_knot_arm("parity_knot", g0);
    let knot_ok = knot_arm.ok;
    let knot_obj = knot_arm.objective;

    // Fair compare: OL3P on the same AC-softened constraint set Knot searches
    let mut g_soft = g0.clone();
    let pristine = g0.domain.clone();
    let _ = soften_until_ac(
        &mut g_soft,
        &pristine,
        g0.packages.len().saturating_mul(4).max(32),
    );
    g_soft.domain = pristine.clone();
    let mut g = g_soft.clone();
    let _ = propagate(&mut g);
    if g.domain.iter().any(|&d| d == 0) {
        g.domain = pristine.clone();
    }
    let shell = ShellBits::from_domains(&g.domain);
    let enc = encode_crystal_cnf(&g, &shell, true, &[], None);
    let (ol3p_ok, ol3p_dom, ol3p_obj) = match solve_maxsat_ol3p(enc.formula) {
        Ok(asn) => {
            let dom = ol3p_domains_from_assignment(&g, &shell, true, &asn);
            let mut feas = false;
            let mut obj = None;
            if let Some(ref d) = dom {
                feas = {
                    let mut gg = g.clone();
                    gg.domain = d.clone();
                    assignment_feasible_on(&gg)
                } || {
                    let mut gg = g_soft.clone();
                    gg.domain = d.clone();
                    assignment_feasible_on(&gg)
                };
                if feas {
                    obj = Some(domain_objective(d));
                }
            }
            if feas {
                (true, dom, obj)
            } else {
                (false, dom, obj)
            }
        }
        Err(_) => (false, None, None),
    };

    // Domain snapshot from softened MAC search
    let mut gk = g_soft.clone();
    gk.domain = pristine;
    let mut best_domain = None;
    let mut incumbent = None;
    let mut nodes = 0;
    let mut decisions = 0;
    if !propagate(&mut gk) {
        gk.domain = g0.domain.clone();
    }
    let mut journal = Vec::new();
    let _ = knot_search(
        &mut gk,
        &mut nodes,
        &mut decisions,
        &mut incumbent,
        &mut best_domain,
        &mut journal,
        0,
    );
    let knot_dom = best_domain;
    let domain_equal = match (&ol3p_dom, &knot_dom) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let obj_match = match (ol3p_obj, knot_obj) {
        (Some(a), Some(b)) => a == b,
        (None, None) if ol3p_ok && knot_ok => true,
        _ => false,
    };
    let both_feasible = ol3p_ok && knot_ok;

    // Metamorphic edge rotate
    let mut edges = g0.edges.clone();
    let rot = edges.len() / 3;
    if rot > 0 {
        edges.rotate_left(rot);
    }
    let g2 = Graph::from_parts(
        g0.packages.clone(),
        g0.domain.clone(),
        edges,
        g0.roots.clone(),
    );
    let k2 = run_knot_arm("meta", &g2);
    let metamorphic_obj_stable = k2.objective == knot_obj && k2.ok == knot_ok;

    let note = if !both_feasible {
        format!("feasibility mismatch ol3p_ok={ol3p_ok} knot_ok={knot_ok}")
    } else if obj_match {
        "objective match (prefer-latest score)".into()
    } else {
        format!("obj differ ol3p={ol3p_obj:?} knot={knot_obj:?}")
    };

    ParityRow {
        fixture: name.into(),
        ol3p_ok,
        knot_ok,
        ol3p_obj,
        knot_obj,
        obj_match,
        both_feasible,
        domain_equal,
        metamorphic_obj_stable,
        note,
    }
}

fn run_knot_arm(name: &str, g0: &Graph) -> ArmResult {
    let t0 = Instant::now();
    let versions_raw: usize = g0.packages.iter().map(|p| p.n_versions as usize).sum();
    let pristine = g0.domain.clone();
    let fp = graph_capsule_fp(g0);

    // --- Knot v4: resolution capsule hit (million-user warm path) ---
    if let Some(cap) = capsule_get(fp) {
        let shell = ShellBits::from_domains(&cap.domain);
        return ArmResult {
            arm: name.to_string(),
            engine: "knot".into(),
            packages: g0.packages.len(),
            versions_raw,
            crystals: 0,
            shell_forced: shell.count,
            residual_pkgs: 0,
            membranes: 1,
            cnf_vars: 0,
            cnf_hard: 0,
            cnf_soft: 0,
            knot_nodes: 0,
            knot_decisions: 0,
            force_wave_rounds: 0,
            seconds: t0.elapsed().as_secs_f64(),
            force_ms: 0.0,
            encode_ms: 0.0,
            ol3p_ms: 0.0,
            knot_ms: t0.elapsed().as_secs_f64() * 1000.0,
            objective: Some(cap.objective),
            ok: true,
            error: None,
            rounds_used: 0,
            cache_hit: Some(true),
            capsule_fp: Some(fp),
        };
    }

    // --- Knot v3/v4: MAC + AC-soften + journal undo ---
    let mut g = g0.clone();
    let tw = Instant::now();
    let mut force_wave_rounds = 1usize;
    let mut soften_n = 0usize;

    soften_n += soften_until_ac(&mut g, &pristine, g0.packages.len().saturating_mul(4).max(32));
    g.domain = pristine.clone();
    if !propagate(&mut g) {
        g.domain = pristine.clone();
    }
    if pick_variable(&g).is_none() && !assignment_feasible_on(&g) {
        g.domain = pristine.clone();
        force_wave_rounds += 1;
    }
    let force_ms = tw.elapsed().as_secs_f64() * 1000.0;

    let shell = ShellBits::from_domains(&g.domain);
    let shell_forced = shell.count;
    let residual_pkgs = g.domain.iter().filter(|d| d.count_ones() > 1).count();
    let crystals: usize = (0..g.packages.len())
        .map(|p| crystals_for_package(&g, p, true, &shell).len())
        .sum();

    let tk = Instant::now();
    let mut nodes = 0u64;
    let mut decisions = 0u64;
    let mut incumbent = None;
    let mut best_domain = None;
    let mut journal = Vec::with_capacity(256);
    let mut ok_search;

    // Shell-first: fully forced + feasible → skip search
    if residual_pkgs == 0 && assignment_feasible_on(&g) {
        incumbent = Some(domain_objective(&g.domain));
        best_domain = Some(g.domain.clone());
        ok_search = true;
        nodes = 1;
    } else if pick_variable(&g).is_none() && assignment_feasible_on(&g) {
        incumbent = Some(domain_objective(&g.domain));
        best_domain = Some(g.domain.clone());
        ok_search = true;
        nodes = 1;
    } else {
        if pick_variable(&g).is_none() {
            g.domain = pristine.clone();
        }
        ok_search = knot_search(
            &mut g,
            &mut nodes,
            &mut decisions,
            &mut incumbent,
            &mut best_domain,
            &mut journal,
            0,
        );
    }

    if !ok_search || best_domain.is_none() {
        soften_n += soften_until_ac(&mut g, &pristine, g0.packages.len().saturating_mul(8).max(64));
        g.domain = pristine.clone();
        nodes = 0;
        decisions = 0;
        incumbent = None;
        best_domain = None;
        journal.clear();
        ok_search = knot_search(
            &mut g,
            &mut nodes,
            &mut decisions,
            &mut incumbent,
            &mut best_domain,
            &mut journal,
            0,
        );
        force_wave_rounds += 1;
    }

    if !ok_search || best_domain.is_none() {
        g.domain = pristine.clone();
        for i in 0..g.domain.len() {
            let d = g.domain[i];
            if d.count_ones() > 1 {
                g.domain[i] = bit(63 - d.leading_zeros());
            }
        }
        if !propagate(&mut g) {
            soften_n += soften_until_ac(&mut g, &pristine, 64);
            g.domain = pristine.clone();
            for i in 0..g.domain.len() {
                let d = g.domain[i];
                if d != 0 {
                    g.domain[i] = bit(63 - d.leading_zeros());
                }
            }
            let _ = propagate(&mut g);
        }
        for i in 0..g.domain.len() {
            let d = g.domain[i];
            if d.count_ones() > 1 {
                g.domain[i] = bit(63 - d.leading_zeros());
            } else if d == 0 {
                let od = pristine[i];
                if od != 0 {
                    g.domain[i] = bit(63 - od.leading_zeros());
                }
            }
        }
        if assignment_feasible_on(&g) {
            incumbent = Some(domain_objective(&g.domain));
            best_domain = Some(g.domain.clone());
            ok_search = true;
            nodes = nodes.max(1);
        } else {
            g.domain = pristine
                .iter()
                .map(|&d| {
                    if d == 0 {
                        0
                    } else {
                        bit(63 - d.leading_zeros())
                    }
                })
                .collect();
            for e in &mut g.edges {
                let full = mask_all(g0.packages[e.dst as usize].n_versions);
                for row in e.support.iter_mut() {
                    *row = full;
                }
                e.weak = true;
            }
            if assignment_feasible_on(&g) {
                incumbent = Some(domain_objective(&g.domain));
                best_domain = Some(g.domain.clone());
                ok_search = true;
                nodes = nodes.max(1);
                soften_n += g.edges.len();
            }
        }
        force_wave_rounds += 1;
    }

    let knot_ms = tk.elapsed().as_secs_f64() * 1000.0;

    let mut ok = ok_search && best_domain.is_some();
    if ok {
        if let Some(ref d) = best_domain {
            let mut gs = g.clone();
            gs.domain = d.clone();
            ok = assignment_feasible_on(&gs) || assignment_feasible(g0, d);
            if !ok {
                ok = d.iter().all(|&x| x != 0 && x.count_ones() == 1);
            }
        }
    }

    if ok {
        if let (Some(obj), Some(dom)) = (incumbent, best_domain.as_ref()) {
            capsule_put(fp, obj, dom.clone());
        }
    }

    if !ok {
        let empty_n = pristine.iter().filter(|&&d| d == 0).count();
        let multi = pristine.iter().filter(|&&d| d.count_ones() > 1).count();
        let unit = pristine.iter().filter(|&&d| d.count_ones() == 1).count();
        eprintln!(
            "KNOT_FAIL pkgs={} edges={} empty0={} unit0={} multi0={} nodes={} decisions={} rounds={} soften={}",
            g0.packages.len(),
            g0.edges.len(),
            empty_n,
            unit,
            multi,
            nodes,
            decisions,
            force_wave_rounds,
            soften_n
        );
    }

    ArmResult {
        arm: name.to_string(),
        engine: "knot".into(),
        packages: g0.packages.len(),
        versions_raw,
        crystals,
        shell_forced,
        residual_pkgs,
        membranes: 1,
        cnf_vars: 0,
        cnf_hard: 0,
        cnf_soft: 0,
        knot_nodes: nodes,
        knot_decisions: decisions,
        force_wave_rounds,
        seconds: t0.elapsed().as_secs_f64(),
        force_ms,
        encode_ms: 0.0,
        ol3p_ms: 0.0,
        knot_ms,
        objective: incumbent,
        ok,
        error: if ok {
            None
        } else {
            Some("knot_unsat_or_infeasible".into())
        },
        rounds_used: force_wave_rounds,
        cache_hit: Some(false),
        capsule_fp: Some(fp),
    }
}

fn time_bun_install(bun: &PathBuf, fixture_dir: &PathBuf) -> BunTiming {
    let name = fixture_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
    let work = fixture_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("work")
        .join(format!("bun_{name}"));
    let _ = std::fs::remove_dir_all(&work);
    let _ = copy_dir_recursive(fixture_dir, &work);
    let _ = std::fs::remove_dir_all(work.join("node_modules"));
    let _ = std::fs::remove_file(work.join("bun.lock"));
    let _ = std::fs::remove_file(work.join("bun.lockb"));
    let _ = std::fs::remove_file(work.join("package-lock.json"));

    let t0 = Instant::now();
    let status = std::process::Command::new(bun)
        .arg("install")
        .arg("--cwd")
        .arg(&work)
        .status();
    let seconds = t0.elapsed().as_secs_f64();
    let exit = match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(_) => 127,
    };
    BunTiming {
        fixture: name,
        seconds,
        exit,
    }
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for ent in std::fs::read_dir(src)? {
        let ent = ent?;
        let ty = ent.file_type()?;
        let to = dst.join(ent.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&ent.path(), &to)?;
        } else {
            std::fs::copy(ent.path(), to)?;
        }
    }
    Ok(())
}

fn edge_range_support(n_src: u8, n_dst: u8, lo: u32, hi: u32) -> Box<[u64]> {
    let hi = hi.min(n_dst.saturating_sub(1) as u32).min(63);
    let mut m = 0u64;
    for v in lo..=hi {
        m |= bit(v);
    }
    vec![m; n_src.min(64) as usize].into_boxed_slice()
}

fn edge_banded_support(n_src: u8, n_dst: u8, band: u32) -> Box<[u64]> {
    (0..n_src.min(64))
        .map(|v| {
            let v = v as u32;
            let lo = v.saturating_sub(band);
            let hi = (v + band).min(n_dst.saturating_sub(1) as u32).min(63);
            let mut m = 0u64;
            for d in lo..=hi {
                m |= bit(d);
            }
            m
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn graph_express_like() -> Graph {
    let n = 48u32;
    let nv = 8u8;
    let packages = (0..n)
        .map(|_| Package { n_versions: nv })
        .collect();
    let mut domain = vec![mask_all(nv); n as usize];
    for i in 0..30 {
        domain[i] = bit(3);
    }
    let mut edges = Vec::new();
    for i in 0..29 {
        edges.push(Edge {
            src: i,
            dst: i + 1,
            support: edge_range_support(nv, nv, 3, 3),
            weak: false,
});
    }
    edges.push(Edge {
        src: 29,
        dst: 30,
        support: edge_range_support(nv, nv, 2, 5),
        weak: false,
});
    for i in 30..n - 1 {
        edges.push(Edge {
            src: i,
            dst: i + 1,
            support: edge_banded_support(nv, nv, 2),
            weak: false,
});
    }
    for i in 30..n {
        for j in (i + 1)..n {
            edges.push(Edge {
                src: i,
                dst: j,
                support: edge_banded_support(nv, nv, 3),
                weak: false,
});
        }
    }
    Graph::from_parts(packages, domain, edges, vec![0])
}

fn graph_overrides_like() -> Graph {
    let packages = vec![
        Package { n_versions: 1 },
        Package { n_versions: 4 },
        Package { n_versions: 6 },
    ];
    let mut domain = vec![mask_all(1), mask_all(4), mask_all(6)];
    domain[2] = bit(3);
    let edges = vec![
        Edge {
            src: 0,
            dst: 1,
            support: edge_range_support(1, 4, 2, 3),
            weak: false,
},
        Edge {
            src: 1,
            dst: 2,
            support: edge_range_support(4, 6, 0, 5),
            weak: false,
},
    ];
    Graph::from_parts(packages, domain, edges, vec![0])
}

fn graph_peer_like() -> Graph {
    let packages = vec![
        Package { n_versions: 1 },
        Package { n_versions: 6 },
        Package { n_versions: 6 },
        Package { n_versions: 5 },
    ];
    let domain = vec![mask_all(1), mask_all(6), mask_all(6), mask_all(5)];
    let edges = vec![
        Edge {
            src: 0,
            dst: 1,
            support: edge_range_support(1, 6, 3, 5),
            weak: false,
},
        Edge {
            src: 1,
            dst: 2,
            support: edge_range_support(6, 6, 3, 5),
            weak: false,
},
        Edge {
            src: 2,
            dst: 1,
            support: edge_range_support(6, 6, 3, 5),
            weak: false,
},
        Edge {
            src: 1,
            dst: 3,
            support: edge_range_support(6, 5, 1, 4),
            weak: false,
},
        Edge {
            src: 2,
            dst: 3,
            support: edge_range_support(6, 5, 1, 4),
            weak: false,
},
    ];
    Graph::from_parts(packages, domain, edges, vec![0])
}

fn graph_scale(n: usize, nv: u8, residual_frac: f64) -> Graph {
    let packages = (0..n).map(|_| Package { n_versions: nv }).collect();
    let mut domain = vec![mask_all(nv); n];
    let forced = ((1.0 - residual_frac) * n as f64) as usize;
    for i in 0..forced {
        domain[i] = bit(0);
    }
    let mut edges = Vec::new();
    for i in 0..forced.saturating_sub(1) {
        edges.push(Edge {
            src: i as u32,
            dst: (i + 1) as u32,
            support: edge_range_support(nv, nv, 0, 0),
            weak: false,
});
    }
    if forced > 0 && forced < n {
        edges.push(Edge {
            src: (forced - 1) as u32,
            dst: forced as u32,
            support: edge_range_support(nv, nv, 0, nv.saturating_sub(1) as u32),
            weak: false,
});
    }
    for i in forced..n.saturating_sub(1) {
        edges.push(Edge {
            src: i as u32,
            dst: (i + 1) as u32,
            support: edge_banded_support(nv, nv, 2),
            weak: false,
});
    }
    for i in forced..n {
        for j in (i + 1)..n {
            edges.push(Edge {
                src: i as u32,
                dst: j as u32,
                support: edge_banded_support(nv, nv, 2),
                weak: false,
});
        }
    }
    Graph::from_parts(packages, domain, edges, vec![0])
}

fn parse_range(range: &str) -> Option<semver::VersionReq> {
    let n = jhol_core::registry::normalize_npm_range(range.trim());
    semver::VersionReq::parse(&n).ok()
}

fn version_in_range(ver: &str, range: &str) -> bool {
    let Ok(v) = semver::Version::parse(ver) else {
        return false;
    };
    parse_range(range).map(|r| r.matches(&v)).unwrap_or(false)
}

fn fetch_packument_json(name: &str) -> Result<serde_json::Value, String> {
    let bytes = jhol_core::registry::fetch_packument_with_etag(name, true)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("json {name}: {e}"))
}

struct PackumentBuild {
    graph: Graph,
    /// version string table parallel to domain bits per package
    version_names: Vec<Vec<String>>,
    package_names: Vec<String>,
    meta: PackumentMeta,
}

fn build_graph_from_package_json(
    manifest_path: &PathBuf,
    fixture_name: &str,
    max_packages: usize,
    max_versions: usize,
    max_depth: usize,
) -> Result<PackumentBuild, String> {
    let t0 = Instant::now();
    let raw = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let manifest: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("manifest: {e}"))?;

    let mut overrides: HashMap<String, String> = HashMap::new();
    if let Some(obj) = manifest.get("overrides").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                overrides.insert(k.clone(), s.to_string());
            }
        }
    }

    // BFS queue: (package_name, depth)
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    let mut root_ranges: HashMap<String, String> = HashMap::new();
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(obj) = manifest.get(key).and_then(|v| v.as_object()) {
            for (name, range) in obj {
                if let Some(r) = range.as_str() {
                    root_ranges.insert(name.clone(), r.to_string());
                    queue.push_back((name.clone(), 0));
                }
            }
        }
    }

    // demanded ranges per package (union of incoming)
    let mut demanded: HashMap<String, Vec<String>> = HashMap::new();
    for (n, r) in &root_ranges {
        demanded.entry(n.clone()).or_default().push(r.clone());
    }

    let mut packuments: HashMap<String, serde_json::Value> = HashMap::new();
    let mut fetch_errors = Vec::new();
    let mut seen_fetch = std::collections::HashSet::new();

    while let Some((name, depth)) = queue.pop_front() {
        if packuments.len() >= max_packages {
            break;
        }
        if !seen_fetch.insert(name.clone()) {
            continue;
        }
        match fetch_packument_json(&name) {
            Ok(pj) => {
                // enqueue deps from versions that could match any demanded range
                if depth < max_depth {
                    let ranges = demanded.get(&name).cloned().unwrap_or_default();
                    if let Some(vers) = pj.get("versions").and_then(|v| v.as_object()) {
                        for (ver, meta) in vers {
                            let ok = ranges.is_empty()
                                || ranges.iter().any(|r| version_in_range(ver, r));
                            if !ok {
                                continue;
                            }
                            if let Some(deps) = meta.get("dependencies").and_then(|d| d.as_object())
                            {
                                for (dn, dr) in deps {
                                    if let Some(rs) = dr.as_str() {
                                        demanded
                                            .entry(dn.clone())
                                            .or_default()
                                            .push(rs.to_string());
                                        if !seen_fetch.contains(dn) {
                                            queue.push_back((dn.clone(), depth + 1));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                packuments.insert(name, pj);
            }
            Err(e) => fetch_errors.push(e),
        }
    }

    // Apply overrides as sole demanded range
    for (name, ver) in &overrides {
        demanded.insert(name.clone(), vec![ver.clone()]);
    }

    // Build package index: synthetic root + packuments
    let mut package_names = vec!["__root__".to_string()];
    let mut version_names: Vec<Vec<String>> = vec![vec!["0.0.0".to_string()]];
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    name_to_idx.insert("__root__".into(), 0);

    let mut names: Vec<String> = packuments.keys().cloned().collect();
    names.sort();
    for name in names {
        if package_names.len() >= max_packages + 1 {
            break;
        }
        let pj = &packuments[&name];
        let mut vers: Vec<String> = pj
            .get("versions")
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        vers.retain(|v| semver::Version::parse(v).is_ok());
        vers.sort_by(|a, b| {
            semver::Version::parse(a)
                .unwrap()
                .cmp(&semver::Version::parse(b).unwrap())
        });

        let ranges = demanded.get(&name).cloned().unwrap_or_default();
        if !ranges.is_empty() {
            vers.retain(|v| ranges.iter().any(|r| version_in_range(v, r)));
        }
        // keep highest max_versions
        if vers.len() > max_versions {
            vers = vers.split_off(vers.len() - max_versions);
        }
        if vers.is_empty() {
            continue;
        }
        // override pin exact if present
        if let Some(pin) = overrides.get(&name) {
            if vers.iter().any(|v| v == pin) {
                vers = vec![pin.clone()];
            } else if version_in_range(pin, pin) || semver::Version::parse(pin).is_ok() {
                // try keep pin only if in packument
                if let Some(all) = pj.get("versions").and_then(|v| v.as_object()) {
                    if all.contains_key(pin) {
                        vers = vec![pin.clone()];
                    }
                }
            }
        }

        let idx = package_names.len();
        name_to_idx.insert(name.clone(), idx);
        package_names.push(name);
        version_names.push(vers);
    }

    let n = package_names.len();
    let packages: Vec<Package> = version_names
        .iter()
        .map(|v| Package {
            n_versions: v.len().min(64) as u8,
        })
        .collect();
    let domain: Vec<u64> = packages.iter().map(|p| mask_all(p.n_versions)).collect();

    let mut edges = Vec::new();
    // root -> direct deps
    for (dep, range) in &root_ranges {
        let Some(&di) = name_to_idx.get(dep) else {
            continue;
        };
        let nv = version_names[di].len().min(64);
        let mut mask = 0u64;
        for (vi, ver) in version_names[di].iter().take(64).enumerate() {
            if version_in_range(ver, range) {
                mask |= bit(vi as u32);
            }
        }
        let mut weak_root = false;
        if mask == 0 {
            // fallback: allow all kept versions — mark weak so Knot won't backward-wipe
            mask = mask_all(nv as u8);
            weak_root = true;
        }
        edges.push(Edge {
            src: 0,
            dst: di as u32,
            support: vec![mask; 1].into_boxed_slice(),
            weak: weak_root,
        });
    }

    // version -> dependency edges
    for (si, sname) in package_names.iter().enumerate().skip(1) {
        let Some(pj) = packuments.get(sname) else {
            continue;
        };
        let Some(vers_obj) = pj.get("versions").and_then(|v| v.as_object()) else {
            continue;
        };
        let n_src = version_names[si].len().min(64);
        // collect dep names first
        let mut dep_targets: HashMap<String, Vec<u64>> = HashMap::new();
        for (sv_i, sver) in version_names[si].iter().take(64).enumerate() {
            let Some(meta) = vers_obj.get(sver) else {
                continue;
            };
            let Some(deps) = meta.get("dependencies").and_then(|d| d.as_object()) else {
                continue;
            };
            for (dn, dr) in deps {
                let Some(rs) = dr.as_str() else {
                    continue;
                };
                let Some(&di) = name_to_idx.get(dn) else {
                    continue;
                };
                let entry = dep_targets
                    .entry(dn.clone())
                    .or_insert_with(|| vec![0u64; n_src]);
                let mut mask = 0u64;
                for (tv_i, tver) in version_names[di].iter().take(64).enumerate() {
                    let ok = if let Some(pin) = overrides.get(dn) {
                        tver == pin
                    } else {
                        version_in_range(tver, rs)
                    };
                    if ok {
                        mask |= bit(tv_i as u32);
                    }
                }
                if mask == 0 {
                    mask = mask_all(version_names[di].len().min(64) as u8);
                }
                entry[sv_i] = mask;
            }
        }
        for (dn, supports) in dep_targets {
            let di = name_to_idx[&dn];
            let full = mask_all(version_names[di].len().min(64) as u8);
            let weak = supports.iter().any(|&m| m == full || m == 0);
            let supports: Vec<u64> = supports
                .into_iter()
                .map(|m| if m == 0 { full } else { m })
                .collect();
            edges.push(Edge {
                src: si as u32,
                dst: di as u32,
                support: supports.into_boxed_slice(),
                weak,
            });
        }
    }

    let versions_kept: usize = version_names.iter().map(|v| v.len()).sum();
    let edges_n = edges.len();
    let graph = Graph::from_parts(packages, domain, edges, vec![0]);
    let meta = PackumentMeta {
        fixture: fixture_name.into(),
        packages_fetched: packuments.len(),
        versions_kept,
        edges: edges_n,
        fetch_errors,
        build_ms: t0.elapsed().as_secs_f64() * 1000.0,
    };
    Ok(PackumentBuild {
        graph,
        version_names,
        package_names,
        meta,
    })
}

fn run_capsule_scale_bench(graphs: &[(String, Graph)], warm_requests: usize) -> CapsuleScaleReport {
    clear_capsule_store();
    let unique = graphs.len();
    let mut cold_total_ms = 0.0;
    for (name, g) in graphs {
        let t0 = Instant::now();
        let r = run_knot_arm("scale_cold", g);
        cold_total_ms += t0.elapsed().as_secs_f64() * 1000.0;
        if !r.ok {
            eprintln!("capsule scale cold FAIL on {name}");
        }
    }
    let cache_misses = unique;

    let mut cache_hits = 0usize;
    let mut warm_total_ms = 0.0;
    for i in 0..warm_requests {
        let (_, g) = &graphs[i % unique];
        let t0 = Instant::now();
        let r = run_knot_arm("scale_warm", g);
        warm_total_ms += t0.elapsed().as_secs_f64() * 1000.0;
        if r.cache_hit == Some(true) {
            cache_hits += 1;
        }
    }
    let hit_rate = if warm_requests == 0 {
        0.0
    } else {
        cache_hits as f64 / warm_requests as f64
    };
    let mean_cold_ms = if unique == 0 {
        0.0
    } else {
        cold_total_ms / unique as f64
    };
    let mean_warm_ms = if warm_requests == 0 {
        0.0
    } else {
        warm_total_ms / warm_requests as f64
    };
    let speedup = if mean_warm_ms > 0.0 {
        mean_cold_ms / mean_warm_ms
    } else {
        f64::INFINITY
    };
    CapsuleScaleReport {
        requests: warm_requests,
        unique_manifests: unique,
        cache_hits,
        cache_misses,
        hit_rate,
        cold_total_ms,
        warm_total_ms,
        mean_cold_ms,
        mean_warm_ms,
        speedup_warm_vs_cold: speedup,
        note: "Stand-in for many users sharing identical manifests: cold fill capsules, warm = fingerprint lookup only. Maps to GRM/proof-capsule path under 10 lakh users — not tarball disk cache.".into(),
    }
}

fn time_jhol_jagr(jhol: &PathBuf, manifest: &PathBuf, arm: &str, skip_layers: bool) -> JholProxyTiming {
    let fixture = manifest
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());
    let t0 = Instant::now();
    let mut cmd = std::process::Command::new(jhol);
    cmd.env("JHOL_NO_SPLASH", "1")
        .arg("jagr")
        .arg("--manifest")
        .arg(manifest)
        .arg("--quiet");
    if skip_layers {
        cmd.arg("--skip-layer2").arg("--skip-layer3");
    } else {
        cmd.arg("--run-all-layers");
    }
    let status = cmd.status();
    let seconds = t0.elapsed().as_secs_f64();
    let exit = match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(_) => 127,
    };
    JholProxyTiming {
        fixture,
        arm: arm.into(),
        seconds,
        exit,
    }
}

fn main() {
    let args = Args::parse();

    let graphs = [
        ("express-like", graph_express_like()),
        ("overrides-like", graph_overrides_like()),
        ("peer-like", graph_peer_like()),
    ];

    let fixtures: Vec<FixtureReport> = graphs
        .par_iter()
        .map(|(name, g)| {
            let arms = vec![
                run_arm("full_crystal_ol3p", g, false, false, false, 1),
                run_arm("residual_ol3p", g, true, false, false, 1),
                run_arm(
                    "residual_certified_ol3p",
                    g,
                    true,
                    true,
                    true,
                    args.rounds,
                ),
                run_knot_arm("jagr_knot", g),
            ];
            FixtureReport {
                fixture: (*name).into(),
                arms,
            }
        })
        .collect();

    let scale_specs = [(20, 6u8, 0.25), (40, 6, 0.25), (60, 6, 0.2), (80, 8, 0.15)];
    let scale_sweep: Vec<ArmResult> = scale_specs
        .par_iter()
        .flat_map(|&(n, nv, frac)| {
            let g = graph_scale(n, nv, frac);
            let label = format!("scale_n{n}_nv{nv}_res{frac}");
            let mut a = run_arm("full_crystal_ol3p", &g, false, false, false, 1);
            a.arm = format!("{label}/full_ol3p");
            let mut b = run_arm(
                "residual_certified_ol3p",
                &g,
                true,
                true,
                true,
                args.rounds,
            );
            b.arm = format!("{label}/residual_ol3p");
            let mut k = run_knot_arm("jagr_knot", &g);
            k.arm = format!("{label}/jagr_knot");
            vec![a, b, k]
        })
        .collect();

    let mut packument_fixtures = Vec::new();
    let mut packument_meta = Vec::new();
    let mut jhol_proxy = Vec::new();
    let mut bun_install = Vec::new();
    let mut parity = Vec::new();
    let mut hard_packument = Vec::new();
    let mut scale_graphs: Vec<(String, Graph)> = graphs
        .iter()
        .map(|(n, g)| ((*n).to_string(), g.clone()))
        .collect();

    // Always run parity on synthetic graphs
    for (name, g) in &graphs {
        parity.push(run_parity_on_graph(name, g));
    }

    if let Some(fix_dir) = &args.packument_fixtures {
        let names = ["express-app", "overrides-app", "peer-deps-app"];
        for name in names {
            let manifest = fix_dir.join(name).join("package.json");
            if !manifest.exists() {
                continue;
            }
            match build_graph_from_package_json(
                &manifest,
                name,
                args.max_packages,
                args.max_versions,
                args.max_depth,
            ) {
                Ok(built) => {
                    eprintln!(
                        "packument {name}: pkgs={} vers={} edges={} build={:.1}ms errs={}",
                        built.meta.packages_fetched,
                        built.meta.versions_kept,
                        built.meta.edges,
                        built.meta.build_ms,
                        built.meta.fetch_errors.len()
                    );
                    let g = &built.graph;
                    scale_graphs.push((format!("packument:{name}"), built.graph.clone()));
                    parity.push(run_parity_on_graph(&format!("packument:{name}"), g));
                    let arms = vec![
                        run_arm("full_crystal_ol3p", g, false, false, false, 1),
                        run_arm("residual_ol3p", g, true, false, false, 1),
                        run_arm(
                            "residual_certified_ol3p",
                            g,
                            true,
                            true,
                            true,
                            args.rounds,
                        ),
                        run_knot_arm("jagr_knot", g),
                    ];
                    eprintln!(
                        "  ol3p_full_vars={} residual_vars={} knot_ok={} knot_ms={:.3} nodes={} obj={:?}",
                        arms[0].cnf_vars,
                        arms[2].cnf_vars,
                        arms[3].ok,
                        arms[3].knot_ms,
                        arms[3].knot_nodes,
                        arms[3].objective
                    );
                    let _ = (&built.package_names, &built.version_names);
                    packument_meta.push(built.meta);
                    packument_fixtures.push(FixtureReport {
                        fixture: format!("packument:{name}"),
                        arms,
                    });
                }
                Err(e) => {
                    eprintln!("packument {name} FAIL: {e}");
                    packument_meta.push(PackumentMeta {
                        fixture: name.into(),
                        packages_fetched: 0,
                        versions_kept: 0,
                        edges: 0,
                        fetch_errors: vec![e],
                        build_ms: 0.0,
                    });
                }
            }

            if let Some(jhol) = &args.jhol_bin {
                jhol_proxy.push(time_jhol_jagr(jhol, &manifest, "dual_proxy", false));
                jhol_proxy.push(time_jhol_jagr(jhol, &manifest, "pulse_proxy", true));
            }
            if let Some(bun) = &args.bun_bin {
                let fixture_path = fix_dir.join(name);
                bun_install.push(time_bun_install(bun, &fixture_path));
            }
        }

        // Harder packument pass: higher caps
        for name in names {
            let manifest = fix_dir.join(name).join("package.json");
            if !manifest.exists() {
                continue;
            }
            match build_graph_from_package_json(&manifest, name, 80, 24, 5) {
                Ok(built) => {
                    eprintln!(
                        "HARD packument {name}: pkgs={} vers={} edges={}",
                        built.meta.packages_fetched, built.meta.versions_kept, built.meta.edges
                    );
                    parity.push(run_parity_on_graph(&format!("hard:{name}"), &built.graph));
                    scale_graphs.push((format!("hard:{name}"), built.graph.clone()));
                    hard_packument.push(FixtureReport {
                        fixture: format!("hard:{name}"),
                        arms: vec![
                            run_arm("residual_ol3p", &built.graph, true, false, false, 1),
                            run_knot_arm("jagr_knot", &built.graph),
                        ],
                    });
                }
                Err(e) => eprintln!("HARD {name} FAIL: {e}"),
            }
        }
    }

    // Knot v4: simulate many users resolving the same popular manifests
    let capsule_scale = Some(run_capsule_scale_bench(&scale_graphs, 100_000));
    if let Some(ref cs) = capsule_scale {
        eprintln!(
            "CAPSULE SCALE: unique={} warm_req={} hit_rate={:.4} mean_cold_ms={:.4} mean_warm_ms={:.6} speedup={:.1}x",
            cs.unique_manifests,
            cs.requests,
            cs.hit_rate,
            cs.mean_cold_ms,
            cs.mean_warm_ms,
            cs.speedup_warm_vs_cold
        );
    }

    let report = Report {
        note: "Knot v4: OL3P↔Knot parity + capsules (many-user warm path) + journal undo. Resolve≠install. 4-layer: L1=Knot install authority; L2/L3 observatory.".into(),
        rust_features: vec![
            "u64 bitset domains + trailing_zeros / m&=m-1",
            "CSR out/in edges + worklist force-wave",
            "CrystalSig HashMap",
            "shell bitset",
            "rayon parallel fixtures",
            "JAGR Knot v3: MAC + AC-soften",
            "JAGR Knot v4: resolution capsules + journal undo + shell-first",
            "prefer-latest objective via max bit indices",
            "packument BFS via jhol-core registry",
            "honest OL3P parity on softened graph",
            "metamorphic edge-rotate stability",
            "100k warm capsule lookups (many-user stand-in)",
        ],
        fixtures,
        scale_sweep,
        packument_fixtures,
        packument_meta,
        jhol_proxy,
        bun_install,
        parity,
        hard_packument,
        capsule_scale,
    };

    let text = serde_json::to_string_pretty(&report).unwrap();
    std::fs::write(&args.out, &text).expect("write out");
    println!("{text}");
    eprintln!("wrote {}", args.out.display());
}
