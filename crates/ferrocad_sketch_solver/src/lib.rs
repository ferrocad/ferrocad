//! FerroCAD's 2-D sketch constraint solver.
//!
//! # Why a Rust solver rather than PlanGCS
//!
//! Upstream's 2-D solver is PlanGCS, vendored C++ that needs Eigen headers and a
//! Boost graph header, under LGPL-2.1+. Binding it would add a C++ toolchain to a
//! workspace whose whole point is that the kernel is Rust, grow `target/` by the
//! Eigen closure, and — because PlanGCS is not in the sparse upstream checkout
//! this repository measures against — mean fetching it first. No contract gate
//! requires it: the sketch contract is behavioural (`solve`, DOF, redundancy), and
//! the constraint vocabulary is small and documented. This crate implements that
//! vocabulary directly: Levenberg-Marquardt on analytic residuals, plus a
//! rank-revealing analysis for DOF and redundancy. It is dependency-free apart
//! from `ferrocad_curves` for the 2-D point type.
//!
//! # What it solves
//!
//! [`Curve2d`] is the parameterized geometry: a point (2 parameters), a line
//! segment (4), a circle (3) or an arc (5) — the parameterization upstream's
//! solver uses. [`Constraint`] is one constraint record, with references to
//! geometry (0-based, in the order the sketch stores it) or to a root axis
//! ([`Reference::RootXAxis`], [`Reference::RootYAxis`]), and a [`Pos`] on that
//! reference for constraints that act on endpoints rather than whole curves.
//!
//! [`solve`] returns the solved curves, whether it converged, the remaining
//! degrees of freedom, and the constraints that are redundant or still violated.
//! It never mutates its input, and a non-converging system is reported rather than
//! partially applied.
//!
//! # The numbers
//!
//! Residuals are built with exact first derivatives (see [`eval`]), the iteration
//! is Levenberg-Marquardt with a dense Cholesky solve, and the rank of the
//! constraint Jacobian is computed by Gram-Schmidt in *constraint order*. Degrees
//! of freedom are `parameters − rank`. A constraint is redundant when its rows add
//! no rank beyond its predecessors; it is conflicting when it can be shown to be
//! keeping the sketch from solving.
//!
//! # Islands and priors
//!
//! A constraint couples only the geometry it names, so the sketch is split into
//! connected components before anything is evaluated: an unconstrained component
//! is answered from its parameter count, an already-satisfied one skips the
//! iteration but still gets its rank analysis, and an active one solves and
//! diagnoses on its own. `conflicting` is then a per-island diagnosis, so one
//! broken sub-assembly cannot smear over a healthy one.
//!
//! [`SoftPrior`] adds a weighted pull toward a target point for interactive
//! dragging. Priors are penalties: they never enter the convergence test or the
//! rank analysis, so they steer the degrees of freedom the constraints leave free
//! instead of fighting them.

mod eval;
mod linalg;

use std::fmt;

use ferrocad_curves::Vector2;

/// The residual below which the system counts as solved.
pub const RESIDUAL_TOLERANCE: f64 = 1e-9;

/// The iteration cap. A sketch that has not converged by then is reported as
/// unconverged rather than run forever.
pub const MAX_ITERATIONS: usize = 200;

/// The penalty weight [`SoftPrior`] uses by default: small enough to steer the
/// free degrees of freedom without visibly fighting a hard constraint.
pub const DEFAULT_PRIOR_WEIGHT: f64 = 1e-2;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// One geometry element as the solver sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve2d {
    /// A point: `(x, y)`.
    Point(Vector2),
    /// A bounded straight segment: `(x1, y1, x2, y2)`.
    Line { start: Vector2, end: Vector2 },
    /// A full circle: `(cx, cy, r)`.
    Circle { center: Vector2, radius: f64 },
    /// A circular arc: `(cx, cy, r, first, last)` in radians.
    Arc {
        center: Vector2,
        radius: f64,
        first: f64,
        last: f64,
    },
    /// A geometry this solver does not parameterize (an ellipse, a spline, an
    /// unbounded curve). It is carried through unchanged, contributes no degrees
    /// of freedom, and is refused as a constraint reference.
    Other,
}

impl Curve2d {
    /// The number of solver parameters this geometry contributes.
    pub fn parameters(&self) -> usize {
        match self {
            Curve2d::Point(_) => 2,
            Curve2d::Line { .. } => 4,
            Curve2d::Circle { .. } => 3,
            Curve2d::Arc { .. } => 5,
            Curve2d::Other => 0,
        }
    }
}

/// Where a constraint points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    /// The geometry at this 0-based index, in the order the sketch stores it.
    Geometry(usize),
    /// The X axis. Its start point is the sketch origin.
    RootXAxis,
    /// The Y axis. Its start point is the sketch origin too.
    RootYAxis,
}

