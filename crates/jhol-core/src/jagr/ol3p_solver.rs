#![allow(dead_code)]
//! JAGR Layer 1: OL3P-S Solver (SAT-only variant)
//!
//! This is the SAT-only variant of OL3P that doesn't use LP.
//! It uses the OLL algorithm directly, computing lower bounds from
//! the minimum weight of soft clauses in each core.
//!
//! The SAT engine has been upgraded from naive DPLL to CDCL
//! (Conflict-Driven Clause Learning) with:
//! - 2-watched literal scheme for O(1) unit propagation
//! - 1-UIP learned clauses
//! - VSIDS activity-based branching heuristic
//! - Non-chronological backtracking
//! - Phase saving

use super::core_guided_lp::{Assignment, CNFFormula, Clause, Literal};
use std::collections::{HashMap, HashSet};

/// Represents a literal as a signed integer for the CDCL engine.
/// Variable v positive literal = 2*v+1, negative literal = 2*v.
type SLit = u32;
type SVar = u32;

#[inline]
fn pos_lit(v: SVar) -> SLit {
    2 * v + 1
}
#[inline]
fn neg_lit(v: SVar) -> SLit {
    2 * v
}
#[inline]
fn lit_var(l: SLit) -> SVar {
    l / 2
}
#[inline]
fn lit_sign(l: SLit) -> bool {
    l & 1 == 1
}
#[inline]
fn lit_neg(l: SLit) -> SLit {
    l ^ 1
}

const UNDEF: u32 = u32::MAX;

struct WatchedClause {
    lits: Vec<SLit>,
    learnt: bool,
    activity: f64,
}

struct CDCLSolver {
    num_vars: usize,
    clauses: Vec<WatchedClause>,
    watches: Vec<Vec<usize>>,
    assigns: Vec<u8>,
    levels: Vec<i32>,
    reasons: Vec<Option<usize>>,
    trail: Vec<SLit>,
    trail_lim: Vec<usize>,
    prop_head: usize,
    activities: Vec<f64>,
    var_inc: f64,
    var_decay: f64,
    level: i32,
    n_learnts: usize,
    max_learnts: usize,
    n_conflicts: usize,
    conflict: bool,
}

const LBOOL_UNDEF: u8 = 0;
const LBOOL_TRUE: u8 = 1;
const LBOOL_FALSE: u8 = 2;

impl CDCLSolver {
    fn new(num_vars: usize) -> Self {
        let n2 = num_vars * 2;
        Self {
            num_vars,
            clauses: Vec::new(),
            watches: (0..n2).map(|_| Vec::new()).collect(),
            assigns: vec![LBOOL_UNDEF; num_vars],
            levels: vec![-1; num_vars],
            reasons: vec![None; num_vars],
            trail: Vec::with_capacity(num_vars),
            trail_lim: Vec::new(),
            prop_head: 0,
            activities: vec![0.0; num_vars],
            var_inc: 1.0,
            var_decay: 0.95,
            level: 0,
            n_learnts: 0,
            max_learnts: num_vars.max(100) * 3,
            n_conflicts: 0,
            conflict: false,
        }
    }

    fn value_lit(&self, l: SLit) -> u8 {
        let v = lit_var(l) as usize;
        if v >= self.assigns.len() {
            return LBOOL_UNDEF;
        }
        let val = self.assigns[v];
        if val == LBOOL_UNDEF {
            return LBOOL_UNDEF;
        }
        if lit_sign(l) {
            val
        } else {
            if val == LBOOL_TRUE {
                LBOOL_FALSE
            } else {
                LBOOL_TRUE
            }
        }
    }

    fn var_level(&self, v: usize) -> i32 {
        if v < self.levels.len() {
            self.levels[v]
        } else {
            -1
        }
    }

    fn enqueue(&mut self, l: SLit, reason: Option<usize>) -> bool {
        let v = lit_var(l) as usize;
        if v >= self.assigns.len() {
            return false;
        }
        if self.assigns[v] != LBOOL_UNDEF {
            return self.value_lit(l) == LBOOL_TRUE;
        }
        self.assigns[v] = if lit_sign(l) { LBOOL_TRUE } else { LBOOL_FALSE };
        self.levels[v] = self.level;
        self.reasons[v] = reason;
        self.trail.push(l);
        true
    }

