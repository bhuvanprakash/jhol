//! JAGR Layer 3: Tropical Geometry - Max-Plus Semiring Mathematics
//!
//! This module implements the tropical (max-plus) semiring algebra that forms
//! the foundation of JAGR's polynomial-time feasibility filter.
//!
//! Tropical semiring: (ℝ ∪ {-∞}, ⊕, ⊗) where:
//! - Tropical addition: a ⊕ b = max(a, b)
//! - Tropical multiplication: a ⊗ b = a + b
//! - Tropical zero: -∞ (identity for ⊕)
//! - Tropical one: 0 (identity for ⊗)

use std::ops::{Add, Mul, Neg};

/// The tropical value type: real numbers extended with -∞
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Tropical(pub f64);

impl Tropical {
    /// Tropical zero element (-∞)
    pub const ZERO: Self = Self(f64::NEG_INFINITY);

    /// Tropical one element (0)
    pub const ONE: Self = Self(0.0);

    /// Create a new tropical value
    pub const fn new(val: f64) -> Self {
        Self(val)
    }

    /// Check if this is tropical zero (-∞)
    pub fn is_zero(&self) -> bool {
        self.0.is_infinite() && self.0.is_sign_negative()
    }

    /// Get the underlying value
    pub fn value(&self) -> f64 {
        self.0
    }
}

/// Tropical addition: a ⊕ b = max(a, b)
impl Add for Tropical {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.max(rhs.0))
    }
}

/// Tropical multiplication: a ⊗ b = a + b
impl Mul for Tropical {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

/// Tropical negation is not defined in the semiring, but we provide
/// the standard negation for use in algorithms
impl Neg for Tropical {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

/// Tropical scalar multiplication: λ ⊗ x = λ + x
impl Mul<f64> for Tropical {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl Mul<Tropical> for f64 {
    type Output = Tropical;

    fn mul(self, rhs: Tropical) -> Self::Output {
        Tropical(self + rhs.0)
    }
}

/// Tropical division: a ⊘ b = a - b (in max-plus)
impl std::ops::Div for Tropical {
    type Output = Self;

    fn div(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

/// Tropical exponentiation: a^b = b ⊗ a = b * a (standard multiplication)
impl std::ops::BitXor for Tropical {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        Self(self.0 * rhs.0)
    }
}

/// Tropical matrix: m × n matrix over the tropical semiring
#[derive(Debug, Clone)]
pub struct TropicalMatrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<Tropical>,
}

impl TropicalMatrix {
    /// Create a new tropical matrix filled with tropical zero (-∞)
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![Tropical::ZERO; rows * cols],
        }
    }