/// Which point of a referenced geometry a constraint acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pos {
    /// The whole curve (or, where a point is unavoidable, its start).
    None,
    /// `PointPos.start`: the first endpoint.
    Start,
    /// `PointPos.end`: the last endpoint.
    End,
    /// `PointPos.mid`: the midpoint of a line, the centre of a circle or arc.
    Mid,
}

impl Pos {
    /// The value upstream's API carries (`PointPos`).
    pub fn code(self) -> i64 {
        match self {
            Pos::None => 0,
            Pos::Start => 1,
            Pos::End => 2,
            Pos::Mid => 3,
        }
    }
}

/// A geometry reference plus the point on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeoRef {
    /// The geometry or root axis.
    pub reference: Reference,
    /// The point on it.
    pub pos: Pos,
}

impl GeoRef {
    /// The whole of a geometry.
    pub fn geometry(index: usize) -> Self {
        Self {
            reference: Reference::Geometry(index),
            pos: Pos::None,
        }
    }

    /// One point of a geometry.
    pub fn geometry_at(index: usize, pos: Pos) -> Self {
        Self {
            reference: Reference::Geometry(index),
            pos,
        }
    }

    /// The X axis, read as a line (or, at [`Pos::Start`], as the origin).
    pub fn root_x() -> Self {
        Self {
            reference: Reference::RootXAxis,
            pos: Pos::None,
        }
    }

    /// The Y axis.
    pub fn root_y() -> Self {
        Self {
            reference: Reference::RootYAxis,
            pos: Pos::None,
        }
    }

    /// The sketch origin: the start of the X axis.
    pub fn origin() -> Self {
        Self {
            reference: Reference::RootXAxis,
            pos: Pos::Start,
        }
    }
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

/// The constraint vocabulary this solver implements.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Two points at the same place.
    Coincident,
    /// The first point lies on the second reference (a line or a circle).
    PointOnObject,
    /// A line is parallel to the X axis, or two points share a Y.
    Horizontal,
    /// A line is parallel to the Y axis, or two points share an X.
    Vertical,
    /// Two lines are parallel.
    Parallel,
    /// Two lines meet at a right angle.
    Perpendicular,
    /// Two curves touch: a line and a circle, or two circles (external).
    Tangent,
    /// Two lines are the same length, or two circles the same radius.
    Equal,
    /// Two points are symmetric about a third.
    Symmetric,
    /// The geometry is pinned where it was when the solve started.
    Block,
    /// A distance between two points (in the sketch plane).
    Distance(f64),
    /// A horizontal distance between two points.
    DistanceX(f64),
    /// A vertical distance between two points.
    DistanceY(f64),
    /// A circle's or arc's radius.
    Radius(f64),
    /// A circle's or arc's diameter.
    Diameter(f64),
    /// The angle between two lines, in radians.
    Angle(f64),
}

/// One constraint record.
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    /// What the constraint says.
    pub kind: Kind,
    /// The first reference.
    pub first: GeoRef,
    /// The second reference, for the kinds that take two.
    pub second: Option<GeoRef>,
    /// The third reference (`Symmetric`).
    pub third: Option<GeoRef>,
}

impl Constraint {
    /// A constraint on one reference.
    pub fn new(kind: Kind, first: GeoRef) -> Self {
        Self {
            kind,
            first,
            second: None,
            third: None,
        }
    }

    /// Add the second reference.
    pub fn with_second(mut self, second: GeoRef) -> Self {
        self.second = Some(second);
        self
    }

    /// Add the third reference.
    pub fn with_third(mut self, third: GeoRef) -> Self {
        self.third = Some(third);
        self
    }
}

/// A soft dragging target: pull a referenced point toward `target` with
/// `weight`, without counting as a hard constraint.
///
/// This is the seam a drag gesture uses. A hard constraint is a row the solve
/// must satisfy; a prior is a row it would *like* to satisfy, so it steers the
/// degrees of freedom the constraints leave free and never appears in the
/// convergence test or the rank analysis. Dragging a point that a constraint
/// pins therefore moves nothing rather than reporting a conflict. The default
/// weight is [`DEFAULT_PRIOR_WEIGHT`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftPrior {
    /// The geometry point being dragged.
    pub reference: GeoRef,
    /// The target position, in sketch coordinates.
    pub target: Vector2,
    /// The penalty weight.
    pub weight: f64,
}

impl SoftPrior {
    /// A prior with [`DEFAULT_PRIOR_WEIGHT`].
    pub fn new(reference: GeoRef, target: Vector2) -> Self {
        Self {
            reference,
            target,
            weight: DEFAULT_PRIOR_WEIGHT,
        }
    }

