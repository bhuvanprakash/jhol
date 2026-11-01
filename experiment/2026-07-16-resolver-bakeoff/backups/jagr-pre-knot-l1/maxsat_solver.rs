#![allow(dead_code)]
//! JAGR Layer 1: Max-SMT Solver with Hard/Soft Constraints
//!
//! Implements the PACSOLVE/MAXNPM approach from ICSE 2023:
//! "Flexible and Optimal Dependency Management via Max-SMT"
//!
//! This handles the full Max-SMT problem with:
//! - Hard constraints (must be satisfied): hard dependencies, version conflicts
//! - Soft constraints (preferences): newest version, minimal changes, security
//!
//! Each package-version pair becomes a Boolean variable.
//! Hard constraints = dependencies/conflicts.
//! Soft constraints = preferences (with weights).

use super::core_guided_lp::{Assignment, CNFFormula, Clause, Literal};
use super::ol3p_solver::OLLSolver;
use std::collections::{BTreeMap, HashMap};

/// Package identifier
pub type PackageId = String;

/// Version of a package
pub type Version = String;

/// Type of constraint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstraintType {
    /// Hard: must be satisfied
    Hard,
    /// Soft: preference, may be violated
    Soft,
    /// Weighted soft: preference with weight
    WeightedSoft,
}

/// Package-version variable
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageVersionVar {
    pub package: PackageId,
    pub version: Version,
}

impl PackageVersionVar {
    pub fn new(package: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            version: version.into(),
        }
    }
}

/// Dependency constraint
#[derive(Debug, Clone)]
pub struct DependencyConstraint {
    /// If this package-version is selected, these are required
    pub from: PackageVersionVar,
    /// Required version range for the dependency
    pub to_package: PackageId,
    pub to_versions: Vec<Version>,
    /// Constraint type
    pub constraint_type: ConstraintType,
    /// Weight (for soft constraints)
    pub weight: f64,
}

/// Conflict constraint (peer dependencies, etc.)
#[derive(Debug, Clone)]
pub struct ConflictConstraint {
    /// These two package-versions cannot both be installed
    pub pkg1: PackageVersionVar,
    pub pkg2: PackageVersionVar,
}

/// MaxSMT problem instance
#[derive(Debug, Clone)]
pub struct MaxSMTInstance {
    /// All available package-versions (Boolean variables)
    pub variables: Vec<PackageVersionVar>,
    /// Map from (package, version) to variable index
    pub var_map: HashMap<(PackageId, Version), usize>,
    /// Hard constraints (dependencies)
    pub dependencies: Vec<DependencyConstraint>,
    /// Conflict constraints
    pub conflicts: Vec<ConflictConstraint>,
    /// Soft constraints (preferences) — implication-based (legacy, may be tautological)
    pub soft_constraints: Vec<DependencyConstraint>,
    /// Raw unit soft clauses: (variable_index, weight) — "prefer this var to be true"
    pub soft_unit_clauses: Vec<(usize, f64)>,
    /// Root package (must be installed)
    pub root: Option<PackageVersionVar>,
}

impl MaxSMTInstance {
    pub fn new() -> Self {
        Self {
            variables: Vec::new(),
            var_map: HashMap::new(),
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            soft_constraints: Vec::new(),
            soft_unit_clauses: Vec::new(),
            root: None,
        }
    }

    /// Add a package-version variable
    pub fn add_variable(&mut self, pkg: PackageId, ver: Version) -> usize {
        if let Some(&idx) = self.var_map.get(&(pkg.clone(), ver.clone())) {
            return idx;
        }
        let var = PackageVersionVar::new(pkg.clone(), ver.clone());
        let idx = self.variables.len();
        self.var_map.insert((pkg, ver), idx);
        self.variables.push(var);
        idx
    }

    /// Add a hard dependency constraint
    pub fn add_dependency(&mut self, dep: DependencyConstraint) {
        assert_eq!(dep.constraint_type, ConstraintType::Hard);
        self.dependencies.push(dep);
    }