    /// Create from a 2D slice
    pub fn from_slice(data: &[&[f64]]) -> Self {
        let rows = data.len();
        let cols = if rows > 0 { data[0].len() } else { 0 };
        let mut matrix = Self::new(rows, cols);
        for (i, row) in data.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                matrix.set(i, j, Tropical::new(val));
            }
        }
        matrix
    }

    /// Get element at (row, col)
    pub fn get(&self, row: usize, col: usize) -> Tropical {
        self.data[row * self.cols + col]
    }

    /// Set element at (row, col)
    pub fn set(&mut self, row: usize, col: usize, val: Tropical) {
        self.data[row * self.cols + col] = val;
    }

    /// Tropical identity matrix: diagonal = 0 (tropical one), off-diagonal = -∞ (tropical zero)
    pub fn identity(size: usize) -> Self {
        let mut mat = Self::new(size, size);
        for i in 0..size {
            mat.set(i, i, Tropical::ONE);
        }
        mat
    }

    /// Binary exponentiation by squaring in tropical semiring: (A)^exp in O(log exp)
    pub fn pow_tropical(&self, mut exp: usize) -> Self {
        assert_eq!(self.rows, self.cols, "Matrix power requires square matrix");
        let n = self.rows;
        if exp == 0 {
            return Self::identity(n);
        }
        let mut base = self.clone();
        let mut result = Self::identity(n);

        while exp > 0 {
            if exp % 2 == 1 {
                result = result.mul(&base);
            }
            if exp > 1 {
                base = base.mul(&base);
            }
            exp /= 2;
        }
        result
    }

    /// Fast transitive closure / all-pairs longest constraint path in O(log D) matrix multiplications
    /// Computes (I ⊕ A)^depth via binary exponentiation.
    pub fn transitive_closure_fast(&self, depth: usize) -> Self {
        assert_eq!(
            self.rows, self.cols,
            "Transitive closure requires square matrix"
        );
        let ident = Self::identity(self.rows);
        let augmented = ident.add(self);
        augmented.pow_tropical(depth)
    }

    /// Tropical matrix multiplication: (A ⊗ B)_{ij} = ⊕_k (A_{ik} ⊗ B_{kj})
    /// = max_k (A_{ik} + B_{kj})
    pub fn mul(&self, other: &Self) -> Self {
        assert_eq!(self.cols, other.rows, "Matrix dimension mismatch");
        let mut result = Self::new(self.rows, other.cols);

        for i in 0..self.rows {
            for j in 0..other.cols {
                let mut max_val = Tropical::ZERO;
                for k in 0..self.cols {
                    let prod = self.get(i, k) * other.get(k, j);
                    max_val = max_val + prod;
                }
                result.set(i, j, max_val);
            }
        }
        result
    }

    /// Tropical matrix-vector multiplication
    pub fn mul_vec(&self, vec: &[Tropical]) -> Vec<Tropical> {
        assert_eq!(self.cols, vec.len(), "Matrix-vector dimension mismatch");
        let mut result = vec![Tropical::ZERO; self.rows];

        for i in 0..self.rows {
            let mut max_val = Tropical::ZERO;
            for j in 0..self.cols {
                let prod = self.get(i, j) * vec[j];
                max_val = max_val + prod;
            }
            result[i] = max_val;
        }
        result
    }

    /// Tropical matrix addition (element-wise max)
    pub fn add(&self, other: &Self) -> Self {
        assert_eq!(self.rows, other.rows, "Row dimension mismatch");
        assert_eq!(self.cols, other.cols, "Col dimension mismatch");
        let mut result = Self::new(self.rows, self.cols);
        for i in 0..self.rows {
            for j in 0..self.cols {
                result.set(i, j, self.get(i, j) + other.get(i, j));
            }
        }
        result
    }

    /// Transpose
    pub fn transpose(&self) -> Self {
        let mut result = Self::new(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                result.set(j, i, self.get(i, j));
            }
        }
        result
    }

    /// Tropical permanent: per(A) = ⊕_{σ∈S_n} ⊗_{i=1}^n A_{i,σ(i)}
    /// = max_{σ∈S_n} Σ_i A_{i,σ(i)}
    pub fn tropical_permanent(&self) -> Tropical {
        assert_eq!(self.rows, self.cols, "Permanent requires square matrix");
        self.tropical_determinant()
    }

    /// Tropical determinant (same as permanent in max-plus)
    pub fn tropical_determinant(&self) -> Tropical {
        assert_eq!(self.rows, self.cols, "Determinant requires square matrix");
        let n = self.rows;

        if n <= 8 {
            self.tropical_permanent_brute_force()
        } else {
            self.tropical_determinant_kim()
        }
    }

    fn tropical_permanent_brute_force(&self) -> Tropical {
        let n = self.rows;
        let mut max_perm_sum = f64::NEG_INFINITY;

        let mut indices: Vec<usize> = (0..n).collect();

        fn permute(indices: &mut [usize], k: usize, matrix: &TropicalMatrix, max_sum: &mut f64) {
            if k == indices.len() {
                let mut sum = 0.0;
                for i in 0..indices.len() {
                    sum += matrix.get(i, indices[i]).0;
                }
                *max_sum = sum.max(*max_sum);
                return;
            }
            for i in k..indices.len() {
                indices.swap(k, i);
                permute(indices, k + 1, matrix, max_sum);
                indices.swap(k, i);
            }
        }

        permute(&mut indices, 0, self, &mut max_perm_sum);
        Tropical::new(max_perm_sum)
    }

    /// O(n³) tropical determinant using the Hungarian algorithm
    ///
    /// In tropical (max-plus) algebra, the determinant is the maximum weight
    /// of any permutation (same as the permanent). We solve this as a
    /// maximum-weight assignment problem using the Hungarian algorithm adapted
    /// for max-plus: O(n³).
    fn tropical_determinant_kim(&self) -> Tropical {
        let n = self.rows;
        let a: Vec<Vec<f64>> = (0..n)
            .map(|i| (0..n).map(|j| self.get(i, j).0).collect())
            .collect();

        // Hungarian algorithm for maximum weight assignment
        // u[i] = dual variable for row i, v[j] = dual variable for column j
        let mut u = vec![0.0; n + 1];
        let mut v = vec![0.0; n + 1];
        let mut p = vec![0usize; n + 1]; // p[j] = row matched to column j
        let mut way = vec![0usize; n + 1];

        for i in 1..=n {
            p[0] = i;
            let mut j0 = 0usize;
            let mut minv = vec![f64::NEG_INFINITY; n + 1];
            let mut used = vec![false; n + 1];

            // For max assignment, we negated the problem convention.
            // Here we adapt directly for max: we want max a[i-1][j-1].
            loop {
                used[j0] = true;
                let i0 = p[j0];
                let mut delta = f64::NEG_INFINITY;
                let mut j1 = 0usize;

                for j in 1..=n {
                    if !used[j] {
                        // Combine row dual u[i0], column dual v[j], and matrix value
                        let cur = a[i0 - 1][j - 1] + u[i0] + v[j];
                        if cur > minv[j] {
                            minv[j] = cur;
                            way[j] = j0;
                        }
                        if minv[j] > delta || delta == f64::NEG_INFINITY {
                            delta = minv[j];
                            j1 = j;
                        }
                    }
                }

                for j in 0..=n {
                    if used[j] {
                        u[p[j]] += delta;
                        v[j] -= delta;
                    } else {
                        minv[j] -= delta;
                    }
                }

                j0 = j1;
                if p[j0] == 0 {
                    break;
                }
            }

            // Augment: trace back the augmenting path
            loop {
                let j1 = way[j0];
                p[j0] = p[j1];
                j0 = j1;
                if j0 == 0 {
                    break;
                }
            }
        }

        // Compute the assignment value
        let mut det_val = 0.0;
        for j in 1..=n {
            if p[j] > 0 {
                det_val += a[p[j] - 1][j - 1];
            }
        }

        // Check if any assigned pair is -∞ (means determinant is zero in tropical)
        for j in 1..=n {
            if p[j] > 0 && a[p[j] - 1][j - 1] == f64::NEG_INFINITY {
                return Tropical::ZERO;
            }
        }

        Tropical::new(det_val)
    }

    // ─── Innovations 1,4,6,7 ──────────────────────────────────────────

    /// #1 Bin-link topology — A² diagonal = hotness. level 0 = hoist.
    pub fn bin_link_topology(&self, threshold: f64) -> Vec<(usize, usize)> {
        let sq = self.mul(self);
        (0..self.rows)
            .map(|i| (i, if sq.get(i, i).0 > threshold { 0 } else { 1 }))
            .collect()
    }
    /// #6 Spectral projection — hot files get material paths.
    pub fn spectral_projection(&self, threshold: f64) -> Vec<bool> {
        (0..self.cols)
            .map(|j| {
                (0..self.rows)
                    .filter(|&i| self.get(i, j).0.is_finite())
                    .count() as f64
                    >= threshold
            })
            .collect()
    }
    /// #7 DAG repair — edges changed from prev plan.
    pub fn dag_diff(&self, prev: &Self) -> Vec<(usize, usize, bool)> {
        let mut c = vec![];
        for i in 0..self.rows.min(prev.rows) {
            for j in 0..self.cols.min(prev.cols) {
                let cur = self.get(i, j).0;
                let old = prev.get(i, j).0;
                if (cur - old).abs() > 0.01 {
                    c.push((i, j, cur.is_finite()));
                }
            }
        }
        c
    }
    /// #4 Hoist plan serialization.
    pub fn to_hoist_plan(&self) -> Vec<String> {
        self.data.iter().map(|t| format!("{:.2}", t.0)).collect()
    }
    pub fn from_hoist_plan(rows: usize, cols: usize, plan: &[String]) -> Self {
        let mut m = Self::new(rows, cols);
        for (i, s) in plan.iter().enumerate() {
            if let Some(d) = m.data.get_mut(i) {
                if let Ok(v) = s.parse::<f64>() {
                    *d = Tropical::new(v);
                }
            }
        }
        m
    }

    /// Check if matrix is tropically diagonally dominant
    pub fn is_diagonally_dominant(&self) -> bool {
        for i in 0..self.rows.min(self.cols) {
            let diag = self.get(i, i);
            let mut max_off = Tropical::ZERO;
            for j in 0..self.cols {
                if i != j {
                    max_off = max_off + self.get(i, j);
                }
            }
            if !(diag >= max_off) {
                return false;
            }
        }
        true
    }
}