    /// The same prior with a different weight.
    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight;
        self
    }
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A reference the constraint cannot use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolveError {
    /// A constraint names a geometry the solver does not parameterize, or one
    /// that does not exist.
    UnsupportedGeometry { geometry: usize },
    /// A constraint is missing a reference its kind needs.
    MissingReference { constraint: usize },
    /// The references exist but the kind does not apply to them (a radius on a
    /// line, a parallel between a circle and a line, a zero-length line).
    InapplicableConstraint { constraint: usize, reason: &'static str },
    /// A soft prior names something that cannot be dragged (the root axes).
    InapplicablePrior { prior: usize },
}

impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolveError::UnsupportedGeometry { geometry } => write!(
                f,
                "geometry {geometry} is not a point, line, circle or arc the solver can move"
            ),
            SolveError::MissingReference { constraint } => {
                write!(f, "constraint {constraint} is missing a reference")
            }
            SolveError::InapplicableConstraint { constraint, reason } => {
                write!(f, "constraint {constraint} cannot apply: {reason}")
            }
            SolveError::InapplicablePrior { prior } => write!(
                f,
                "soft prior {prior} names a root axis; only sketch geometry can be dragged"
            ),
        }
    }
}

impl std::error::Error for SolveError {}

/// The outcome of one solve.
#[derive(Clone, Debug)]
pub struct Solution {
    /// The geometry at the solution (the best fit when `converged` is false).
    /// The order and the length match the input.
    pub curves: Vec<Curve2d>,
    /// Whether every residual is within [`RESIDUAL_TOLERANCE`].
    pub converged: bool,
    /// The remaining degrees of freedom: parameters minus the Jacobian's rank.
    pub dof: usize,
    /// The constraints that add no rank beyond their predecessors, as 0-based
    /// indices into the constraint list the solve was given.
    pub redundant: Vec<usize>,
    /// The constraints still violated at the best fit, as 0-based indices.
    pub conflicting: Vec<usize>,
    /// The largest residual at the best fit.
    pub max_residual: f64,
}

// ---------------------------------------------------------------------------
// The solve
// ---------------------------------------------------------------------------

/// Check that every constraint applies, without solving anything.
///
/// This is the validation `addConstraint` uses, so a constraint that the solver
/// could never satisfy cannot enter the model: the check runs the same residual
/// code the solve does, so the two cannot drift.
pub fn check(curves: &[Curve2d], constraints: &[Constraint]) -> Result<(), SolveError> {
    let layout = eval::Layout::new(curves);
    let initial = layout.params(curves);
    eval::evaluate(&layout, curves, &initial, &initial, constraints).map(|_| ())
}

/// Solve `constraints` over `curves`.
///
/// The input is never mutated. On a structural problem (an unsupported
/// reference, an inapplicable constraint) this returns an error; an
/// over-constrained or unreachable system is *not* an error — it returns a
/// [`Solution`] with `converged == false` and the conflicting constraints named,
/// so a caller can report the conflict instead of guessing.
///
/// The sketch is split into **connected islands** first. A constraint only ever
/// couples the geometries it names, and a root-axis reference couples nothing
/// (the axes cannot move), so two unrelated sub-assemblies solve independently:
/// each reports its own conflicts, and a broken island does not drag the
/// diagnosis of a healthy one. An island with no constraint at all is answered
/// from its parameter count alone, and an island whose residuals are already
/// satisfied skips the iteration and only pays for the rank analysis.
pub fn solve(curves: &[Curve2d], constraints: &[Constraint]) -> Result<Solution, SolveError> {
    solve_with_priors(curves, constraints, &[])
}