    /// Add a conflict
    pub fn add_conflict(&mut self, c: ConflictConstraint) {
        self.conflicts.push(c);
    }

    /// Add a soft preference.
    /// If from and to_package are the SAME package, the implication ¬from ∨ to is
    /// tautological under at-most-one encoding (can't have both true simultaneously),
    /// so we redirect to `add_unit_soft` for the "from" version variable instead.
    /// For cross-package preferences, the implication encoding is kept.
    pub fn add_preference(&mut self, dep: DependencyConstraint) {
        if dep.from.package == dep.to_package {
            if let Some(&from_idx) = self
                .var_map
                .get(&(dep.from.package.clone(), dep.from.version.clone()))
            {
                self.add_unit_soft(from_idx, dep.weight);
                return;
            }
        }
        self.soft_constraints.push(dep);
    }

    /// Add a unit soft clause: prefer variable at `var_idx` to be true, with given weight.
    /// Unlike implication-based soft preferences, these are never tautological.
    pub fn add_unit_soft(&mut self, var_idx: usize, weight: f64) {
        self.soft_unit_clauses.push((var_idx, weight));
    }

    /// Set the root package (must be installed)
    pub fn set_root(&mut self, root: PackageVersionVar) {
        self.root = Some(root);
    }

    /// Convert to CNF for SAT solving
    pub fn to_cnf(&self) -> CNFFormula {
        let mut formula = CNFFormula::new(self.variables.len());

        // Add dependencies as hard clauses
        for dep in &self.dependencies {
            let from_idx = match self
                .var_map
                .get(&(dep.from.package.clone(), dep.from.version.clone()))
            {
                Some(&idx) => idx,
                None => continue,
            };

            if dep.to_versions.is_empty() {
                let clause = Clause::new(vec![Literal::negative(from_idx)]);
                formula.add_hard(clause);
            } else {
                let to_lits: Vec<Literal> = dep
                    .to_versions
                    .iter()
                    .filter_map(|v| {
                        self.var_map
                            .get(&(dep.to_package.clone(), v.clone()))
                            .map(|&idx| Literal::positive(idx))
                    })
                    .collect();

                if to_lits.is_empty() {
                    let clause = Clause::new(vec![Literal::negative(from_idx)]);
                    formula.add_hard(clause);
                } else {
                    let mut lits = vec![Literal::negative(from_idx)];
                    lits.extend(to_lits);
                    formula.add_hard(Clause::new(lits));
                }
            }
        }

        // Add conflicts
        for conflict in &self.conflicts {
            let i1 = match self
                .var_map
                .get(&(conflict.pkg1.package.clone(), conflict.pkg1.version.clone()))
            {
                Some(&idx) => idx,
                None => continue,
            };
            let i2 = match self
                .var_map
                .get(&(conflict.pkg2.package.clone(), conflict.pkg2.version.clone()))
            {
                Some(&idx) => idx,
                None => continue,
            };

            let clause = Clause::new(vec![Literal::negative(i1), Literal::negative(i2)]);
            formula.add_hard(clause);
        }

        // Add version uniqueness (at most one version per package)
        let mut by_package: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (idx, var) in self.variables.iter().enumerate() {
            by_package.entry(&var.package).or_default().push(idx);
        }

        let mut max_var = self.variables.len().saturating_sub(1);
        for (_, indices) in &by_package {
            let local_max = indices.iter().max().copied().unwrap_or(0);
            max_var = max_var.max(local_max);
        }

        for (_, indices) in by_package {
            let amo_clauses = super::ol3p_solver::at_most_one_commander(&indices);
            for clause in &amo_clauses {
                for lit in &clause.literals {
                    max_var = max_var.max(lit.var);
                }
                formula.add_hard(clause.clone());
            }
        }

        // Ensure the formula tracks enough variables for the auxiliary
        // variables created by the commander encoding. Without this, the
        // CDCL solver's assigns/levels arrays are too small, causing
        // out-of-bounds access or missed propagation.
        if max_var >= formula.num_vars {
            formula.num_vars = max_var + 1;
        }

        // Add root constraint (must be installed)
        // When root version is "*" or "latest", at least one version of the
        // root package must be selected. Otherwise, the specific version must be true.
        if let Some(root) = &self.root {
            if root.version == "*" || root.version == "latest" {
                let root_lits: Vec<Literal> = self
                    .variables
                    .iter()
                    .enumerate()
                    .filter(|(_, var)| var.package == root.package)
                    .map(|(idx, _)| Literal::positive(idx))
                    .collect();
                if !root_lits.is_empty() {
                    formula.add_hard(Clause::new(root_lits));
                } else {
                    formula.add_hard(Clause::new(vec![]));
                }
            } else if let Some(&idx) = self
                .var_map
                .get(&(root.package.clone(), root.version.clone()))
            {
                let clause = Clause::new(vec![Literal::positive(idx)]);
                formula.add_hard(clause);
            } else {
                formula.add_hard(Clause::new(vec![]));
            }
        }

        // Add soft constraints
        for soft in &self.soft_constraints {
            let from_idx = match self
                .var_map
                .get(&(soft.from.package.clone(), soft.from.version.clone()))
            {
                Some(&idx) => idx,
                None => continue,
            };

            let to_lits: Vec<Literal> = soft
                .to_versions
                .iter()
                .filter_map(|v| {
                    self.var_map
                        .get(&(soft.to_package.clone(), v.clone()))
                        .map(|&idx| Literal::positive(idx))
                })
                .collect();

            if !to_lits.is_empty() {
                let mut lits = vec![Literal::negative(from_idx)];
                lits.extend(to_lits);
                formula.add_soft(Clause::with_weight(lits, soft.weight));
            } else {
                formula.add_soft(Clause::with_weight(
                    vec![Literal::negative(from_idx)],
                    soft.weight,
                ));
            }
        }

        // Add unit soft clauses: "prefer x_i to be true" with weight
        for &(var_idx, weight) in &self.soft_unit_clauses {
            formula.add_soft(Clause::with_weight(
                vec![Literal::positive(var_idx)],
                weight,
            ));
        }

        formula
    }
}

