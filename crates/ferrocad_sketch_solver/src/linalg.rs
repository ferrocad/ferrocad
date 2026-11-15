//! Dense linear algebra for the solver.
//!
//! Sketches are small (tens of parameters), so a dense normal-equation solve and
//! a modified Gram-Schmidt rank analysis are the right size of machinery: no
//! matrix library, no sparse solver, and nothing that needs a C++ dependency.

use crate::eval::Row;

/// A row whose component of the previous row space is shorter than this is
/// treated as dependent. Rows are normalized first, so the threshold is relative
/// to a unit row.
const RANK_TOLERANCE: f64 = 1e-7;

/// Rank, redundancy and dependent-cycle metadata from the Gram-Schmidt analysis.
pub(crate) struct RankAnalysis {
    /// The number of linearly independent Jacobian rows.
    pub(crate) rank: usize,
    /// The constraints whose rows all added zero rank.
    pub(crate) redundant: Vec<usize>,
    /// The constraints that are part of an over-constrained linear cycle: either
    /// one of their rows was dependent, or a dependent row projected onto a basis
    /// direction they contributed.
    pub(crate) dependent_cycle: Vec<bool>,
}

/// `Σ r²`: the Levenberg-Marquardt cost.
pub(crate) fn sum_of_squares(rows: &[Row]) -> f64 {
    rows.iter().map(|row| row.value * row.value).sum()
}

/// The largest `|r|`, which is the convergence measure.
pub(crate) fn max_abs(rows: &[Row]) -> f64 {
    rows.iter()
        .fold(0.0_f64, |worst, row| worst.max(row.value.abs()))
}

/// `(JᵀJ, Jᵀr)` for the residual rows.
pub(crate) fn normal_equations(rows: &[Row], size: usize) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut normal = vec![vec![0.0; size]; size];
    let mut gradient = vec![0.0; size];
    for row in rows {
        for i in 0..size {
            let left = row.gradient[i];
            if left == 0.0 {
                continue;
            }
            gradient[i] += left * row.value;
            for j in i..size {
                normal[i][j] += left * row.gradient[j];
            }
        }
    }
    for i in 0..size {
        for j in 0..i {
            normal[i][j] = normal[j][i];
        }
    }
    (normal, gradient)
}

/// Solve `A x = b` for a symmetric positive-definite `A` by Cholesky; `None`
/// when `A` is not positive definite (the caller raises the damping and retries).
pub(crate) fn cholesky_solve(mut a: Vec<Vec<f64>>, b: &[f64]) -> Option<Vec<f64>> {
    let size = a.len();
    if size == 0 {
        return Some(Vec::new());
    }
    for i in 0..size {
        for j in 0..=i {
            let mut sum = a[i][j];
            for k in 0..j {
                sum -= a[i][k] * a[j][k];
            }
            if i == j {
                if !(sum > 0.0) {
                    return None;
                }
                a[i][i] = sum.sqrt();
            } else {
                a[i][j] = sum / a[j][j];
            }
        }
    }
    // L y = b, then Lᵀ x = y.
    let mut y = vec![0.0; size];
    for i in 0..size {
        let mut sum = b[i];
        for k in 0..i {
            sum -= a[i][k] * y[k];
        }
        y[i] = sum / a[i][i];
    }
    let mut x = vec![0.0; size];
    for i in (0..size).rev() {
        let mut sum = y[i];
        for k in (i + 1)..size {
            sum -= a[k][i] * x[k];
        }
        x[i] = sum / a[i][i];
    }
    Some(x)
}

/// The rank of the constraint Jacobian, the constraints that add nothing to it,
/// and the constraints that take part in a rank-dependent cycle.
///
/// The rows are processed **in constraint order** and orthonormalized against
/// everything already accepted, which is what makes the answer meaningful: a
/// constraint is reported as redundant when it says nothing its predecessors did
/// not already say, so the *later* of two identical constraints is the one
/// named — the same convention a reader expects from upstream's list.
///
/// A constraint with several rows is redundant only when *every* row is
/// dependent; a partly dependent constraint still constrains something, and
/// calling it redundant would make `delConstraint` on it destructive.
///
/// `dependent_cycle` is the wider statement: a row that turns out to be a
/// combination of earlier rows names *both* itself and the constraints that own
/// the directions it consumed. That is the candidate set the conflict diagnosis
/// filters on — an over-constrained cycle has no single obviously-guilty member,
/// so all of its members have to be tried.
pub(crate) fn rank_analysis(groups: &[Vec<Vec<f64>>]) -> RankAnalysis {
    let mut basis: Vec<Vec<f64>> = Vec::new();
    let mut basis_owners: Vec<usize> = Vec::new();
    let mut redundant = Vec::new();
    let mut dependent_cycle = vec![false; groups.len()];

    for (index, rows) in groups.iter().enumerate() {
        let mut added = 0;
        for row in rows {
            let mut vector = row.clone();
            if normalize(&mut vector).is_some() {
                let mut touched_basis = vec![false; basis.len()];
                for _ in 0..2 {
                    for (slot, direction) in basis.iter().enumerate() {
                        let projection = dot(direction, &vector);
                        if projection.abs() > RANK_TOLERANCE {
                            touched_basis[slot] = true;
                        }
                        for (component, base) in vector.iter_mut().zip(direction) {
                            *component -= projection * base;
                        }
                    }
                }
                let length = dot(&vector, &vector).sqrt();
                if length > RANK_TOLERANCE {
                    let direction = vector.iter().map(|value| value / length).collect();
                    basis.push(direction);
                    basis_owners.push(index);
                    added += 1;
                } else {
                    dependent_cycle[index] = true;
                    for (slot, touched) in touched_basis.into_iter().enumerate() {
                        if touched {
                            dependent_cycle[basis_owners[slot]] = true;
                        }
                    }
                }
            } else {
                dependent_cycle[index] = true;
            }
        }
        if added == 0 {
            redundant.push(index);
        }
    }

    RankAnalysis {
        rank: basis.len(),
        redundant,
        dependent_cycle,
    }
}

/// Normalize in place; `None` for the zero vector.
fn normalize(vector: &mut [f64]) -> Option<f64> {
    let length = dot(vector, vector).sqrt();
    if !length.is_finite() || length == 0.0 {
        return None;
    }
    for value in vector.iter_mut() {
        *value /= length;
    }
    Some(length)
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(left, right)| left * right).sum()
}