/// Solve `constraints` over `curves` while pulling free degrees of freedom
/// toward optional [`SoftPrior`] dragging targets.
///
/// Priors are penalties: they steer the under-constrained part of the solve
/// without entering the convergence test or the rank analysis. `solve` is this
/// function with no priors.
pub fn solve_with_priors(
    curves: &[Curve2d],
    constraints: &[Constraint],
    priors: &[SoftPrior],
) -> Result<Solution, SolveError> {
    // Validate the whole system first, so a structural error reports the global
    // geometry and constraint indices the caller gave us rather than a local one.
    check(curves, constraints)?;
    if !priors.is_empty() {
        let layout = eval::Layout::new(curves);
        let initial = layout.params(curves);
        eval::evaluate_priors(&layout, curves, &initial, &initial, priors)?;
    }

    let count = curves.len();
    if count == 0 && constraints.is_empty() {
        return Ok(Solution {
            curves: Vec::new(),
            converged: true,
            dof: 0,
            redundant: Vec::new(),
            conflicting: Vec::new(),
            max_residual: 0.0,
        });
    }

    // Union the geometry the constraints couple. Root axes are deliberately not
    // unioned: two sub-assemblies grounded on the origin are still independent.
    let mut groups = DisjointSet::new(count);
    for constraint in constraints {
        let mut named = named_geometries(constraint);
        if let Some(first) = named.next() {
            for other in named {
                groups.union(first, other);
            }
        }
    }

    let mut island_curves: Vec<Vec<usize>> = vec![Vec::new(); count];
    for index in 0..count {
        let root = groups.find(index);
        island_curves[root].push(index);
    }
    let mut island_constraints: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut root_only: Vec<usize> = Vec::new();
    for (index, constraint) in constraints.iter().enumerate() {
        match named_geometries(constraint).next() {
            Some(geometry) => {
                let root = groups.find(geometry);
                island_constraints[root].push(index);
            }
            None => root_only.push(index),
        }
    }
    let mut island_priors: Vec<Vec<SoftPrior>> = vec![Vec::new(); count];
    for prior in priors {
        if let Reference::Geometry(geometry) = prior.reference.reference {
            let root = groups.find(geometry);
            island_priors[root].push(*prior);
        }
    }

    let mut local_index = vec![0usize; count];
    let mut solved = curves.to_vec();
    let mut dof = 0usize;
    let mut converged = true;
    let mut max_residual = 0.0_f64;
    let mut redundant = Vec::new();
    let mut conflicting = Vec::new();

    for root in 0..count {
        let geometries = &island_curves[root];
        if geometries.is_empty() {
            continue;
        }
        let constraint_indices = &island_constraints[root];
        let priors = &island_priors[root];

        // Tier 1: nothing constrains these parameters, so they contribute their
        // full parameter count and cost no evaluation at all.
        if constraint_indices.is_empty() && priors.is_empty() {
            for &geometry in geometries {
                dof += curves[geometry].parameters();
            }
            continue;
        }

        let mut local_curves = Vec::with_capacity(geometries.len());
        for (local, &global) in geometries.iter().enumerate() {
            local_index[global] = local;
            local_curves.push(curves[global]);
        }
        let local_constraints: Vec<Constraint> = constraint_indices
            .iter()
            .map(|&index| remap_constraint(&constraints[index], &local_index))
            .collect();
        let local_priors: Vec<SoftPrior> = priors
            .iter()
            .map(|prior| SoftPrior {
                reference: remap_reference(prior.reference, &local_index),
                target: prior.target,
                weight: prior.weight,
            })
            .collect();

        let outcome = run_island(&local_curves, &local_constraints, &local_priors)?;
        for (local, &global) in geometries.iter().enumerate() {
            solved[global] = outcome.solution.curves[local];
        }
        dof += outcome.solution.dof;
        max_residual = max_residual.max(outcome.solution.max_residual);
        for &local in &outcome.solution.redundant {
            redundant.push(constraint_indices[local]);
        }
        if !outcome.solution.converged {
            converged = false;
            for local in diagnose(
                &local_curves,
                &local_constraints,
                &outcome.solution,
                &outcome.dependent_cycle,
            ) {
                conflicting.push(constraint_indices[local]);
            }
        }
    }

    // A constraint that names only root axes has no island of its own.
    if !root_only.is_empty() {
        let local_constraints: Vec<Constraint> = root_only
            .iter()
            .map(|&index| constraints[index].clone())
            .collect();
        let outcome = run_island(&[], &local_constraints, &[])?;
        max_residual = max_residual.max(outcome.solution.max_residual);
        for &local in &outcome.solution.redundant {
            redundant.push(root_only[local]);
        }
        if !outcome.solution.converged {
            converged = false;
            for local in diagnose(
                &[],
                &local_constraints,
                &outcome.solution,
                &outcome.dependent_cycle,
            ) {
                conflicting.push(root_only[local]);
            }
        }
    }

    redundant.sort_unstable();
    conflicting.sort_unstable();

    Ok(Solution {
        curves: solved,
        converged,
        dof,
        redundant,
        conflicting,
        max_residual,
    })
}

/// Which constraints are responsible for an island that does not converge.
///
/// The useful answer is not "every residual is nonzero": a single bad datum drags
/// the rest of the island and leaves many rows slightly violated. Two filters
/// narrow the candidates first — a constraint has to be part of a rank-dependent
/// cycle *or* still be violated at the best fit to be able to rescue convergence
/// by leaving. The diagnosis is then leave-one-out on that candidate set: a
/// constraint is named when removing it lets the rest solve, which is exactly the
/// constraint a user has to change. If no single removal helps, the report falls
/// back to the constraints still violated at the best fit.
fn diagnose(
    curves: &[Curve2d],
    constraints: &[Constraint],
    solution: &Solution,
    dependent_cycle: &[bool],
) -> Vec<usize> {
    if constraints.len() > 1 {
        let mut responsible = Vec::new();
        for index in 0..constraints.len() {
            let in_cycle = dependent_cycle.get(index).copied().unwrap_or(true);
            let violated = solution.conflicting.contains(&index);
            if !in_cycle && !violated {
                continue;
            }
            let reduced: Vec<Constraint> = constraints
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, constraint)| constraint.clone())
                .collect();
            if let Ok(without) = run_island(curves, &reduced, &[]) {
                if without.solution.converged {
                    responsible.push(index);
                }
            }
        }
        if !responsible.is_empty() {
            return responsible;
        }
    }
    let mut violated = solution.conflicting.clone();
    violated.sort_unstable();
    violated
}

/// One island's solution plus the rank metadata its diagnosis needs.
struct IslandOutcome {
    solution: Solution,
    dependent_cycle: Vec<bool>,
}