/// Result of MaxSMT solving
#[derive(Debug, Clone)]
pub struct MaxSMTResult {
    /// Selected package-versions
    pub selection: HashMap<PackageId, Version>,
    /// Whether all hard constraints are satisfied
    pub is_satisfiable: bool,
    /// Cost (sum of violated soft weights)
    pub cost: f64,
    /// Number of violated soft constraints
    pub num_violated: usize,
    /// Solution is optimal
    pub is_optimal: bool,
}

/// MaxSMT Solver for JAGR Layer 1
pub struct MaxSMTSolver {
    instance: MaxSMTInstance,
    /// Underlying OLL solver
    oll_solver: Option<OLLSolver>,
}

impl MaxSMTSolver {
    pub fn new(instance: MaxSMTInstance) -> Self {
        Self {
            instance,
            oll_solver: None,
        }
    }

    /// Solve the MaxSMT instance
    pub fn solve(&mut self) -> Result<MaxSMTResult, String> {
        let cnf = self.instance.to_cnf();

        let mut hard_only = cnf.clone();
        hard_only.clauses = cnf
            .hard_clauses
            .iter()
            .map(|&i| cnf.clauses[i].clone())
            .collect();
        hard_only.soft_clauses = Vec::new();
        hard_only.hard_clauses = (0..hard_only.clauses.len()).collect();

        let hard_solver = OLLSolver::new(hard_only);
        let (sat, _model) = hard_solver.dpll_solve(&hard_solver.formula.clauses);

        if !sat {
            return Ok(MaxSMTResult {
                selection: HashMap::new(),
                is_satisfiable: false,
                cost: 0.0,
                num_violated: 0,
                is_optimal: true,
            });
        }

        let mut oll_solver = OLLSolver::new(cnf);
        let result = oll_solver.solve()?;

        let mut selection: HashMap<PackageId, Version> = HashMap::new();
        for (&var, &val) in &result {
            if var < self.instance.variables.len() && val {
                let pv = &self.instance.variables[var];
                selection
                    .entry(pv.package.clone())
                    .and_modify(|existing| {
                        if super::jagr_resolver::compare_versions(&pv.version, existing)
                            == std::cmp::Ordering::Greater
                        {
                            *existing = pv.version.clone();
                        }
                    })
                    .or_insert_with(|| pv.version.clone());
            }
        }

        let cost = self.compute_cost(&result);
        let num_violated = self.count_violated(&result);

        Ok(MaxSMTResult {
            selection,
            is_satisfiable: true,
            cost,
            num_violated,
            is_optimal: true,
        })
    }