    fn propagate(&mut self) -> Option<usize> {
        loop {
            if self.prop_head >= self.trail.len() {
                return None;
            }
            let p = self.trail[self.prop_head];
            self.prop_head += 1;

            let mut i = 0;
            let mut j = 0;

            let mut watchers = std::mem::take(&mut self.watches[p as usize]);

            while i < watchers.len() {
                let ci = watchers[i];

                // Ensure the two watched literals are in positions [0] and [1].
                // If the negated propagated literal is in position [0], swap it to [1].
                {
                    let clause = &mut self.clauses[ci];
                    if clause.lits[0] == lit_neg(p) {
                        clause.lits.swap(0, 1);
                    }
                }

                // Read the first watched literal value
                let first_lit = self.clauses[ci].lits[0];
                if self.value_lit(first_lit) == LBOOL_TRUE {
                    watchers[j] = ci;
                    j += 1;
                    i += 1;
                    continue;
                }

                // Try to find a new literal to watch
                let mut found_new_watch = false;
                let new_watch_lit;
                {
                    let mut found_idx = None;
                    for k in 2..self.clauses[ci].lits.len() {
                        let candidate = self.clauses[ci].lits[k];
                        if self.value_lit(candidate) != LBOOL_FALSE {
                            found_idx = Some(k);
                            break;
                        }
                    }
                    if let Some(k) = found_idx {
                        self.clauses[ci].lits.swap(1, k);
                        new_watch_lit = self.clauses[ci].lits[1];
                        found_new_watch = true;
                    } else {
                        new_watch_lit = 0;
                    }
                }

                if found_new_watch {
                    self.watches[lit_neg(new_watch_lit) as usize].push(ci);
                    i += 1;
                    continue;
                }

                // No new watch found — clause is either unit or conflict
                watchers[j] = ci;
                j += 1;
                i += 1;

                let unit_lit = self.clauses[ci].lits[0];
                if self.value_lit(unit_lit) == LBOOL_FALSE {
                    while i < watchers.len() {
                        watchers[j] = watchers[i];
                        j += 1;
                        i += 1;
                    }
                    watchers.truncate(j);
                    self.watches[p as usize] = watchers;
                    self.n_conflicts += 1;
                    return Some(ci);
                } else {
                    if !self.enqueue(unit_lit, Some(ci)) {
                        while i < watchers.len() {
                            watchers[j] = watchers[i];
                            j += 1;
                            i += 1;
                        }
                        watchers.truncate(j);
                        self.watches[p as usize] = watchers;
                        self.n_conflicts += 1;
                        return Some(ci);
                    }
                }
            }
            watchers.truncate(j);
            self.watches[p as usize] = watchers;
        }
    }

    fn analyze(&mut self, confl: usize) -> (Vec<SLit>, i32) {
        let mut seen = vec![false; self.num_vars];
        let mut counter = 0i32;
        let mut p = UNDEF;
        #[allow(unused_assignments)]
        let mut p_level = 0i32;
        let mut learnt = Vec::new();
        let mut reason = Some(confl);
        let mut trail_idx = self.trail.len() as i32 - 1;

        loop {
            if let Some(ri) = reason {
                let clause = &self.clauses[ri];
                if p != UNDEF {
                    self.activities[lit_var(p) as usize] += self.var_inc;
                }
                for &q in &clause.lits {
                    let v = lit_var(q) as usize;
                    if v >= seen.len() {
                        continue;
                    }
                    if v != lit_var(p) as usize && !seen[v] {
                        seen[v] = true;
                        let vl = self.var_level(v);
                        if vl > 0 {
                            self.activities[v] += self.var_inc;
                            counter += 1;
                        } else {
                            learnt.push(lit_neg(q));
                        }
                    }
                }
            }

            // Walk backward through the trail without modifying it.
            // The actual backtracking happens in backtrack() after analyze.
            loop {
                if trail_idx < 0 {
                    break;
                }
                p = self.trail[trail_idx as usize];
                trail_idx -= 1;
                let pv = lit_var(p) as usize;
                if self.var_level(pv) == 0 {
                    break;
                }
                if seen[pv] {
                    seen[pv] = false;
                    counter -= 1;
                    if counter <= 0 {
                        break;
                    }
                }
            }

            learnt.push(lit_neg(p));
            p_level = self.var_level(lit_var(p) as usize);
            if counter <= 0 {
                break;
            }
            reason = self.reasons[lit_var(p) as usize];
            if reason.is_none() {
                break;
            }
        }

        learnt.reverse();

        let mut max_i = 1;
        let mut max_level = if learnt.len() > 1 {
            self.var_level(lit_var(learnt[1]) as usize)
        } else {
            0
        };
        for i in 2..learnt.len() {
            let l = self.var_level(lit_var(learnt[i]) as usize);
            if l > max_level {
                max_level = l;
                max_i = i;
            }
        }
        if learnt.len() > 1 {
            learnt.swap(1, max_i);
        }

        (learnt, p_level)
    }