/// Solve one connected component.
fn run_island(
    curves: &[Curve2d],
    constraints: &[Constraint],
    priors: &[SoftPrior],
) -> Result<IslandOutcome, SolveError> {
    let layout = eval::Layout::new(curves);
    let initial = layout.params(curves);
    let size = layout.len();

    let initial_groups = eval::evaluate(&layout, curves, &initial, &initial, constraints)?;
    let initial_rows = flat(&initial_groups);

    // Tier 2: an island whose hard residuals are already satisfied and that has no
    // prior to apply needs no iteration — but it still gets the rank analysis, so
    // its degrees of freedom and its redundant constraints are reported.
    let best = if linalg::max_abs(&initial_rows) <= RESIDUAL_TOLERANCE && priors.is_empty() {
        Iteration {
            x: initial,
            groups: initial_groups,
            converged: true,
        }
    } else {
        iterate(&layout, curves, &initial, initial.clone(), constraints, priors)?
    };

    let rows_per_constraint: Vec<Vec<Vec<f64>>> = best
        .groups
        .iter()
        .map(|group| group.rows.iter().map(|row| row.gradient.clone()).collect())
        .collect();
    let analysis = linalg::rank_analysis(&rows_per_constraint);
    let conflicting = best
        .groups
        .iter()
        .filter(|group| {
            group
                .rows
                .iter()
                .fold(0.0_f64, |worst, row| worst.max(row.value.abs()))
                > RESIDUAL_TOLERANCE
        })
        .map(|group| group.index)
        .collect();

    Ok(IslandOutcome {
        solution: Solution {
            curves: layout.curves(&best.x, curves),
            converged: best.converged,
            dof: size - analysis.rank,
            redundant: analysis.redundant,
            conflicting,
            max_residual: linalg::max_abs(&flat(&best.groups)),
        },
        dependent_cycle: analysis.dependent_cycle,
    })
}

/// One Levenberg-Marquardt run from `start`.
struct Iteration {
    x: Vec<f64>,
    groups: Vec<eval::ConstraintRows>,
    converged: bool,
}

fn iterate(
    layout: &eval::Layout,
    curves: &[Curve2d],
    initial: &[f64],
    start: Vec<f64>,
    constraints: &[Constraint],
    priors: &[SoftPrior],
) -> Result<Iteration, SolveError> {
    let combined = descend(layout, curves, initial, start, constraints, priors)?;
    if priors.is_empty() {
        return Ok(combined);
    }
    // The prior rows are penalties, so their optimum trades a little hard
    // feasibility for the pull (the trade is O(weight²)). One pass over the hard
    // rows only removes that trade-off while leaving the free degrees of freedom
    // — the ones the prior was dragging — where the prior put them.
    descend(layout, curves, initial, combined.x, constraints, &[])
}

fn descend(
    layout: &eval::Layout,
    curves: &[Curve2d],
    initial: &[f64],
    start: Vec<f64>,
    constraints: &[Constraint],
    priors: &[SoftPrior],
) -> Result<Iteration, SolveError> {
    let size = layout.len();
    let mut x = start;
    let mut groups = eval::evaluate(layout, curves, initial, &x, constraints)?;
    let mut hard_rows = flat(&groups);
    let mut rows = hard_rows.clone();
    if !priors.is_empty() {
        rows.extend(eval::evaluate_priors(layout, curves, initial, &x, priors)?);
    }
    let mut score = linalg::sum_of_squares(&rows);
    // With a prior in play the cost can keep improving after the hard rows are
    // satisfied, so only a prior-free run stops on the residual alone.
    let mut converged = priors.is_empty() && linalg::max_abs(&hard_rows) <= RESIDUAL_TOLERANCE;
    let mut lambda = 1e-3;
    let mut stagnant = 0usize;
    let mut iterations = 0usize;

    while !converged && stagnant < 6 && iterations < MAX_ITERATIONS {
        iterations += 1;
        let (normal, gradient) = linalg::normal_equations(&rows, size);
        let mut accepted = false;
        for _ in 0..12 {
            let mut damped = normal.clone();
            for (index, row) in damped.iter_mut().enumerate() {
                row[index] += lambda * (1.0 + normal[index][index].abs());
            }
            if let Some(step) = linalg::cholesky_solve(damped, &gradient) {
                let candidate: Vec<f64> = x
                    .iter()
                    .zip(&step)
                    .map(|(value, delta)| value - delta)
                    .collect();
                if candidate.iter().all(|value| value.is_finite()) {
                    let next = eval::evaluate(layout, curves, initial, &candidate, constraints)?;
                    let next_hard = flat(&next);
                    let mut next_rows = next_hard.clone();
                    if !priors.is_empty() {
                        next_rows.extend(eval::evaluate_priors(
                            layout,
                            curves,
                            initial,
                            &candidate,
                            priors,
                        )?);
                    }
                    let next_score = linalg::sum_of_squares(&next_rows);
                    if next_score < score {
                        let moved = step.iter().fold(0.0_f64, |worst, d| worst.max(d.abs()));
                        x = candidate;
                        hard_rows = next_hard;
                        rows = next_rows;
                        score = next_score;
                        groups = next;
                        lambda = (lambda / 3.0).max(1e-12);
                        if priors.is_empty() {
                            converged = linalg::max_abs(&hard_rows) <= RESIDUAL_TOLERANCE;
                        }
                        accepted = true;
                        if moved < 1e-14 {
                            stagnant += 1;
                        }
                        break;
                    }
                }
            }
            lambda *= 10.0;
            if lambda > 1e12 {
                break;
            }
        }
        if !accepted {
            break;
        }
    }

    Ok(Iteration {
        x,
        groups,
        converged: linalg::max_abs(&hard_rows) <= RESIDUAL_TOLERANCE,
    })
}