/// Tropical polynomial: P(x) = ⊕_{α} c_α ⊗ x^α
#[derive(Debug, Clone)]
pub struct TropicalPolynomial {
    pub monomials: Vec<(Tropical, Vec<usize>)>,
    pub num_vars: usize,
}

impl TropicalPolynomial {
    pub fn new(num_vars: usize) -> Self {
        Self {
            monomials: Vec::new(),
            num_vars,
        }
    }

    pub fn add_monomial(&mut self, coeff: Tropical, exponents: Vec<usize>) {
        assert_eq!(exponents.len(), self.num_vars);
        self.monomials.push((coeff, exponents));
    }

    pub fn evaluate(&self, x: &[Tropical]) -> Tropical {
        assert_eq!(x.len(), self.num_vars);
        let mut result = Tropical::ZERO;

        for (coeff, exponents) in &self.monomials {
            // Tropical monomial evaluation: coeff ⊗ ⊗ x_i^exp_i = coeff + sum(exp_i * x_i)
            // Skip zero exponents (they contribute nothing in tropical)
            let mut term_val = coeff.0;
            for (var_idx, &exp) in exponents.iter().enumerate() {
                if exp > 0 && var_idx < x.len() {
                    // exp * x[var_idx] in tropical: exp (as integer) plus x[var_idx]
                    // Since x[var_idx] is a tropical value (a real number x_i),
                    // exp * x[var_idx] = exp * x_i in real arithmetic
                    term_val += (exp as f64) * x[var_idx].0;
                }
            }
            result = result + Tropical(term_val);
        }
        result
    }