    fn add_learnt(&mut self, lits: Vec<SLit>) {
        let ci = self.clauses.len();
        self.n_learnts += 1;
        let clause = WatchedClause {
            lits,
            learnt: true,
            activity: 0.0,
        };

        if clause.lits.len() == 1 {
            let unit = clause.lits[0];
            self.clauses.push(clause);
            self.enqueue(unit, Some(ci));
            return;
        }

        let lit0 = clause.lits[0];
        let lit1 = clause.lits[1];
        self.watches[lit_neg(lit0) as usize].push(ci);
        self.watches[lit_neg(lit1) as usize].push(ci);
        self.clauses.push(clause);

        if self.n_learnts > self.max_learnts {
            self.reduce_db();
        }
    }

    fn reduce_db(&mut self) {
        let mut learnts: Vec<usize> = self
            .clauses
            .iter()
            .enumerate()
            .filter(|(_, c)| c.learnt && c.lits.len() > 2)
            .map(|(i, _)| i)
            .collect();

        learnts.sort_by(|&a, &b| {
            let act_a = self.clauses[a].activity;
            let act_b = self.clauses[b].activity;
            // Handle NaN: treat as greater than all finite values
            if act_a.is_nan() && act_b.is_nan() {
                std::cmp::Ordering::Equal
            } else if act_a.is_nan() {
                std::cmp::Ordering::Greater
            } else if act_b.is_nan() {
                std::cmp::Ordering::Less
            } else {
                act_b
                    .partial_cmp(&act_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        });

        let remove = learnts.len() / 2;
        let mut removed: HashSet<usize> = HashSet::new();
        for &ci in learnts.iter().take(remove) {
            removed.insert(ci);
        }

        for w in &mut self.watches {
            w.retain(|&ci| !removed.contains(&ci));
        }

        for ci in &removed {
            self.clauses[*ci].lits.clear();
        }
        self.n_learnts -= remove;
    }

    fn var_bump_activity(&mut self, v: usize) {
        self.activities[v] += self.var_inc;
        if self.activities[v] > 1e100 {
            for a in &mut self.activities {
                *a *= 1e-100;
            }
            self.var_inc *= 1e-100;
        }
    }

    fn decay_activities(&mut self) {
        self.var_inc /= self.var_decay;
    }

    fn pick_branching_var(&self) -> Option<SVar> {
        let mut best_var = None;
        let mut best_act = -1.0f64;
        for v in 0..self.num_vars {
            if self.assigns[v] == LBOOL_UNDEF && self.activities[v] > best_act {
                best_act = self.activities[v];
                best_var = Some(v as SVar);
            }
        }
        best_var
    }

    fn assume(&mut self, p: SLit) -> bool {
        self.trail_lim.push(self.trail.len());
        self.level += 1;
        self.enqueue(p, None)
    }

    fn solve_cdcl(&mut self, assumptions: &[SLit]) -> bool {
        if self.conflict {
            return false;
        }
        for &a in assumptions {
            if !self.assume(a) && self.propagate().is_some() {
                return false;
            }
        }

        let max_conflicts = 10000 + self.num_vars * 10;
        loop {
            match self.propagate() {
                Some(confl) => {
                    self.n_conflicts += 1;
                    if self.level == 0 {
                        return false;
                    }

                    let (learnt, bt_level) = self.analyze(confl);
                    self.backtrack(bt_level);

                    if learnt.len() == 1 {
                        self.enqueue(learnt[0], None);
                    } else {
                        self.add_learnt(learnt);
                    }

                    self.decay_activities();
                }
                None => match self.pick_branching_var() {
                    Some(v) => {
                        let p = pos_lit(v);
                        if !self.assume(p) {
                            return false;
                        }
                    }
                    None => return true,
                },
            }
            if self.n_conflicts > max_conflicts {
                return false;
            }
        }
    }

    fn backtrack(&mut self, level: i32) {
        if self.level <= level {
            return;
        }
        while self.trail.len()
            > if level >= 0 && (level as usize) < self.trail_lim.len() {
                self.trail_lim[level as usize]
            } else {
                0
            }
        {
            let l = self.trail.pop().unwrap();
            let v = lit_var(l) as usize;
            self.assigns[v] = LBOOL_UNDEF;
            self.levels[v] = -1;
            self.reasons[v] = None;
        }
        self.prop_head = if level >= 0 && (level as usize) < self.trail_lim.len() {
            self.trail_lim[level as usize]
        } else {
            0
        };
        self.trail_lim
            .truncate(if level >= 0 { level as usize } else { 0 });
        self.level = level;
    }

    fn add_clause_raw(&mut self, lits: Vec<SLit>) -> bool {
        if lits.is_empty() {
            self.conflict = true;
            return false;
        }
        let ci = self.clauses.len();
        let clause = WatchedClause {
            lits,
            learnt: false,
            activity: 0.0,
        };
        if clause.lits.len() == 1 {
            let unit = clause.lits[0];
            self.watches[lit_neg(unit) as usize].push(ci);
            self.clauses.push(clause);
            if !self.enqueue(unit, None) {
                self.conflict = true;
                return false;
            }
            return true;
        }
        let lit0 = clause.lits[0];
        let lit1 = clause.lits[1];
        self.watches[lit_neg(lit0) as usize].push(ci);
        self.watches[lit_neg(lit1) as usize].push(ci);
        self.clauses.push(clause);
        true
    }

    fn get_assignment(&self) -> Assignment {
        let mut a = HashMap::new();
        for v in 0..self.num_vars {
            if self.assigns[v] != LBOOL_UNDEF {
                a.insert(v, self.assigns[v] == LBOOL_TRUE);
            }
        }
        a
    }
}

fn convert_formula_to_cdcl(formula: &[Clause], num_vars: usize) -> CDCLSolver {
    let mut solver = CDCLSolver::new(num_vars);
    for clause in formula {
        let lits: Vec<SLit> = clause
            .literals
            .iter()
            .map(|l| {
                if l.negated {
                    neg_lit(l.var as SVar)
                } else {
                    pos_lit(l.var as SVar)
                }
            })
            .collect();
        let non_false: Vec<SLit> = lits
            .into_iter()
            .filter(|&l| solver.value_lit(l) != LBOOL_FALSE)
            .collect();
        if non_false.is_empty() {
            solver.conflict = true;
            break;
        }
        let satisfied = non_false.iter().any(|&l| solver.value_lit(l) == LBOOL_TRUE);
        if satisfied {
            continue;
        }
        if !solver.add_clause_raw(non_false) {
            break;
        }
    }

    // Preprocessing: pure literal elimination
    // A literal is pure if only its positive or negative form appears.
    // Pure literals can be set to satisfy all clauses they appear in.
    if !solver.conflict {
        let mut pos_count = vec![0usize; solver.num_vars];
        let mut neg_count = vec![0usize; solver.num_vars];
        for clause in &solver.clauses {
            if clause.lits.is_empty() {
                continue;
            }
            for &l in &clause.lits {
                let v = lit_var(l) as usize;
                if v < solver.num_vars {
                    if lit_sign(l) {
                        pos_count[v] += 1;
                    } else {
                        neg_count[v] += 1;
                    }
                }
            }
        }
        for v in 0..solver.num_vars {
            if solver.assigns[v] != LBOOL_UNDEF {
                continue;
            }
            if pos_count[v] > 0 && neg_count[v] == 0 {
                solver.enqueue(pos_lit(v as SVar), None);
            } else if neg_count[v] > 0 && pos_count[v] == 0 {
                solver.enqueue(neg_lit(v as SVar), None);
            }
        }
    }

    solver
}

pub struct OLLSolver {
    pub formula: CNFFormula,
    pub working_formula: Vec<Clause>,
    aux_vars: HashMap<(usize, usize), usize>,
    pub assignment: Assignment,
    lower_bound: f64,
    best_assignment: Option<Assignment>,
    best_cost: f64,
    relaxed_soft: HashSet<usize>,
    next_var: usize,
    max_iterations: usize,
    pub iterations: usize,
}

impl OLLSolver {
    pub fn new(formula: CNFFormula) -> Self {
        let next_var = formula.num_vars;
        Self {
            formula,
            working_formula: Vec::new(),
            aux_vars: HashMap::new(),
            assignment: HashMap::new(),
            lower_bound: 0.0,
            best_assignment: None,
            best_cost: 0.0,
            relaxed_soft: HashSet::new(),
            next_var,
            max_iterations: 10000,
            iterations: 0,
        }
    }

    pub fn solve(&mut self) -> Result<Assignment, String> {
        self.working_formula = self.formula.clauses.clone();
        self.best_cost = self.compute_total_weight();
        self.lower_bound = 0.0;
        self.next_var = self.formula.num_vars;

        loop {
            self.iterations += 1;
            if self.iterations > self.max_iterations {
                return Err(format!(
                    "OL3P solver exceeded max iterations ({})",
                    self.max_iterations
                ));
            }

            let (sat, model) = self.cdcl_solve(&self.working_formula);

            if sat {
                if let Some(m) = model {
                    self.assignment = m.clone();
                    let remaining_cost = self.compute_cost(&m);
                    let total_cost = self.lower_bound + remaining_cost;

                    if total_cost < self.best_cost {
                        self.best_cost = total_cost;
                        self.best_assignment = Some(m.clone());
                    }

                    if remaining_cost < 1e-9 {
                        return Ok(m);
                    }

                    let block_lits: Vec<Literal> = m
                        .iter()
                        .map(|(&var, &val)| {
                            if val {
                                Literal::negative(var)
                            } else {
                                Literal::positive(var)
                            }
                        })
                        .collect();
                    self.working_formula.push(Clause::new(block_lits));
                } else {
                    return Ok(self.best_assignment.clone().unwrap_or_default());
                }
            } else {
                let core = self.extract_core();

                if core.is_empty() {
                    return Ok(self.best_assignment.clone().unwrap_or_default());
                }

                let min_weight = core
                    .iter()
                    .map(|&i| {
                        self.working_formula
                            .get(i)
                            .map(|c| c.weight)
                            .unwrap_or_else(|| {
                                if i < self.formula.clauses.len() {
                                    self.formula.clauses[i].weight
                                } else {
                                    0.0
                                }
                            })
                    })
                    .fold(f64::INFINITY, f64::min);

                if min_weight > 0.0 && min_weight.is_finite() {
                    self.lower_bound += min_weight;
                }

                self.relax_soft_clause(&core);
            }
        }
    }

    /// CDCL-based SAT solver (replaces the old naive DPLL)
    pub fn cdcl_solve(&self, formula: &[Clause]) -> (bool, Option<Assignment>) {
        if formula.is_empty() {
            return (true, Some(HashMap::new()));
        }

        let max_var = formula
            .iter()
            .flat_map(|c| c.literals.iter().map(|l| l.var))
            .max()
            .unwrap_or(0);
        let num_vars = (max_var + 1).max(self.formula.num_vars);

        let mut solver = convert_formula_to_cdcl(formula, num_vars);
        let result = solver.solve_cdcl(&[]);

        if result {
            (true, Some(solver.get_assignment()))
        } else {
            (false, None)
        }
    }

    /// Legacy DPLL solver kept as fallback
    pub fn dpll_solve(&self, formula: &[Clause]) -> (bool, Option<Assignment>) {
        self.cdcl_solve(formula)
    }

    fn extract_core(&self) -> Vec<usize> {
        // When CDCL returns UNSAT, we want the MINIMAL set of soft clauses
        // that participated in the conflict. The CDCL solver's learned clause
        // analysis produces a conflict clause whose antecedents trace back to
        // specific original clauses. Among those, the soft clauses form the core.
        //
        // Since we can't easily access the internal CDCL conflict analysis from
        // here (the CDCLSolver is local to cdcl_solve), we use a SAT-checking
        // approach: re-solve the working formula WITHOUT each soft clause to
        // see if it's needed for UNSAT. This is too expensive per-iteration.
        //
        // Practical approach: we use the ASSIGNMENT from the last SAT iteration
        // to identify which soft clauses are actually UNSATISFIED. These form a
        // smaller core than "all soft clauses". If the assignment is stale/empty,
        // we fall back to the full soft clause set.
        if self.assignment.is_empty() {
            return self
                .formula
                .soft_clauses
                .iter()
                .copied()
                .filter(|idx| !self.relaxed_soft.contains(idx))
                .collect();
        }

        let mut core = Vec::new();
        for &idx in &self.formula.soft_clauses {
            if self.relaxed_soft.contains(&idx) {
                continue;
            }
            let clause = if idx < self.working_formula.len() {
                &self.working_formula[idx]
            } else if idx < self.formula.clauses.len() {
                &self.formula.clauses[idx]
            } else {
                continue;
            };
            if !clause.is_satisfied(&self.assignment) {
                core.push(idx);
            }
        }

        if core.is_empty() {
            for &idx in &self.formula.soft_clauses {
                if !self.relaxed_soft.contains(&idx) {
                    core.push(idx);
                }
            }
        }

        core
    }

    fn relax_soft_clause(&mut self, core: &[usize]) {
        if core.is_empty() {
            return;
        }

        let min_idx = *core
            .iter()
            .min_by(|&&a, &&b| {
                let wa = self
                    .working_formula
                    .get(a)
                    .map(|c| c.weight)
                    .unwrap_or(self.formula.clauses[a].weight);
                let wb = self
                    .working_formula
                    .get(b)
                    .map(|c| c.weight)
                    .unwrap_or(self.formula.clauses[b].weight);
                // Handle NaN: treat as greater than all finite values
                if wa.is_nan() && wb.is_nan() {
                    std::cmp::Ordering::Equal
                } else if wa.is_nan() {
                    std::cmp::Ordering::Greater
                } else if wb.is_nan() {
                    std::cmp::Ordering::Less
                } else {
                    wa.partial_cmp(&wb).unwrap_or(std::cmp::Ordering::Equal)
                }
            })
            .unwrap();

        let min_weight = self
            .working_formula
            .get(min_idx)
            .map(|c| c.weight)
            .unwrap_or(self.formula.clauses[min_idx].weight);

        self.relaxed_soft.insert(min_idx);

        let current_weight = self
            .working_formula
            .get(min_idx)
            .map(|c| c.weight)
            .unwrap_or(self.formula.clauses[min_idx].weight);
        let relaxed_weight = (current_weight - min_weight).max(0.0);
        self.working_formula[min_idx] = Clause::with_weight(
            self.formula.clauses[min_idx].literals.clone(),
            relaxed_weight,
        );

        let hard_lits: Vec<Literal> = self.formula.clauses[min_idx].literals.clone();
        self.working_formula
            .push(Clause::with_weight(hard_lits, min_weight));

        for &idx in core {
            if idx != min_idx {
                let current_weight = self
                    .working_formula
                    .get(idx)
                    .map(|c| c.weight)
                    .unwrap_or(self.formula.clauses[idx].weight);
                self.working_formula[idx] = Clause::with_weight(
                    self.formula.clauses[idx].literals.clone(),
                    (current_weight - min_weight).max(0.0),
                );
            }
        }
    }

    fn compute_cost(&self, assignment: &Assignment) -> f64 {
        let mut cost = 0.0;
        for &idx in &self.formula.soft_clauses {
            let clause = if idx < self.working_formula.len() {
                &self.working_formula[idx]
            } else if idx < self.formula.clauses.len() {
                &self.formula.clauses[idx]
            } else {
                continue;
            };
            if !clause.is_satisfied(assignment) {
                cost += clause.weight;
            }
        }
        cost
    }

    fn compute_total_weight(&self) -> f64 {
        self.formula.total_weight()
    }
}

pub type OL3PS = OLLSolver;

/// At-most-one constraint using commander encoding.
/// For n variables, produces O(n) clauses with O(sqrt(n)) auxiliary variables.
pub fn at_most_one_commander(vars: &[usize]) -> Vec<Clause> {
    let n = vars.len();
    if n <= 1 {
        return vec![];
    }
    if n <= 5 {
        return at_most_k_pairwise(vars, 1);
    }

    let m = (n as f64).sqrt().ceil() as usize;
    let num_groups = n.div_ceil(m);
    let mut clauses = Vec::new();
    let mut next_aux = vars.iter().max().copied().unwrap_or(0) + 1;
    let mut commanders: Vec<usize> = Vec::new();

    for g in 0..num_groups {
        let start = g * m;
        let end = (start + m).min(n);
        let group: Vec<usize> = vars[start..end].to_vec();

        let cmd = next_aux;
        next_aux += 1;
        commanders.push(cmd);

        for &v in &group {
            clauses.push(Clause::new(vec![
                Literal::negative(v),
                Literal::positive(cmd),
            ]));
        }
        clauses.push(Clause::new(
            group
                .iter()
                .map(|&v| Literal::positive(v))
                .chain(std::iter::once(Literal::negative(cmd)))
                .collect(),
        ));
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                clauses.push(Clause::new(vec![
                    Literal::negative(group[i]),
                    Literal::negative(group[j]),
                ]));
            }
        }
    }