/// A union-find over geometry indices.
struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(count: usize) -> Self {
        Self {
            parent: (0..count).collect(),
        }
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.parent[index] != index {
            let next = self.parent[index];
            self.parent[index] = self.parent[next];
            index = next;
        }
        index
    }

    fn union(&mut self, a: usize, b: usize) {
        let left = self.find(a);
        let right = self.find(b);
        if left != right {
            if left < right {
                self.parent[right] = left;
            } else {
                self.parent[left] = right;
            }
        }
    }
}

/// The geometry a constraint names, in reference order. Root axes are skipped:
/// they are fixed points, not nodes of the coupling graph.
fn named_geometries(constraint: &Constraint) -> impl Iterator<Item = usize> + '_ {
    [Some(constraint.first), constraint.second, constraint.third]
        .into_iter()
        .flatten()
        .filter_map(|reference| match reference.reference {
            Reference::Geometry(index) => Some(index),
            Reference::RootXAxis | Reference::RootYAxis => None,
        })
}

fn remap_reference(reference: GeoRef, local_index: &[usize]) -> GeoRef {
    match reference.reference {
        Reference::Geometry(index) => GeoRef {
            reference: Reference::Geometry(local_index[index]),
            pos: reference.pos,
        },
        Reference::RootXAxis | Reference::RootYAxis => reference,
    }
}