    pub fn newton_polytope(&self) -> Vec<Vec<usize>> {
        self.monomials
            .iter()
            .filter(|(c, _)| !c.is_zero())
            .map(|(_, exp)| exp.clone())
            .collect()
    }

    pub fn add(&self, other: &Self) -> Self {
        assert_eq!(self.num_vars, other.num_vars);
        let mut result = Self::new(self.num_vars);
        let mut all_monomials = self.monomials.clone();
        all_monomials.extend(other.monomials.clone());
        // Combine like terms (same exponents)
        let mut result_monomials: Vec<(Tropical, Vec<usize>)> = Vec::new();
        for (coeff, exp) in all_monomials {
            let mut found = false;
            for (existing_coeff, existing_exp) in result_monomials.iter_mut() {
                if existing_exp == &exp {
                    *existing_coeff = *existing_coeff + coeff;
                    found = true;
                    break;
                }
            }
            if !found {
                result_monomials.push((coeff, exp));
            }
        }
        result.monomials = result_monomials;
        result
    }

    pub fn mul(&self, other: &Self) -> Self {
        assert_eq!(self.num_vars, other.num_vars);
        let mut result = Self::new(self.num_vars);

        for (c1, e1) in &self.monomials {
            for (c2, e2) in &other.monomials {
                let coeff = *c1 * *c2;
                let exponents: Vec<usize> = e1.iter().zip(e2.iter()).map(|(a, b)| a + b).collect();
                result.monomials.push((coeff, exponents));
            }
        }
        // Combine like terms
        let mut result_monomials: Vec<(Tropical, Vec<usize>)> = Vec::new();
        for (coeff, exp) in result.monomials {
            let mut found = false;
            for (existing_coeff, existing_exp) in result_monomials.iter_mut() {
                if existing_exp == &exp {
                    *existing_coeff = *existing_coeff + coeff;
                    found = true;
                    break;
                }
            }
            if !found {
                result_monomials.push((coeff, exp));
            }
        }
        result.monomials = result_monomials;
        result
    }
}

/// Tropical rational function: P ⊘ Q = P / Q
#[derive(Debug, Clone)]
pub struct TropicalRationalFunction {
    pub numerator: TropicalPolynomial,
    pub denominator: TropicalPolynomial,
}

impl TropicalRationalFunction {
    pub fn new(num: TropicalPolynomial, den: TropicalPolynomial) -> Self {
        assert_eq!(num.num_vars, den.num_vars);
        Self {
            numerator: num,
            denominator: den,
        }
    }

    pub fn evaluate(&self, x: &[Tropical]) -> Tropical {
        let num_val = self.numerator.evaluate(x);
        let den_val = self.denominator.evaluate(x);
        num_val / den_val
    }
}

/// Max-atom: max(z, y) + r ≥ x
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaxAtom {
    pub z: usize,
    pub y: usize,
    pub x: usize,
    pub r: f64,
}

impl MaxAtom {
    pub fn new(z: usize, y: usize, x: usize, r: f64) -> Self {
        Self { z, y, x, r }
    }

    pub fn is_difference_constraint(&self) -> bool {
        self.z == self.y
    }

    pub fn evaluate(&self, x: &[f64]) -> bool {
        let max_zy = x[self.z].max(x[self.y]);
        max_zy + self.r >= x[self.x]
    }
}

/// System of max-atoms
#[derive(Debug, Clone)]
pub struct MaxAtomSystem {
    pub atoms: Vec<MaxAtom>,
    pub num_vars: usize,
}