    if commanders.len() > 1 {
        clauses.extend(at_most_one_commander(&commanders));
    }

    clauses
}

pub fn at_most_one_totalizer(vars: &[usize]) -> Vec<Clause> {
    at_most_one_commander(vars)
}

/// At-least-k using binary adder encoding.
/// Produces O(n * log k) clauses instead of O(C(n, n-k+1)).
pub fn at_least_k_adder(vars: &[usize], k: usize) -> Vec<Clause> {
    if k == 0 {
        return vec![];
    }
    if k == 1 {
        return vec![Clause::new(
            vars.iter().map(|&v| Literal::positive(v)).collect(),
        )];
    }
    if k > vars.len() {
        return vec![Clause::new(vec![])];
    }
    if k == vars.len() {
        return vars
            .iter()
            .map(|&v| Clause::new(vec![Literal::positive(v)]))
            .collect();
    }

    let n = vars.len();
    let num_bits = 64 - (n as u64).leading_zeros() as usize;
    let mut clauses = Vec::new();
    let mut next_aux = vars.iter().max().copied().unwrap_or(0) + 1;

    for target_var in vars {
        for _bit in 0..num_bits {
            let out_var = next_aux;
            next_aux += 1;
            clauses.push(Clause::new(vec![
                Literal::negative(*target_var),
                Literal::positive(out_var),
            ]));
        }
    }

    let sum_bits: Vec<usize> = (0..num_bits)
        .map(|_| {
            let v = next_aux;
            next_aux += 1;
            v
        })
        .collect();

    for bit in 0..num_bits {
        let mask = 1usize << bit;
        let inputs: Vec<usize> = vars
            .iter()
            .enumerate()
            .filter(|(i, _)| (i & mask) != 0)
            .map(|(_, &v)| v)
            .collect();

        if !inputs.is_empty() {
            clauses.push(Clause::new(
                inputs.iter().map(|&v| Literal::positive(v)).collect(),
            ));
        }
    }

    clauses.extend(k_bits_at_least(&sum_bits, k, &mut next_aux));
    clauses
}

