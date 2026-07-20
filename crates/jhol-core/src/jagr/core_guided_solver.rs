//! JAGR Layer 1: Core-Guided Solver
//!
//! High-level wrapper for the core-guided MaxSAT/MaxSMT solver.
//! Orchestrates OL3P (with LP) and OL3P-S (without LP) based on availability.

use super::core_guided_lp::{Assignment, CNFFormula};
use super::maxsat_solver::MaxSMTInstance;
use super::ol3p_solver::OLLSolver;

/// Solver type selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreGuidedVariant {
    /// OL3P with LP relaxation
    OL3P,
    /// OL3P-S: OLL-based without LP
    OL3PS,
}

/// Core-guided solver result
#[derive(Debug, Clone)]
pub struct CoreGuidedResult {
    pub assignment: Assignment,
    pub is_optimal: bool,
    pub cost: f64,
    pub iterations: usize,
    pub variant: CoreGuidedVariant,
}

/// Core-guided solver facade
pub struct CoreGuidedSolver {
    variant: CoreGuidedVariant,
    max_iterations: usize,
}

impl CoreGuidedSolver {
    pub fn new() -> Self {
        Self {
            variant: CoreGuidedVariant::OL3PS,
            max_iterations: 10000,
        }
    }

    pub fn with_variant(variant: CoreGuidedVariant) -> Self {
        Self {
            variant,
            max_iterations: 10000,
        }
    }

    pub fn with_max_iterations(mut self, max_iter: usize) -> Self {
        self.max_iterations = max_iter;
        self
    }

    /// Solve using the selected variant
    pub fn solve(&self, formula: CNFFormula) -> Result<CoreGuidedResult, String> {
        match self.variant {
            CoreGuidedVariant::OL3P => self.solve_ol3p(formula),
            CoreGuidedVariant::OL3PS => self.solve_ol3ps(formula),
        }
    }

    /// Solve using OL3P (with LP)
    /// The LP variant uses the same OLL solver backend but enriches the lower
    /// bound with the LP relaxation. The OL3PSolver in core_guided_lp.rs
    /// already integrates LP-based lower bounds into its iterative loop.
    fn solve_ol3p(&self, formula: CNFFormula) -> Result<CoreGuidedResult, String> {
        use super::core_guided_lp::OL3PSolver;

        let mut solver = OL3PSolver::new(formula);
        let assignment = solver.solve()?;
        let iterations = solver.iterations;
        let cost = self.compute_cost_from_cnf(solver.formula(), &assignment);
        let hit_limit = iterations >= self.max_iterations;

        Ok(CoreGuidedResult {
            assignment,
            is_optimal: !hit_limit && cost < 1e-9,
            cost,
            iterations,
            variant: CoreGuidedVariant::OL3P,
        })
    }

    /// Solve using OL3P-S (without LP)
    fn solve_ol3ps(&self, formula: CNFFormula) -> Result<CoreGuidedResult, String> {
        let mut solver = OLLSolver::new(formula);
        let assignment = solver.solve()?;
        let iterations = solver.iterations;
        let cost = self.compute_cost(&solver, &assignment);
        let hit_limit = iterations >= self.max_iterations;

        Ok(CoreGuidedResult {
            assignment,
            is_optimal: !hit_limit && cost < 1e-9,
            cost,
            iterations,
            variant: CoreGuidedVariant::OL3PS,
        })
    }

    /// Compute cost from soft clause violations in the assignment
    /// Uses working_formula (reduced after relaxation) not original formula.clauses
    fn compute_cost(&self, solver: &OLLSolver, assignment: &Assignment) -> f64 {
        let mut cost = 0.0;
        for &idx in &solver.formula.soft_clauses {
            let clause = if idx < solver.working_formula.len() {
                &solver.working_formula[idx]
            } else if idx < solver.formula.clauses.len() {
                &solver.formula.clauses[idx]
            } else {
                continue;
            };
            if !clause.is_satisfied(assignment) {
                cost += clause.weight;
            }
        }
        cost
    }

    /// Compute cost directly from the CNF formula's soft clauses
    fn compute_cost_from_cnf(&self, formula: &CNFFormula, assignment: &Assignment) -> f64 {
        let mut cost = 0.0;
        for &idx in &formula.soft_clauses {
            if !formula.clauses[idx].is_satisfied(assignment) {
                cost += formula.clauses[idx].weight;
            }
        }
        cost
    }
}

impl Default for CoreGuidedSolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Monotonic MaxSAT session used by JAGR metadata refinement.
///
/// Authoritative clauses remain resident across rounds. Objective lower bounds
/// are safe to retain because refinement only adds hard constraints, so the
/// optimum cannot improve. Internal CDCL trail state is intentionally rebuilt
/// until clause-addition reimplication is implemented and independently
/// certified.
pub struct IncrementalCoreGuidedSession {
    formula: CNFFormula,
    variant: CoreGuidedVariant,
    max_iterations: usize,
    generation: u64,
    solve_count: usize,
    certified_lower_bound: f64,
    last_result: Option<CoreGuidedResult>,
}

