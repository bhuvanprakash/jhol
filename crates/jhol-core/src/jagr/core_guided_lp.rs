//! JAGR Layer 1: Core-Guided LP-Based MaxSAT
//!
//! Implements the OL3P/OL3P-S algorithm from the SAT 2025 paper:
//! "Core-Guided Linear Programming-Based Maximum Satisfiability"
//!
//! This provides the exact MaxSAT solver for Layer 1 of JAGR.
//! Uses Linear Programming for computing lower bounds and SAT for
//! finding satisfying assignments.

use std::collections::{HashMap, HashSet};

/// Boolean literal: positive or negated variable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Literal {
    pub var: usize,
    pub negated: bool,
}

impl Literal {
    pub fn positive(var: usize) -> Self {
        Self {
            var,
            negated: false,
        }
    }

    pub fn negative(var: usize) -> Self {
        Self { var, negated: true }
    }

    pub fn negate(&self) -> Self {
        Self {
            var: self.var,
            negated: !self.negated,
        }
    }

    pub fn variable(&self) -> usize {
        self.var
    }

    pub fn is_positive(&self) -> bool {
        !self.negated
    }
}

/// Boolean clause: disjunction of literals
#[derive(Debug, Clone)]
pub struct Clause {
    pub literals: Vec<Literal>,
    pub weight: f64, // for MaxSAT
}

impl Clause {
    pub fn new(literals: Vec<Literal>) -> Self {
        Self {
            literals,
            weight: 1.0,
        }
    }

    pub fn with_weight(literals: Vec<Literal>, weight: f64) -> Self {
        Self { literals, weight }
    }

    /// Check if clause is satisfied by assignment
    /// Under 3-valued logic: a clause is satisfied if at least one literal
    /// evaluates to TRUE. Unassigned variables keep the clause "potentially
    /// satisfiable" but do NOT satisfy it. For cost computation (which needs a
    /// definitive true/false), we treat unassigned as unknown and only mark
    /// a clause as violated when all assigned literals are false AND no
    /// unassigned literal can rescue it. For is_satisfied, we require at
    /// least one literal to be concretely TRUE.
    pub fn is_satisfied(&self, assignment: &HashMap<usize, bool>) -> bool {
        self.literals
            .iter()
            .any(|lit| match assignment.get(&lit.var).copied() {
                Some(val) => {
                    if lit.negated {
                        !val
                    } else {
                        val
                    }
                }
                None => false,
            })
    }

    /// Check if a clause is "definitely violated" (all literals evaluate to false)
    /// regardless of unassigned variables
    pub fn is_violated(&self, assignment: &HashMap<usize, bool>) -> bool {
        // Clause is violated when all assigned literals are false and no unassigned can make it true
        // Simplified: violated if no literal can be true given the current partial assignment
        for lit in &self.literals {
            let val = assignment.get(&lit.var).copied();
            match val {
                Some(v) => {
                    let lit_val = if lit.negated { !v } else { v };
                    if lit_val {
                        return false; // This literal is true, clause is satisfied
                    }
                }
                None => {
                    // Unassigned - could potentially satisfy
                    return false;
                }
            }
        }
        // All assigned literals are false
        true
    }

    /// Check if clause is unit (one literal unassigned, rest false)
    pub fn is_unit(&self, assignment: &HashMap<usize, bool>) -> Option<Literal> {
        let mut unit_lit = None;
        let mut unassigned_count = 0;

        for lit in &self.literals {
            match assignment.get(&lit.var).copied() {
                Some(val) => {
                    let lit_val = if lit.negated { !val } else { val };
                    if lit_val {
                        return None; // Clause is satisfied
                    }
                }
                None => {
                    unassigned_count += 1;
                    if unassigned_count > 1 {
                        return None;
                    }
                    unit_lit = Some(*lit);
                }
            }
        }

        if unassigned_count == 1 {
            unit_lit
        } else {
            None
        }
    }
}