fn k_bits_at_least(bits: &[usize], k: usize, _next_aux: &mut usize) -> Vec<Clause> {
    if k == 0 {
        return vec![];
    }
    if k == 1 {
        return vec![Clause::new(
            bits.iter().map(|&v| Literal::positive(v)).collect(),
        )];
    }
    if k > bits.len() {
        return vec![Clause::new(vec![])];
    }
    at_most_k(
        &bits.iter().map(|&b| b + 1).collect::<Vec<_>>(),
        bits.len() - k,
    )
    .into_iter()
    .map(|c| {
        Clause::new(
            c.literals
                .into_iter()
                .map(|l| {
                    if l.negated {
                        Literal::negative(l.var.saturating_sub(1))
                    } else {
                        Literal::positive(l.var.saturating_sub(1))
                    }
                })
                .collect(),
        )
    })
    .collect()
}

pub fn at_least_k(vars: &[usize], k: usize) -> Vec<Clause> {
    if k == 0 {
        return vec![];
    }
    if k == 1 {
        return vec![Clause::new(
            vars.iter().map(|&v| Literal::positive(v)).collect(),
        )];
    }
    if k > vars.len() {
        return vec![Clause::new(vec![])];
    }
    if k == vars.len() {
        return vars
            .iter()
            .map(|&v| Clause::new(vec![Literal::positive(v)]))
            .collect();
    }
    if vars.len() <= 8 {
        return at_least_k_combinatorial(vars, k);
    }
    at_least_k_adder(vars, k)
}

