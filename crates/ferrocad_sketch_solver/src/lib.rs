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
//! no rank beyond its predecessors; it is conflicting when its residual is still
//! above the tolerance at the best fit.

mod eval;
mod linalg;

use std::fmt;

use ferrocad_curves::Vector2;

/// The residual below which the system counts as solved.
pub const RESIDUAL_TOLERANCE: f64 = 1e-9;

/// The iteration cap. A sketch that has not converged by then is reported as
/// unconverged rather than run forever.
pub const MAX_ITERATIONS: usize = 200;

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
pub fn solve(curves: &[Curve2d], constraints: &[Constraint]) -> Result<Solution, SolveError> {
    let mut solution = run(curves, constraints)?;
    if !solution.converged {
        solution.conflicting = diagnose(curves, constraints, &solution);
    }
    Ok(solution)
}

/// Which constraints are responsible for a system that does not converge.
///
/// The useful answer is not "every residual is nonzero": a single bad datum
/// drags the whole sketch and leaves every row slightly violated. So the
/// diagnosis is a leave-one-out test — a constraint is named when removing it
/// lets the rest of the sketch solve, which is exactly the constraint a user has
/// to change. If no single removal helps (two independent conflicts, or a
/// non-convergent but consistent system), the report falls back to the constraints
/// still violated at the best fit, and the contract says so.
fn diagnose(
    curves: &[Curve2d],
    constraints: &[Constraint],
    solution: &Solution,
) -> Vec<usize> {
    if constraints.len() > 1 {
        let mut responsible = Vec::new();
        for index in 0..constraints.len() {
            let reduced: Vec<Constraint> = constraints
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, constraint)| constraint.clone())
                .collect();
            if let Ok(without) = run(curves, &reduced) {
                if without.converged {
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

/// The iteration and the rank analysis, without the conflict diagnosis.
fn run(curves: &[Curve2d], constraints: &[Constraint]) -> Result<Solution, SolveError> {
    let layout = eval::Layout::new(curves);
    let initial = layout.params(curves);
    let best = iterate(&layout, curves, &initial, initial.clone(), constraints)?;
    let size = layout.len();

    let rows_per_constraint: Vec<Vec<Vec<f64>>> = best
        .groups
        .iter()
        .map(|group| group.rows.iter().map(|row| row.gradient.clone()).collect())
        .collect();
    let (rank, redundant) = linalg::independent_rank(&rows_per_constraint);
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

    Ok(Solution {
        curves: layout.curves(&best.x, curves),
        converged: best.converged,
        dof: size - rank,
        redundant,
        conflicting,
        max_residual: linalg::max_abs(&flat(&best.groups)),
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
) -> Result<Iteration, SolveError> {
    let size = layout.len();
    let mut x = start;
    let mut groups = eval::evaluate(layout, curves, initial, &x, constraints)?;
    let mut cost = flat(&groups);
    let mut score = linalg::sum_of_squares(&cost);
    let mut converged = linalg::max_abs(&cost) <= RESIDUAL_TOLERANCE;
    let mut lambda = 1e-3;
    let mut stagnant = 0usize;
    let mut iterations = 0usize;

    while !converged && stagnant < 6 && iterations < MAX_ITERATIONS {
        iterations += 1;
        let (normal, gradient) = linalg::normal_equations(&cost, size);
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
                    let next_rows = flat(&next);
                    let next_score = linalg::sum_of_squares(&next_rows);
                    if next_score < score {
                        let moved = step.iter().fold(0.0_f64, |worst, d| worst.max(d.abs()));
                        x = candidate;
                        cost = next_rows;
                        score = next_score;
                        groups = next;
                        lambda = (lambda / 3.0).max(1e-12);
                        converged = linalg::max_abs(&cost) <= RESIDUAL_TOLERANCE;
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
        converged: linalg::max_abs(&cost) <= RESIDUAL_TOLERANCE,
    })
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
}