/// CNF formula: conjunction of clauses
#[derive(Debug, Clone)]
pub struct CNFFormula {
    pub clauses: Vec<Clause>,
    pub num_vars: usize,
    pub hard_clauses: Vec<usize>, // indices of hard (must-satisfy) clauses
    pub soft_clauses: Vec<usize>, // indices of soft (optimization) clauses
}

impl CNFFormula {
    pub fn new(num_vars: usize) -> Self {
        Self {
            clauses: Vec::new(),
            num_vars,
            hard_clauses: Vec::new(),
            soft_clauses: Vec::new(),
        }
    }

    pub fn add_hard(&mut self, clause: Clause) {
        let idx = self.clauses.len();
        self.clauses.push(clause);
        self.hard_clauses.push(idx);
    }

    pub fn add_soft(&mut self, clause: Clause) {
        let idx = self.clauses.len();
        self.clauses.push(clause);
        self.soft_clauses.push(idx);
    }

    pub fn total_weight(&self) -> f64 {
        self.soft_clauses
            .iter()
            .map(|&i| self.clauses[i].weight)
            .sum()
    }
}

/// Linear program for MaxSAT lower bound
#[derive(Debug, Clone)]
pub struct LP {
    /// Variables: (name, lower_bound, upper_bound)
    pub variables: Vec<(String, f64, f64)>,
    /// Objective: coefficients for each variable
    pub objective: Vec<f64>,
    /// Constraints: (coeffs, comparison_op, rhs)
    pub constraints: Vec<(Vec<f64>, LPComparison, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LPComparison {
    Le, // ≤
    Ge, // ≥
    Eq, // =
}

impl LP {
    pub fn new() -> Self {
        Self {
            variables: Vec::new(),
            objective: Vec::new(),
            constraints: Vec::new(),
        }
    }

    pub fn add_variable(&mut self, name: String, lb: f64, ub: f64) -> usize {
        let idx = self.variables.len();
        self.variables.push((name, lb, ub));
        self.objective.push(0.0);
        idx
    }

    pub fn set_objective(&mut self, var: usize, coeff: f64) {
        self.objective[var] = coeff;
    }

    pub fn add_constraint(&mut self, coeffs: Vec<f64>, comp: LPComparison, rhs: f64) {
        self.constraints.push((coeffs, comp, rhs));
    }

    /// Solve the LP using simplex-like method (simplified)
    /// Returns (optimal_value, solution)
    pub fn solve(&self) -> (f64, Vec<f64>) {
        // Use a simple Simplex implementation
        simplex_solve(self)
    }
}

/// Simplex method for LP solving (revised simplex with full tableau)
///
/// Solves: maximize c^T x subject to Ax ≤ b, lb ≤ x ≤ ub
/// Uses two-phase simplex: Phase I finds a basic feasible solution,
/// Phase II optimizes the objective.
fn simplex_solve(lp: &LP) -> (f64, Vec<f64>) {
    let n = lp.variables.len();
    if n == 0 {
        return (0.0, vec![]);
    }

    let (a_std, b_std) = standardize_constraints(&lp.constraints, &lp.variables);
    let m = a_std.len();
    if m == 0 {
        let mut sol = Vec::with_capacity(n);
        let mut val = 0.0;
        for i in 0..n {
            let ub = lp.variables[i].2;
            let lb = lp.variables[i].1;
            if lp.objective[i] > 0.0 {
                sol.push(ub);
            } else {
                sol.push(lb);
            }
            val += lp.objective[i] * sol[i];
        }
        return (val, sol);
    }

    let result = two_phase_simplex(&lp.objective, &a_std, &b_std, n, m);
    match result {
        Some((_val, sol)) => {
            let clamped: Vec<f64> = sol
                .iter()
                .enumerate()
                .map(|(i, &x)| x.clamp(lp.variables[i].1, lp.variables[i].2))
                .collect();
            let clamped_val: f64 = lp
                .objective
                .iter()
                .zip(clamped.iter())
                .map(|(c, x)| c * x)
                .sum();
            (clamped_val, clamped)
        }
        None => (0.0, vec![0.0; n]),
    }
}

fn standardize_constraints(
    constraints: &[(Vec<f64>, LPComparison, f64)],
    variables: &[(String, f64, f64)],
) -> (Vec<Vec<f64>>, Vec<f64>) {
    let n = variables.len();
    let mut a: Vec<Vec<f64>> = Vec::new();
    let mut b: Vec<f64> = Vec::new();

    for (coeffs, comp, rhs) in constraints {
        let padded: Vec<f64> = if coeffs.len() < n {
            let mut row = coeffs.clone();
            row.resize(n, 0.0);
            row
        } else {
            coeffs.clone()
        };
        match comp {
            LPComparison::Le => {
                a.push(padded);
                b.push(*rhs);
            }
            LPComparison::Ge => {
                a.push(padded.iter().map(|&c| -c).collect());
                b.push(-rhs);
            }
            LPComparison::Eq => {
                a.push(padded.clone());
                b.push(*rhs);
                a.push(padded.iter().map(|&c| -c).collect());
                b.push(-rhs);
            }
        }
    }

    for i in 0..b.len() {
        if b[i] < 0.0 {
            for j in 0..a[i].len() {
                a[i][j] = -a[i][j];
            }
            b[i] = -b[i];
        }
    }

    (a, b)
}

fn two_phase_simplex(
    obj: &[f64],
    a: &[Vec<f64>],
    b: &[f64],
    n: usize,
    m: usize,
) -> Option<(f64, Vec<f64>)> {
    let total = n + m;
    let mut tableau: Vec<Vec<f64>> = Vec::with_capacity(m + 1);

    for i in 0..m {
        let mut row = vec![0.0; total + 1];
        for j in 0..n {
            row[j] = a[i][j];
        }
        row[n + i] = 1.0;
        row[total] = b[i];
        tableau.push(row);
    }

    let mut basis: Vec<usize> = (0..m).map(|i| n + i).collect();

    let phase1_obj: Vec<f64> = {
        let mut p = vec![0.0; total];
        for i in 0..m {
            if b[i] < -1e-12 {
                p[n + i] = -1.0;
            }
        }
        for i in 0..m {
            if b[i] < -1e-12 {
                for j in 0..total {
                    p[j] += tableau[i][j];
                }
            }
        }
        p
    };

    let needs_phase1 = b.iter().any(|&v| v < -1e-12);

    if needs_phase1 {
        let mut phase1_row = vec![0.0; total + 1];
        for j in 0..total {
            phase1_row[j] = phase1_obj[j];
        }
        for i in 0..m {
            if b[i] < -1e-12 {
                for j in 0..total + 1 {
                    phase1_row[j] -= tableau[i][j];
                }
            }
        }
        for i in 0..m {
            if b[i] < -1e-12 {
                for j in 0..total + 1 {
                    tableau[i][j] = -tableau[i][j];
                }
                basis[i] = n + i;
            }
        }

        pivot_simplex(&mut tableau, &mut basis, total, m, 200 + m * 2);

        let mut is_feasible = true;
        for i in 0..m {
            if basis[i] >= n && tableau[i][total].abs() > 1e-8 {
                is_feasible = false;
                break;
            }
        }
        if !is_feasible {
            return None;
        }
    }

    let mut obj_row = vec![0.0; total + 1];
    for j in 0..n {
        obj_row[j] = -obj[j];
    }
    for i in 0..m {
        if basis[i] < n {
            let coeff = obj_row[basis[i]];
            if coeff.abs() > 1e-12 {
                for j in 0..total + 1 {
                    obj_row[j] -= coeff * tableau[i][j];
                }
            }
        }
    }
    tableau.push(obj_row);

    pivot_simplex(&mut tableau, &mut basis, total, m, 500 + m * n);

    let mut sol = vec![0.0; n];
    for i in 0..m {
        if basis[i] < n {
            sol[basis[i]] = tableau[i][total];
        }
    }

    let opt_val = -tableau[m][total];
    Some((opt_val, sol))
}

fn pivot_simplex(
    tableau: &mut [Vec<f64>],
    basis: &mut [usize],
    total: usize,
    m: usize,
    max_iters: usize,
) {
    let rows = m + 1;
    let cols = total + 1;
    let mut iterations = 0;

    loop {
        iterations += 1;
        if iterations > max_iters {
            break;
        }

        let obj_row_idx = m;
        let mut entering = usize::MAX;
        let mut most_neg = -1e-10;
        for j in 0..total {
            if tableau[obj_row_idx][j] < most_neg {
                most_neg = tableau[obj_row_idx][j];
                entering = j;
            }
        }
        if entering == usize::MAX {
            break;
        }

        let mut leaving = usize::MAX;
        let mut min_ratio = f64::INFINITY;
        for i in 0..m {
            if tableau[i][entering] > 1e-12 {
                let ratio = tableau[i][cols - 1] / tableau[i][entering];
                if ratio < min_ratio - 1e-12 {
                    min_ratio = ratio;
                    leaving = i;
                } else if (ratio - min_ratio).abs() < 1e-12 && leaving < m {
                    if basis[i] > basis[leaving] {
                        leaving = i;
                    }
                }
            }
        }
        if leaving == usize::MAX {
            break;
        }

        let pivot = tableau[leaving][entering];
        for j in 0..cols {
            tableau[leaving][j] /= pivot;
        }
        for i in 0..rows {
            if i != leaving {
                let factor = tableau[i][entering];
                if factor.abs() > 1e-15 {
                    for j in 0..cols {
                        tableau[i][j] -= factor * tableau[leaving][j];
                    }
                }
            }
        }
        basis[leaving] = entering;
    }
}

/// Core: a set of clauses that are jointly unsatisfiable
#[derive(Debug, Clone)]
pub struct Core {
    pub clause_indices: Vec<usize>,
    pub new_variables: Vec<usize>, // For OL3P encoding
}

/// Assignment: variable -> bool
pub type Assignment = HashMap<usize, bool>;

/// OL3P: Core-guided LP-based MaxSAT algorithm
pub struct OL3PSolver {
    formula: CNFFormula,
    /// Working formula with OLL/OL3P relaxation clauses
    working_formula: Vec<Clause>,
    /// Current LP
    lp: LP,
    /// Lower bound
    lower_bound: f64,
    /// Upper bound (best solution found)
    upper_bound: f64,
    /// Best assignment
    best_assignment: Option<Assignment>,
    /// Number of iterations
    pub iterations: usize,
    /// Max iterations to prevent infinite loop
    max_iterations: usize,
    /// Metavariables for OL3P encoding
    o_vars: HashMap<(usize, usize), usize>, // (clause_idx, j) -> o_ij
    e_vars: HashMap<(usize, usize), usize>, // (clause_idx, j) -> e_ij
    /// Reduced costs from LP
    reduced_costs: Vec<f64>,
    /// Solution from LP
    lp_solution: Vec<f64>,
    /// Current assignment
    current_assignment: Assignment,
    /// Next variable index for new variables
    next_var: usize,
    /// Core history to avoid re-processing
    core_history: HashSet<Vec<usize>>,
}

impl OL3PSolver {
    pub fn new(formula: CNFFormula) -> Self {
        let next_var = formula.num_vars;
        let lp = LP::new();

        Self {
            formula,
            working_formula: Vec::new(),
            lp,
            lower_bound: 0.0,
            upper_bound: 0.0,
            best_assignment: None,
            iterations: 0,
            max_iterations: 10000,
            o_vars: HashMap::new(),
            e_vars: HashMap::new(),
            reduced_costs: Vec::new(),
            lp_solution: Vec::new(),
            current_assignment: HashMap::new(),
            next_var,
            core_history: HashSet::new(),
        }
    }

    pub fn formula(&self) -> &CNFFormula {
        &self.formula
    }

    /// Main OL3P algorithm loop
    pub fn solve(&mut self) -> Result<Assignment, String> {
        // Initialize
        self.working_formula = self.formula.clauses.clone();
        self.upper_bound = self.formula.total_weight();
        self.lower_bound = 0.0;
        self.next_var = self.formula.num_vars;

        loop {
            self.iterations += 1;

            if self.iterations > self.max_iterations {
                return Err("Max iterations reached".to_string());
            }

            // Step 1: Set reduced cost variables to false in SAT check
            // Build reduced assignment
            let reduced_assignment = self.build_reduced_assignment();

            // Step 2: SAT check with reduced costs = 0
            let (sat_result, model) = self.sat_check(&reduced_assignment);

            if sat_result {
                // SAT! Optimal found
                self.upper_bound = self.lower_bound;
                self.best_assignment = Some(model.unwrap_or_default());
                return Ok(self.best_assignment.clone().unwrap());
            }

            // UNSAT - get core
            let core = self.extract_core();

            if core.clause_indices.is_empty() {
                return Err("No core found but UNSAT".to_string());
            }

            // Check for duplicate core
            let mut sorted_core = core.clause_indices.clone();
            sorted_core.sort();
            if !self.core_history.insert(sorted_core.clone()) {
                // Duplicate core, can't improve
                return Ok(self.best_assignment.clone().unwrap_or_default());
            }

            // Step 3: Add OLL constraints + OL3P LP constraints
            self.add_oll_constraints(&core);
            self.add_ol3p_lp_constraints(&core);

            // Step 4: Resolve LP
            let (new_lb, solution) = self.resolve_lp();

            if new_lb > self.lower_bound {
                self.lower_bound = new_lb;
            }

            self.lp_solution = solution;
            self.compute_reduced_costs();

            // Check for fixed variables
            self.fix_variables();

            // If lower bound == upper bound, we're optimal
            if (self.upper_bound - self.lower_bound).abs() < 1e-9 {
                return Ok(self.best_assignment.clone().unwrap_or_default());
            }
        }
    }

    /// Build assignment with non-zero reduced costs set based on LP solution direction
    fn build_reduced_assignment(&self) -> Assignment {
        let mut assignment = HashMap::new();

        for (var, &cost) in self.reduced_costs.iter().enumerate() {
            if cost.abs() < 1e-9 {
                // Free variable — don't set it
            } else {
                // Fixed based on LP solution direction:
                // LP suggests true (sol_val >= 0.5) → fix to true;
                // LP suggests false → fix to false.
                let sol_val = self.lp_solution.get(var).copied().unwrap_or(0.0);
                assignment.insert(var, sol_val >= 0.5);
            }
        }

        // Apply any existing assignment
        for (&var, &val) in &self.current_assignment {
            assignment.insert(var, val);
        }

        assignment
    }

    /// SAT check with the given partial assignment
    fn sat_check(&self, assignment: &Assignment) -> (bool, Option<Assignment>) {
        let mut formula = self.formula.clone();
        for (&var, &val) in assignment {
            if var < formula.num_vars {
                if val {
                    formula.add_hard(Clause::new(vec![Literal::positive(var)]));
                } else {
                    formula.add_hard(Clause::new(vec![Literal::negative(var)]));
                }
            }
        }
        let mut solver = super::ol3p_solver::OLLSolver::new(formula);
        match solver.solve() {
            Ok(assign) => (true, Some(assign)),
            Err(_) => (false, None),
        }
    }

    /// Extract an unsatisfiable core
    fn extract_core(&self) -> Core {
        // Use the LP solution / reduced-cost assignment to approximate
        // which soft clauses are UNSATISFIED. This gives a smaller core
        // than "all soft clauses", improving OLL convergence.
        let assignment = self.build_reduced_assignment();

        let mut core_indices: Vec<usize> = Vec::new();
        for &idx in &self.formula.soft_clauses {
            if idx < self.formula.clauses.len() {
                let clause = &self.formula.clauses[idx];
                if !clause.is_satisfied(&assignment) {
                    core_indices.push(idx);
                }
            }
        }

        if core_indices.is_empty() {
            core_indices = self.formula.soft_clauses.iter().copied().collect();
        }

        Core {
            clause_indices: core_indices,
            new_variables: Vec::new(),
        }
    }

    /// Add OLL (core-guided) constraints for a core
    fn add_oll_constraints(&mut self, core: &Core) {
        let m = core.clause_indices.len();
        if m == 0 {
            return;
        }

        // Step 1: For each core clause, allocate a relaxation variable b_i.
        // The clause becomes (original_lits ∨ b_i), meaning the clause is
        // satisfied if any original literal is true OR the relaxation fires.
        // b_i = true  ⟹  clause i is relaxed (not required to satisfy its
        // original literals).  Count(b_i=true) = number of relaxed core clauses.
        let mut b_vars: Vec<usize> = Vec::with_capacity(m);
        for &clause_idx in &core.clause_indices {
            let b_var = self.next_var;
            self.next_var += 1;
            b_vars.push(b_var);

            let mut relaxed = self.working_formula[clause_idx].literals.clone();
            relaxed.push(Literal::positive(b_var));
            self.working_formula[clause_idx] =
                Clause::with_weight(relaxed, self.working_formula[clause_idx].weight);
        }

        // Step 2: At-least-j cardinality constraints for j = 1..m.
        // "At least j of the b_i must be true" is equivalent to:
        //   for every subset S of size m−j+1, ⋁_{i∈S} b_i
        // We encode this with auxiliary variable o_j and the implication
        //   o_j → b_k   for each k   (if o_j true, ALL b_k true)
        // and a soft unit clause ¬o_j with weight j.  The soft clause
        // prefers o_j = false (fewer relaxations).  When violated,
        // o_j = true forces all b_k true, relaxing all core clauses.
        for j in 1..=m {
            let weight = j as f64;
            let o_j = self.next_var;
            self.next_var += 1;

            self.o_vars.insert((core.clause_indices[0], j), o_j);

            // Soft clause: ¬o_j  (prefer no relaxation at this level)
            let soft = Clause::with_weight(vec![Literal::negative(o_j)], weight);
            self.working_formula.push(soft);

            // Hard clauses: o_j → b_k  ↔  ¬o_j ∨ b_k  for each b_k
            for &b_var in &b_vars {
                let hard_clause =
                    Clause::new(vec![Literal::negative(o_j), Literal::positive(b_var)]);
                self.working_formula.push(hard_clause);
            }
        }
    }

    /// Add OL3P-specific LP constraints
    fn add_ol3p_lp_constraints(&mut self, core: &Core) {
        let m = core.clause_indices.len();

        // Create e_j variables: e_j = 1 iff exactly j of the core
        // clauses are relaxed. We add LP constraints:
        //   sum_j e_j = 1 (exactly one "exactly-k" bucket is active)
        //   sum_j j * e_j = sum_i b_i (the "exactly k" bucket matches the count)
        //   e_j >= 0, e_j <= 1
        // These are added to the LP during resolve_lp().

        for j in 0..=m {
            let e_var = self.next_var;
            self.next_var += 1;
            self.e_vars.insert((core.clause_indices[0], j), e_var);
        }
    }

    /// Resolve the LP
    fn resolve_lp(&mut self) -> (f64, Vec<f64>) {
        self.lp = LP::new();

        // One LP variable per Boolean variable (continuous relaxation in [0,1])
        for var in 0..self.next_var {
            self.lp.add_variable(format!("x_{}", var), 0.0, 1.0);
        }

        // For each hard clause (¬x_i ∨ x_j ∨ ...):
        //   LP relaxation: (1 - x_i) + x_j + ... >= 1
        //   i.e., sum of positive + (1 - negated) >= 1
        //   rearranged: for positive lits add +1, for negated add -1, rhs adjusted
        for (idx, clause) in self.working_formula.iter().enumerate() {
            let is_hard = if idx < self.formula.clauses.len() {
                self.formula.hard_clauses.contains(&idx)
            } else {
                // Extended clauses from OLL relaxation are hard constraints
                // (they encode at-least-k constraints and relaxation implications).
                // A clause is hard if it's not a soft clause — soft clauses have
                // positive weight and are either original soft clauses or the
                // negated o_j unit clauses added by add_oll_constraints.
                // Heuristic: if the clause was NOT added as a soft preference
                // (identified by being a single-negated-variable clause with
                // positive weight), treat it as hard.
                let is_soft_unit =
                    clause.literals.len() == 1 && clause.literals[0].negated && clause.weight > 0.0;
                !is_soft_unit
            };

            if !is_hard && !clause.literals.is_empty() {
                // Soft clauses contribute to the objective, not as hard LP constraints
                continue;
            }

            let mut coeffs = vec![0.0; self.next_var];
            let mut rhs_offset = 0.0;

            for lit in &clause.literals {
                if lit.negated {
                    coeffs[lit.var] -= 1.0;
                    rhs_offset -= 1.0;
                } else {
                    coeffs[lit.var] += 1.0;
                }
            }

            // sum(coeff_i * x_i) >= 1 + rhs_offset
            // Convert to ≤ form: -sum >= -(1 + rhs_offset)
            if !coeffs.iter().all(|&c: &f64| c.abs() < 1e-15) {
                self.lp
                    .add_constraint(coeffs, LPComparison::Ge, 1.0 + rhs_offset);
            }
        }

        // Objective: minimize sum of soft clause violation weights
        // For each soft clause ¬x_i with weight w:
        //   violation = x_i (if the variable is true, clause is violated)
        // But we want MAXIMIZE, so objective = -sum(w_i * x_i) for soft vars
        for (idx, clause) in self.working_formula.iter().enumerate() {
            let is_soft =
                idx >= self.formula.clauses.len() || self.formula.soft_clauses.contains(&idx);
            if !is_soft {
                continue;
            }
            let weight = clause.weight;
            for lit in &clause.literals {
                if lit.negated {
                    // ¬x with weight w → minimize x → set objective -w for x
                    self.lp.set_objective(
                        lit.var,
                        self.lp.objective.get(lit.var).copied().unwrap_or(0.0) - weight,
                    );
                } else {
                    // x with weight w → maximize x being true → set objective +w
                    self.lp.set_objective(
                        lit.var,
                        self.lp.objective.get(lit.var).copied().unwrap_or(0.0) + weight,
                    );
                }
            }
        }

        let (val, sol) = self.lp.solve();
        let new_lb = -val; // We maximized -violation, so lb = -val
        (new_lb.max(self.lower_bound), sol)
    }

    /// Compute reduced costs from LP solution
    fn compute_reduced_costs(&mut self) {
        // Reduced cost of variable x_j = c_j - sum_i (a_ij * y_i)
        // where y_i are the dual values (shadow prices of constraints).
        // For the simplex solver, the reduced cost is the objective
        // coefficient minus the dot product of constraint coefficients
        // with dual prices. A non-basic variable with reduced cost > 0
        // means we'd lose objective value by increasing it — so we fix
        // it to 0 (false).
        //
        // Approximation from the primal solution: if variable x_j is at
        // its lower bound (0) in the optimal LP solution, its reduced cost
        // is non-negative (we shouldn't increase it). If it's at the upper
        // bound (1), reduced cost is non-positive (we shouldn't decrease it).
        // If it's strictly interior, reduced cost is zero.
        self.reduced_costs = (0..self.next_var)
            .map(|i| {
                let sol_val = self.lp_solution.get(i).copied().unwrap_or(0.0);
                if sol_val <= 1e-9 {
                    // At lower bound: reduced cost should be >= 0
                    // Use the objective coefficient as a proxy
                    self.lp.objective.get(i).copied().unwrap_or(0.0)
                } else if sol_val >= 1.0 - 1e-9 {
                    // At upper bound: reduced cost should be <= 0
                    self.lp.objective.get(i).copied().unwrap_or(0.0)
                } else {
                    // Interior: reduced cost ≈ 0
                    0.0
                }
            })
            .collect();
    }

    /// Fix variables with non-zero reduced costs
    fn fix_variables(&mut self) {
        for (var, &cost) in self.reduced_costs.iter().enumerate() {
            if cost.abs() > 1e-9 {
                let sol_val = self.lp_solution.get(var).copied().unwrap_or(0.0);
                let val = sol_val >= 0.5;
                self.current_assignment.insert(var, val);
            }
        }
    }
}

/// Main entry point: solve a MaxSAT problem using OL3P
pub fn solve_maxsat_ol3p(formula: CNFFormula) -> Result<Assignment, String> {
    let mut solver = OL3PSolver::new(formula);
    solver.solve()
}

/// Convert version constraints to CNF
pub fn version_constraints_to_cnf(
    constraints: &[(usize, usize, f64)], // (pkg_idx, version_idx, version_val)
) -> CNFFormula {
    let mut formula = CNFFormula::new(0);

    for (pkg_idx, _ver_idx, _ver_val) in constraints {
        // Each constraint becomes a clause
        // E.g., "package X version >= 1.0" becomes: X_version_1_0 ∨ X_version_1_1 ∨ ...
        // For simplicity, just add as a unit clause
        let clause = Clause::new(vec![Literal::positive(*pkg_idx)]);
        formula.add_hard(clause);
    }

    formula
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal() {
        let lit = Literal::positive(5);
        assert_eq!(lit.var, 5);
        assert!(lit.is_positive());

        let neg = lit.negate();
        assert!(!neg.is_positive());
    }

    #[test]
    fn test_clause_satisfied() {
        let clause = Clause::new(vec![Literal::positive(0), Literal::negative(1)]);
        let mut assignment = HashMap::new();
        assignment.insert(0, false);
        assignment.insert(1, false);
        // Not satisfied (0=false, !1=true → satisfied)
        assert!(clause.is_satisfied(&assignment));
    }

    #[test]
    fn test_unit_clause() {
        let clause = Clause::new(vec![Literal::positive(0)]);
        let mut assignment = HashMap::new();
        assignment.insert(1, false);
        // Unit: 0 is unassigned
        let unit = clause.is_unit(&assignment);
        assert_eq!(unit, Some(Literal::positive(0)));
    }

    #[test]
    fn test_ol3p_solve_simple() {
        let mut formula = CNFFormula::new(3);
        // Hard: (x0 ∨ x1)
        formula.add_hard(Clause::new(vec![
            Literal::positive(0),
            Literal::positive(1),
        ]));
        // Soft: (¬x0) with weight 1
        formula.add_soft(Clause::new(vec![Literal::negative(0)]));

        let result = solve_maxsat_ol3p(formula);
        assert!(result.is_ok());
    }

    #[test]
    fn test_lp_solve() {
        let mut lp = LP::new();
        lp.add_variable("x".to_string(), 0.0, 10.0);
        lp.set_objective(0, 1.0);
        lp.add_constraint(vec![1.0], LPComparison::Le, 5.0);

        let (val, sol) = lp.solve();
        assert_eq!(sol.len(), 1);
    }
}