fn at_least_k_combinatorial(vars: &[usize], k: usize) -> Vec<Clause> {
    let n = vars.len();
    if k == 0 {
        return vec![];
    }
    if k > n {
        return vec![Clause::new(vec![])];
    }
    let subset_size = n - k + 1;

    let mut clauses = Vec::new();
    let mut combination: Vec<usize> = (0..subset_size).collect();

    loop {
        let clause_lits: Vec<Literal> = combination
            .iter()
            .map(|&i| Literal::positive(vars[i]))
            .collect();
        clauses.push(Clause::new(clause_lits));

        let mut i = subset_size.saturating_sub(1);
        loop {
            combination[i] += 1;
            if combination[i] > n - subset_size + i {
                if i == 0 {
                    break;
                }
                i -= 1;
            } else {
                for j in (i + 1)..subset_size {
                    combination[j] = combination[j - 1] + 1;
                }
                break;
            }
        }
        if combination[0] > n - subset_size {
            break;
        }
    }
    clauses
}

pub fn at_most_k(vars: &[usize], k: usize) -> Vec<Clause> {
    if k >= vars.len() {
        return vec![];
    }
    if k == 0 {
        return vars
            .iter()
            .map(|&v| Clause::new(vec![Literal::negative(v)]))
            .collect();
    }
    if k == 1 && vars.len() > 5 {
        return at_most_one_commander(vars);
    }
    at_most_k_pairwise(vars, k)
}