impl MaxAtomSystem {
    pub fn new(num_vars: usize) -> Self {
        Self {
            atoms: Vec::new(),
            num_vars,
        }
    }

    pub fn add_atom(&mut self, atom: MaxAtom) {
        let max_var = atom.z.max(atom.y).max(atom.x);
        if max_var >= self.num_vars {
            self.num_vars = max_var + 1;
        }
        self.atoms.push(atom);
    }

    pub fn add_aux_variable(&mut self) -> usize {
        let idx = self.num_vars;
        self.num_vars += 1;
        idx
    }

    pub fn satisfies(&self, x: &[f64]) -> bool {
        self.atoms.iter().all(|a| a.evaluate(x))
    }

    pub fn to_tropical_matrices(&self) -> (TropicalMatrix, TropicalMatrix) {
        let m = self.atoms.len();
        let n = self.num_vars;

        let mut a = TropicalMatrix::new(m, n);
        let mut rhs = TropicalMatrix::new(m, n);

        for (i, atom) in self.atoms.iter().enumerate() {
            a.set(i, atom.z, a.get(i, atom.z) + Tropical::new(atom.r));
            a.set(i, atom.y, a.get(i, atom.y) + Tropical::new(atom.r));
            rhs.set(i, atom.x, rhs.get(i, atom.x) + Tropical::ONE);
        }

        (a, rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tropical_addition() {
        let a = Tropical::new(3.0);
        let b = Tropical::new(5.0);
        assert_eq!((a + b).0, 5.0);

        let zero = Tropical::ZERO;
        assert_eq!((a + zero).0, 3.0);
    }

    #[test]
    fn test_tropical_multiplication() {
        let a = Tropical::new(3.0);
        let b = Tropical::new(5.0);
        assert_eq!((a * b).0, 8.0);

        let one = Tropical::ONE;
        assert_eq!((a * one).0, 3.0);
    }

    #[test]
    fn test_tropical_matrix_mul() {
        let a = TropicalMatrix::from_slice(&[&[1.0, 2.0], &[3.0, 4.0]]);
        let b = TropicalMatrix::from_slice(&[&[5.0, 6.0], &[7.0, 8.0]]);

        let c = a.mul(&b);
        assert_eq!(c.get(0, 0).0, 9.0);
        assert_eq!(c.get(0, 1).0, 10.0);
        assert_eq!(c.get(1, 0).0, 11.0);
        assert_eq!(c.get(1, 1).0, 12.0);
    }

    #[test]
    fn test_tropical_determinant() {
        let a = TropicalMatrix::from_slice(&[&[1.0, 2.0], &[3.0, 4.0]]);
        assert_eq!(a.tropical_determinant().0, 5.0);
    }

    #[test]
    fn test_max_atom() {
        let atom = MaxAtom::new(0, 1, 2, 3.0);
        let x = vec![1.0, 4.0, 6.0];
        assert!(atom.evaluate(&x));

        let x2 = vec![1.0, 4.0, 8.0];
        assert!(!atom.evaluate(&x2));
    }

    #[test]
    fn test_max_atom_system() {
        let mut sys = MaxAtomSystem::new(3);
        sys.add_atom(MaxAtom::new(0, 1, 2, 3.0));
        sys.add_atom(MaxAtom::new(1, 2, 0, 1.0));

        let x = vec![5.0, 2.0, 6.0];
        assert!(sys.satisfies(&x));
    }

    #[test]
    fn test_tropical_matrix_pow() {
        let a = TropicalMatrix::from_slice(&[&[0.0, 2.0], &[f64::NEG_INFINITY, 0.0]]);
        let a2 = a.pow_tropical(2);
        // (A^2)_01 = max(0+2, 2+0) = 2.0
        assert_eq!(a2.get(0, 1).0, 2.0);

        let ident = a.pow_tropical(0);
        assert_eq!(ident.get(0, 0).0, 0.0);
        assert_eq!(ident.get(1, 1).0, 0.0);
        assert!(ident.get(0, 1).is_zero());
    }

    #[test]
    fn test_tropical_matrix_transitive_closure() {
        let a = TropicalMatrix::from_slice(&[
            &[f64::NEG_INFINITY, 3.0, f64::NEG_INFINITY],
            &[f64::NEG_INFINITY, f64::NEG_INFINITY, 4.0],
            &[f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
        ]);
        let closure = a.transitive_closure_fast(4);
        // Path from 0 to 2 has length 3 + 4 = 7.0
        assert_eq!(closure.get(0, 2).0, 7.0);
    }
}