    /// Compute cost of an assignment
    fn compute_cost(&self, assignment: &Assignment) -> f64 {
        let mut cost = 0.0;
        for soft in &self.instance.soft_constraints {
            let from_idx = match self
                .instance
                .var_map
                .get(&(soft.from.package.clone(), soft.from.version.clone()))
            {
                Some(&idx) => idx,
                None => continue,
            };

            if let Some(&val) = assignment.get(&from_idx) {
                if val {
                    let to_lits: Vec<Literal> = soft
                        .to_versions
                        .iter()
                        .filter_map(|v| {
                            self.instance
                                .var_map
                                .get(&(soft.to_package.clone(), v.clone()))
                                .map(|&idx| Literal::positive(idx))
                        })
                        .collect();

                    let to_sat = to_lits.iter().any(|lit| {
                        let var = lit.var;
                        let val = assignment.get(&var).copied().unwrap_or(false);
                        if lit.is_positive() {
                            val
                        } else {
                            !val
                        }
                    });

                    if !to_sat {
                        cost += soft.weight;
                    }
                }
            }
        }
        for &(var_idx, weight) in &self.instance.soft_unit_clauses {
            if !assignment.get(&var_idx).copied().unwrap_or(false) {
                cost += weight;
            }
        }
        cost
    }

    /// Count violated soft constraints
    fn count_violated(&self, assignment: &Assignment) -> usize {
        let mut count = 0;
        for soft in &self.instance.soft_constraints {
            let from_idx = match self
                .instance
                .var_map
                .get(&(soft.from.package.clone(), soft.from.version.clone()))
            {
                Some(&idx) => idx,
                None => continue,
            };

            if let Some(&val) = assignment.get(&from_idx) {
                if val {
                    let to_lits: Vec<Literal> = soft
                        .to_versions
                        .iter()
                        .filter_map(|v| {
                            self.instance
                                .var_map
                                .get(&(soft.to_package.clone(), v.clone()))
                                .map(|&idx| Literal::positive(idx))
                        })
                        .collect();

                    let clause = Clause::new({
                        let mut lits = vec![Literal::negative(from_idx)];
                        lits.extend(to_lits);
                        lits
                    });

                    if !clause.is_satisfied(assignment) {
                        count += 1;
                    }
                }
            }
        }
        for &(var_idx, _weight) in &self.instance.soft_unit_clauses {
            if !assignment.get(&var_idx).copied().unwrap_or(false) {
                count += 1;
            }
        }
        count
    }
}