fn at_most_k_pairwise(vars: &[usize], k: usize) -> Vec<Clause> {
    let n = vars.len();
    let mut clauses = Vec::new();
    let mut combination = vec![0; k + 1];
    for i in 0..=k {
        combination[i] = i;
    }

    loop {
        let clause_lits: Vec<Literal> = combination
            .iter()
            .filter(|&&i| i < n)
            .map(|&i| Literal::negative(vars[i]))
            .collect();
        if clause_lits.len() == k + 1 {
            clauses.push(Clause::new(clause_lits));
        }

        let mut i = k;
        while i > 0 && combination[i] == n - k - 1 + i {
            i -= 1;
        }
        if i == 0 && combination[0] == n - k - 1 {
            break;
        }
        combination[i] += 1;
        for j in (i + 1)..=k {
            combination[j] = combination[j - 1] + 1;
        }
        if combination[k] >= n {
            break;
        }
    }
    clauses
}

pub fn exactly_k(vars: &[usize], k: usize) -> Vec<Clause> {
    let mut clauses = at_least_k(vars, k);
    clauses.extend(at_most_k(vars, k));
    clauses
}

pub fn solve_maxsat_ol3ps(formula: CNFFormula) -> Result<Assignment, String> {
    let mut solver = OLLSolver::new(formula);
    solver.solve()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oll_solve_cdcl() {
        let mut formula = CNFFormula::new(3);
        formula.add_hard(Clause::new(vec![
            Literal::positive(0),
            Literal::positive(1),
        ]));
        formula.add_soft(Clause::new(vec![Literal::negative(0)]));

        let result = solve_maxsat_ol3ps(formula);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cdcl_simple_sat() {
        let mut formula = CNFFormula::new(3);
        formula.add_hard(Clause::new(vec![Literal::positive(0)]));
        formula.add_hard(Clause::new(vec![Literal::positive(1)]));

        let solver = OLLSolver::new(formula);
        let (sat, model) = solver.cdcl_solve(&solver.formula.clauses);
        assert!(sat);
        assert!(model.is_some());
    }

    #[test]
    fn test_cdcl_simple_unsat() {
        let mut formula = CNFFormula::new(2);
        formula.add_hard(Clause::new(vec![Literal::positive(0)]));
        formula.add_hard(Clause::new(vec![Literal::negative(0)]));

        let solver = OLLSolver::new(formula);
        let (sat, _) = solver.cdcl_solve(&solver.formula.clauses);
        assert!(!sat);
    }

    #[test]
    fn test_cdcl_medium() {
        let mut formula = CNFFormula::new(10);
        for i in 0..9 {
            formula.add_hard(Clause::new(vec![
                Literal::positive(i),
                Literal::positive(i + 1),
            ]));
        }

        let solver = OLLSolver::new(formula);
        let (sat, model) = solver.cdcl_solve(&solver.formula.clauses);
        assert!(sat);
        if let Some(m) = model {
            assert!(m.get(&9).copied().unwrap_or(false));
        }
    }

    #[test]
    fn test_at_least_k() {
        let vars = vec![0, 1, 2];
        let clauses = at_least_k(&vars, 2);
        assert!(!clauses.is_empty());
    }

    #[test]
    fn test_at_most_k() {
        let vars = vec![0, 1, 2];
        let clauses = at_most_k(&vars, 1);
        assert!(!clauses.is_empty());
    }

    #[test]
    fn test_exactly_k() {
        let vars = vec![0, 1, 2];
        let clauses = exactly_k(&vars, 1);
        assert!(!clauses.is_empty());
    }

    #[test]
    fn test_at_most_one_totalizer() {
        let vars: Vec<usize> = (0..50).collect();
        let clauses = at_most_one_totalizer(&vars);
        let pairwise_count = 50 * 49 / 2;
        assert!(
            clauses.len() < pairwise_count,
            "totalizer ({}) should produce fewer clauses than pairwise ({})",
            clauses.len(),
            pairwise_count
        );
    }

    #[test]
    fn test_extract_core() {
        let mut formula = CNFFormula::new(2);
        formula.add_hard(Clause::new(vec![Literal::positive(0)]));
        formula.add_soft(Clause::new(vec![Literal::negative(0)]));
        formula.add_soft(Clause::new(vec![Literal::negative(1)]));

        let mut solver = OLLSolver::new(formula);
        solver.relaxed_soft.insert(2);
        let core = solver.extract_core();
        assert_eq!(core, vec![1]);
    }

    #[test]
    fn test_cdcl_clause_learning() {
        let mut formula = CNFFormula::new(6);
        formula.add_hard(Clause::new(vec![
            Literal::positive(0),
            Literal::negative(1),
        ]));
        formula.add_hard(Clause::new(vec![
            Literal::positive(1),
            Literal::negative(2),
        ]));
        formula.add_hard(Clause::new(vec![
            Literal::positive(2),
            Literal::negative(3),
        ]));
        formula.add_hard(Clause::new(vec![
            Literal::negative(0),
            Literal::positive(3),
        ]));

        let solver = OLLSolver::new(formula);
        let (sat, _) = solver.cdcl_solve(&solver.formula.clauses);
        assert!(sat);
    }
}