fn remap_constraint(constraint: &Constraint, local_index: &[usize]) -> Constraint {
    Constraint {
        kind: constraint.kind.clone(),
        first: remap_reference(constraint.first, local_index),
        second: constraint
            .second
            .map(|other| remap_reference(other, local_index)),
        third: constraint.third.map(|other| remap_reference(other, local_index)),
    }
}
fn flat(groups: &[eval::ConstraintRows]) -> Vec<eval::Row> {
    groups
        .iter()
        .flat_map(|group| group.rows.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f64, y: f64) -> Vector2 {
        Vector2::new(x, y)
    }

    fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Curve2d {
        Curve2d::Line {
            start: point(x1, y1),
            end: point(x2, y2),
        }
    }

    fn circle(cx: f64, cy: f64, r: f64) -> Curve2d {
        Curve2d::Circle {
            center: point(cx, cy),
            radius: r,
        }
    }

    fn close(a: Vector2, b: Vector2) -> bool {
        (a.x - b.x).abs() < 1e-7 && (a.y - b.y).abs() < 1e-7
    }

    #[test]
    fn a_perturbed_rectangle_closes_on_solve() {
        // Four lines that are roughly, not exactly, a rectangle.
        let curves = [
            line(0.0, 0.0, 10.3, 0.2),
            line(10.0, 0.4, 9.8, 6.0),
            line(9.9, 6.3, 0.1, 6.1),
            line(0.2, 5.8, 0.0, 0.3),
        ];
        let mut constraints = vec![
            // Axis alignment.
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
            Constraint::new(Kind::Vertical, GeoRef::geometry(1)),
            Constraint::new(Kind::Horizontal, GeoRef::geometry(2)),
            Constraint::new(Kind::Vertical, GeoRef::geometry(3)),
        ];
        // The corners meet.
        for (index, next) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
            constraints.push(
                Constraint::new(
                    Kind::Coincident,
                    GeoRef::geometry_at(index, Pos::End),
                )
                .with_second(GeoRef::geometry_at(next, Pos::Start)),
            );
        }

        let solution = solve(&curves, &constraints).expect("solves");
        assert!(
            solution.converged,
            "max residual {}",
            solution.max_residual
        );
        // Width, height, x and y remain free.
        assert_eq!(solution.dof, 4);
        assert!(solution.redundant.is_empty(), "{:?}", solution.redundant);

        let solved: Vec<_> = solution
            .curves
            .iter()
            .map(|curve| match curve {
                Curve2d::Line { start, end } => (*start, *end),
                _ => panic!("a line was expected"),
            })
            .collect();
        for (index, next) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
            assert!(
                close(solved[index].1, solved[next].0),
                "corner {index} -> {next} is open"
            );
        }
        // Axis-aligned means each side shares a coordinate.
        assert!((solved[0].0.y - solved[0].1.y).abs() < 1e-7);
        assert!((solved[1].0.x - solved[1].1.x).abs() < 1e-7);
    }

    #[test]
    fn a_distance_datum_moves_the_end_point() {
        let curves = [line(0.0, 0.0, 1.0, 0.0)];
        let constraints = [
            // Anchor the start on the origin and keep the line horizontal.
            Constraint::new(
                Kind::Coincident,
                GeoRef::geometry_at(0, Pos::Start),
            )
            .with_second(GeoRef::origin()),
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
            Constraint::new(
                Kind::Distance(10.0),
                GeoRef::geometry_at(0, Pos::Start),
            )
            .with_second(GeoRef::geometry_at(0, Pos::End)),
        ];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged);
        assert_eq!(solution.dof, 0, "the line is fully constrained");
        match solution.curves[0] {
            Curve2d::Line { start, end } => {
                assert!(close(start, point(0.0, 0.0)));
                assert!(close(end, point(10.0, 0.0)), "{end:?}");
            }
            _ => panic!("a line was expected"),
        }
    }

    #[test]
    fn a_radius_datum_resizes_a_circle() {
        let curves = [circle(1.0, 2.0, 1.0)];
        let constraints = [Constraint::new(Kind::Radius(5.0), GeoRef::geometry(0))];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged);
        assert_eq!(solution.dof, 2, "the centre is still free");
        match solution.curves[0] {
            Curve2d::Circle { center, radius } => {
                assert!(close(center, point(1.0, 2.0)));
                assert!((radius - 5.0).abs() < 1e-9, "{radius}");
            }
            _ => panic!("a circle was expected"),
        }
    }

    #[test]
    fn a_duplicate_constraint_is_reported_redundant() {
        let curves = [line(0.0, 0.0, 4.0, 0.0)];
        let constraints = [
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
        ];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged);
        assert_eq!(solution.redundant, vec![1], "the later duplicate");
        assert_eq!(solution.dof, 3);
        assert!(solution.conflicting.is_empty());
    }

    #[test]
    fn contradictory_constraints_report_no_convergence() {
        let curves = [line(0.0, 0.0, 3.0, 0.0)];
        let constraints = [
            Constraint::new(
                Kind::Coincident,
                GeoRef::geometry_at(0, Pos::Start),
            )
            .with_second(GeoRef::geometry_at(0, Pos::End)),
            Constraint::new(
                Kind::Distance(5.0),
                GeoRef::geometry_at(0, Pos::Start),
            )
            .with_second(GeoRef::geometry_at(0, Pos::End)),
        ];
        let solution = solve(&curves, &constraints).expect("a conflict is not an error");
        assert!(!solution.converged);
        assert!(!solution.conflicting.is_empty());
    }

    #[test]
    fn a_block_pins_the_geometry_it_names() {
        let curves = [line(1.0, 2.0, 3.0, 4.0)];
        let constraints = [Constraint::new(Kind::Block, GeoRef::geometry(0))];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged);
        assert_eq!(solution.dof, 0);
        assert_eq!(solution.curves[0], curves[0]);
    }

    #[test]
    fn an_arc_radius_datum_moves_the_rim_and_its_endpoints() {
        let curves = [Curve2d::Arc {
            center: point(0.0, 0.0),
            radius: 2.0,
            first: 0.0,
            last: std::f64::consts::FRAC_PI_2,
        }];
        let constraints = [Constraint::new(Kind::Radius(4.0), GeoRef::geometry(0))];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged);
        let Curve2d::Arc { radius, .. } = solution.curves[0] else {
            panic!("an arc was expected");
        };
        assert!((radius - 4.0).abs() < 1e-9, "{radius}");
    }

    #[test]
    fn a_radius_on_a_line_is_refused() {
        let curves = [line(0.0, 0.0, 1.0, 0.0)];
        let constraints = [Constraint::new(Kind::Radius(1.0), GeoRef::geometry(0))];
        let error = check(&curves, &constraints).expect_err("a line has no radius");
        assert!(matches!(
            error,
            SolveError::InapplicableConstraint { constraint: 0, .. }
        ));
    }

    #[test]
    fn unparameterized_geometry_is_carried_through() {
        let curves = [Curve2d::Other, line(0.0, 0.0, 1.0, 0.0)];
        let constraints = [Constraint::new(Kind::Horizontal, GeoRef::geometry(1))];
        let solution = solve(&curves, &constraints).expect("solves");
        assert_eq!(solution.curves[0], Curve2d::Other);
        assert_eq!(solution.dof, 3);
    }

    #[test]
    fn a_perpendicular_pair_with_equal_lengths_is_a_square_corner() {
        let curves = [
            line(0.0, 0.0, 4.0, 0.0),
            line(4.0, 0.0, 4.0, 1.0),
        ];
        let constraints = [
            Constraint::new(
                Kind::Coincident,
                GeoRef::geometry_at(0, Pos::End),
            )
            .with_second(GeoRef::geometry_at(1, Pos::Start)),
            Constraint::new(Kind::Perpendicular, GeoRef::geometry(0))
                .with_second(GeoRef::geometry(1)),
            Constraint::new(Kind::Equal, GeoRef::geometry(0))
                .with_second(GeoRef::geometry(1)),
        ];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged, "{}", solution.max_residual);
        let lengths: Vec<f64> = solution
            .curves
            .iter()
            .map(|curve| match curve {
                Curve2d::Line { start, end } => (end.x - start.x).hypot(end.y - start.y),
                _ => 0.0,
            })
            .collect();
        assert!((lengths[0] - lengths[1]).abs() < 1e-7, "{lengths:?}");
    }

    #[test]
    fn independent_islands_isolate_multiple_conflicts() {
        // Two disconnected lines, each given two contradictory length datums.
        // A single leave-one-out pass over the whole sketch would find nothing —
        // removing one constraint still leaves the other island unconverged — so
        // the split is what makes the diagnosis possible.
        //
        // The second island deliberately uses two datums rather than
        // Vertical+Horizontal: a line free to collapse satisfies both by shrinking
        // to zero length, so that pair is *not* a conflict.
        let curves = [line(0.0, 0.0, 5.0, 0.0), line(10.0, 10.0, 15.0, 10.0)];
        let constraints = [
            // Island 0: horizontal, length 5, and a contradictory length 8.
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
            Constraint::new(Kind::Distance(5.0), GeoRef::geometry_at(0, Pos::Start))
                .with_second(GeoRef::geometry_at(0, Pos::End)),
            Constraint::new(Kind::Distance(8.0), GeoRef::geometry_at(0, Pos::Start))
                .with_second(GeoRef::geometry_at(0, Pos::End)),
            // Island 1: lengths 3 and 9.
            Constraint::new(Kind::Distance(3.0), GeoRef::geometry_at(1, Pos::Start))
                .with_second(GeoRef::geometry_at(1, Pos::End)),
            Constraint::new(Kind::Distance(9.0), GeoRef::geometry_at(1, Pos::Start))
                .with_second(GeoRef::geometry_at(1, Pos::End)),
        ];

        let solution = solve(&curves, &constraints).expect("a conflict is not an error");
        assert!(!solution.converged);
        assert_eq!(
            solution.conflicting,
            vec![1, 2, 3, 4],
            "each island's pair is named; the satisfied Horizontal (0) is not"
        );
    }

    #[test]
    fn already_satisfied_island_still_reports_redundancy_and_dof() {
        // The first island's residuals are already zero, the second has no
        // constraint at all: neither needs an iteration, but both still report
        // their degrees of freedom and redundancy.
        let curves = [line(0.0, 0.0, 4.0, 0.0), Curve2d::Point(point(9.0, 9.0))];
        let constraints = [
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
        ];
        let solution = solve(&curves, &constraints).expect("solves");
        assert!(solution.converged);
        assert_eq!(solution.redundant, vec![1]);
        // 3 DOF on the horizontal line + 2 DOF on the unconstrained point.
        assert_eq!(solution.dof, 5);
    }

    #[test]
    fn soft_prior_drags_free_dof_without_violating_hard_constraints() {
        // A line whose start is pinned to the origin and constrained horizontal:
        // one free degree of freedom (`end.x`).
        let curves = [line(0.0, 0.0, 2.0, 0.0)];
        let constraints = [
            Constraint::new(Kind::Coincident, GeoRef::geometry_at(0, Pos::Start))
                .with_second(GeoRef::origin()),
            Constraint::new(Kind::Horizontal, GeoRef::geometry(0)),
        ];
        // Drag the end toward (7.5, 4.0): the free x follows, the hard horizontal
        // row keeps y at 0.
        let priors = [SoftPrior::new(
            GeoRef::geometry_at(0, Pos::End),
            point(7.5, 4.0),
        )];
        let solution = solve_with_priors(&curves, &constraints, &priors).expect("solves");
        assert!(solution.converged, "{}", solution.max_residual);
        assert_eq!(solution.dof, 1);
        assert!(solution.redundant.is_empty());
        assert!(solution.conflicting.is_empty());
        match solution.curves[0] {
            Curve2d::Line { start, end } => {
                assert!(close(start, point(0.0, 0.0)), "{start:?}");
                assert!(close(end, point(7.5, 0.0)), "expected (7.5, 0.0), got {end:?}");
            }
            _ => panic!("a line was expected"),
        }
    }

    #[test]
    fn a_prior_on_the_root_is_refused() {
        let curves = [line(0.0, 0.0, 2.0, 0.0)];
        let priors = [SoftPrior::new(GeoRef::origin(), point(1.0, 1.0))];
        let error = solve_with_priors(&curves, &[], &priors).expect_err("the origin cannot move");
        assert!(matches!(error, SolveError::InapplicablePrior { prior: 0 }));
    }
}