/// Convert dependency graph to MaxSMT instance
pub fn build_maxsmt_from_dependencies(
    root_pkg: PackageId,
    root_ver: Version,
    dependencies: &[(PackageId, Version, Vec<(PackageId, Version)>)], // (from, from_ver, [(to_pkg, to_ver)])
    versions: &HashMap<PackageId, Vec<Version>>,
) -> MaxSMTInstance {
    let mut instance = MaxSMTInstance::new();

    // Add variables
    for (pkg, vers) in versions {
        for ver in vers {
            instance.add_variable(pkg.clone(), ver.clone());
        }
    }

    // Add root
    instance.set_root(PackageVersionVar::new(root_pkg.clone(), root_ver.clone()));

    // Add dependencies
    for (from_pkg, from_ver, deps) in dependencies {
        let from_var = PackageVersionVar::new(from_pkg.clone(), from_ver.clone());

        for (to_pkg, to_ver) in deps {
            let dep = DependencyConstraint {
                from: from_var.clone(),
                to_package: to_pkg.clone(),
                to_versions: vec![to_ver.clone()],
                constraint_type: ConstraintType::Hard,
                weight: 0.0,
            };
            instance.add_dependency(dep);
        }
    }

    instance
}

/// Add soft preference: prefer latest version
pub fn add_version_preference(
    instance: &mut MaxSMTInstance,
    pkg: PackageId,
    preferred_versions: Vec<Version>,
    weight: f64,
) {
    // For each version of pkg, prefer the highest version
    // Add soft constraint: v_i → v_{i+1} (prefer newest)

    for i in 0..preferred_versions.len().saturating_sub(1) {
        let from_var = PackageVersionVar::new(pkg.clone(), preferred_versions[i + 1].clone());
        let to_versions = vec![preferred_versions[i].clone()];

        let soft = DependencyConstraint {
            from: from_var,
            to_package: pkg.clone(),
            to_versions,
            constraint_type: ConstraintType::WeightedSoft,
            weight,
        };
        instance.add_preference(soft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maxsmt_instance() {
        let mut instance = MaxSMTInstance::new();

        // Add A 1.0, A 2.0, B 1.0
        instance.add_variable("A".to_string(), "1.0".to_string());
        instance.add_variable("A".to_string(), "2.0".to_string());
        instance.add_variable("B".to_string(), "1.0".to_string());

        // A 1.0 depends on B 1.0
        let dep = DependencyConstraint {
            from: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            to_package: "B".to_string(),
            to_versions: vec!["1.0".to_string()],
            constraint_type: ConstraintType::Hard,
            weight: 0.0,
        };
        instance.add_dependency(dep);

        // Set root
        instance.set_root(PackageVersionVar::new("A".to_string(), "1.0".to_string()));

        let cnf = instance.to_cnf();
        assert!(!cnf.hard_clauses.is_empty());
    }

    #[test]
    fn test_maxsmt_solve() {
        let mut instance = MaxSMTInstance::new();

        instance.add_variable("A".to_string(), "1.0".to_string());
        instance.add_variable("A".to_string(), "2.0".to_string());
        instance.add_variable("B".to_string(), "1.0".to_string());

        let dep = DependencyConstraint {
            from: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            to_package: "B".to_string(),
            to_versions: vec!["1.0".to_string()],
            constraint_type: ConstraintType::Hard,
            weight: 0.0,
        };
        instance.add_dependency(dep);

        instance.set_root(PackageVersionVar::new("A".to_string(), "2.0".to_string()));

        let mut solver = MaxSMTSolver::new(instance);
        let result = solver.solve();

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_satisfiable);
    }

    #[test]
    fn test_unsatisfiable() {
        let mut instance = MaxSMTInstance::new();

        instance.add_variable("A".to_string(), "1.0".to_string());
        instance.add_variable("B".to_string(), "1.0".to_string());

        // A 1.0 requires B 2.0 (not available)
        let dep = DependencyConstraint {
            from: PackageVersionVar::new("A".to_string(), "1.0".to_string()),
            to_package: "B".to_string(),
            to_versions: vec!["2.0".to_string()],
            constraint_type: ConstraintType::Hard,
            weight: 0.0,
        };
        instance.add_dependency(dep);

        instance.set_root(PackageVersionVar::new("A".to_string(), "1.0".to_string()));

        let mut solver = MaxSMTSolver::new(instance);
        let result = solver.solve().unwrap();

        assert!(!result.is_satisfiable);
    }
}