impl IncrementalCoreGuidedSession {
    pub fn new(formula: CNFFormula, variant: CoreGuidedVariant, max_iterations: usize) -> Self {
        Self {
            formula,
            variant,
            max_iterations,
            generation: 0,
            solve_count: 0,
            certified_lower_bound: 0.0,
            last_result: None,
        }
    }

    pub fn add_hard(&mut self, clause: super::core_guided_lp::Clause) {
        self.formula.add_hard(clause);
        self.generation += 1;
    }

    pub fn solve(&mut self) -> Result<CoreGuidedResult, String> {
        let solver =
            CoreGuidedSolver::with_variant(self.variant).with_max_iterations(self.max_iterations);
        let mut result = solver.solve(self.formula.clone())?;
        // CDCL may omit variables whose false value is sufficient. Materialize
        // them before validating the monotonic hard-clause frontier.
        for variable in 0..self.formula.num_vars {
            result.assignment.entry(variable).or_insert(false);
        }
        if self
            .formula
            .hard_clauses
            .iter()
            .any(|&index| !self.formula.clauses[index].is_satisfied(&result.assignment))
        {
            return Err("incremental JAGR model does not satisfy the hard frontier".to_string());
        }
        result.cost = self
            .formula
            .soft_clauses
            .iter()
            .filter(|&&index| !self.formula.clauses[index].is_satisfied(&result.assignment))
            .map(|&index| self.formula.clauses[index].weight)
            .sum();
        self.solve_count += 1;
        if result.is_optimal {
            self.certified_lower_bound = self.certified_lower_bound.max(result.cost);
        }
        self.last_result = Some(result.clone());
        Ok(result)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn solve_count(&self) -> usize {
        self.solve_count
    }

    pub fn certified_lower_bound(&self) -> f64 {
        self.certified_lower_bound
    }

    pub fn formula(&self) -> &CNFFormula {
        &self.formula
    }

    pub fn last_result(&self) -> Option<&CoreGuidedResult> {
        self.last_result.as_ref()
    }
}

/// Convenience: solve a MaxSMT instance using core-guided approach
pub fn solve_maxsmt_core_guided(instance: &MaxSMTInstance) -> Result<CoreGuidedResult, String> {
    let cnf = instance.to_cnf();
    let solver = CoreGuidedSolver::new();
    solver.solve(cnf)
}

#[cfg(test)]
mod tests {
    use super::super::core_guided_lp::{Clause, Literal};
    use super::*;

    #[test]
    fn test_core_guided_solver() {
        let mut formula = CNFFormula::new(2);
        formula.add_hard(Clause::new(vec![Literal::positive(0)]));
        formula.add_soft(Clause::new(vec![Literal::positive(1)]));

        let solver = CoreGuidedSolver::new();
        let result = solver.solve(formula);

        assert!(result.is_ok());
    }

    #[test]
    fn test_core_guided_ol3ps() {
        let mut formula = CNFFormula::new(3);
        formula.add_hard(Clause::new(vec![
            Literal::positive(0),
            Literal::positive(1),
        ]));
        formula.add_soft(Clause::new(vec![Literal::negative(0)]));
        formula.add_soft(Clause::new(vec![Literal::negative(1)]));

        let solver = CoreGuidedSolver::with_variant(CoreGuidedVariant::OL3PS);
        let result = solver.solve(formula);

        assert!(result.is_ok());
    }

    #[test]
    fn incremental_session_accepts_monotonic_hard_constraints() {
        let mut formula = CNFFormula::new(1);
        formula.add_soft(Clause::new(vec![Literal::positive(0)]));
        let mut session = IncrementalCoreGuidedSession::new(formula, CoreGuidedVariant::OL3PS, 100);

        let first = session.solve().unwrap();
        assert_eq!(first.assignment.get(&0), Some(&true));
        session.add_hard(Clause::new(vec![Literal::negative(0)]));
        let second = session.solve().unwrap();
        assert_eq!(second.assignment.get(&0), Some(&false));
        assert_eq!(session.generation(), 1);
        assert_eq!(session.solve_count(), 2);
    }

    #[test]
    fn incremental_session_retains_safe_objective_bound() {
        let mut formula = CNFFormula::new(1);
        formula.add_soft(Clause::with_weight(vec![Literal::positive(0)], 2.0));
        let mut session = IncrementalCoreGuidedSession::new(formula, CoreGuidedVariant::OL3PS, 100);
        session.solve().unwrap();
        let first_bound = session.certified_lower_bound();
        session.add_hard(Clause::new(vec![Literal::negative(0)]));
        session.solve().unwrap();
        assert!(session.certified_lower_bound() >= first_bound);
    }
}
