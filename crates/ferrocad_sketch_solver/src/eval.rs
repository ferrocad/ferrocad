//! Parameter layout, residual rows and their exact first derivatives.
//!
//! Every residual is built from [`Row`] values, which carry a numeric value and
//! its gradient with respect to the parameter vector. The arithmetic rules below
//! (product, quotient, square root) apply the chain rule, so the Jacobian is
//! analytic and exact — not a finite difference. That matters for the rank
//! analysis: two identical constraints must produce bit-identical rows, which a
//! difference quotient would only approximate.

use ferrocad_curves::Vector2;

use crate::{Constraint, Curve2d, GeoRef, Kind, Pos, Reference, SolveError};

// ---------------------------------------------------------------------------
// The parameter layout
// ---------------------------------------------------------------------------

/// Where each geometry's parameters live in the solver's vector.
///
/// `Point` is `(x, y)`, `Line` is `(x1, y1, x2, y2)`, `Circle` is `(cx, cy, r)`
/// and `Arc` is `(cx, cy, r, first, last)` — the parameterization upstream's
/// solver uses. A geometry the solver does not model ([`Curve2d::Other`]) has no
/// block: it keeps its shape and is refused as a constraint reference.
pub(crate) struct Layout {
    offsets: Vec<Option<usize>>,
    total: usize,
}

impl Layout {
    pub(crate) fn new(curves: &[Curve2d]) -> Self {
        let mut offsets = Vec::with_capacity(curves.len());
        let mut total = 0;
        for curve in curves {
            let extent = curve.parameters();
            if extent == 0 {
                offsets.push(None);
            } else {
                offsets.push(Some(total));
                total += extent;
            }
        }
        Self { offsets, total }
    }

    pub(crate) fn len(&self) -> usize {
        self.total
    }

    pub(crate) fn offset(&self, geometry: usize) -> Option<usize> {
        self.offsets.get(geometry).copied().flatten()
    }

    /// The starting parameter vector for `curves`.
    pub(crate) fn params(&self, curves: &[Curve2d]) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.total);
        for curve in curves {
            match curve {
                Curve2d::Point(point) => out.extend([point.x, point.y]),
                Curve2d::Line { start, end } => {
                    out.extend([start.x, start.y, end.x, end.y]);
                }
                Curve2d::Circle { center, radius } => {
                    out.extend([center.x, center.y, *radius]);
                }
                Curve2d::Arc {
                    center,
                    radius,
                    first,
                    last,
                } => out.extend([center.x, center.y, *radius, *first, *last]),
                Curve2d::Other => {}
            }
        }
        out
    }

    /// The curves at parameter vector `x`; a geometry the solver does not
    /// parameterize is returned unchanged.
    pub(crate) fn curves(&self, x: &[f64], base: &[Curve2d]) -> Vec<Curve2d> {
        let mut out = Vec::with_capacity(base.len());
        for (index, curve) in base.iter().enumerate() {
            let Some(offset) = self.offset(index) else {
                out.push(*curve);
                continue;
            };
            out.push(match curve {
                Curve2d::Point(_) => Curve2d::Point(Vector2::new(x[offset], x[offset + 1])),
                Curve2d::Line { .. } => Curve2d::Line {
                    start: Vector2::new(x[offset], x[offset + 1]),
                    end: Vector2::new(x[offset + 2], x[offset + 3]),
                },
                Curve2d::Circle { .. } => Curve2d::Circle {
                    center: Vector2::new(x[offset], x[offset + 1]),
                    radius: x[offset + 2],
                },
                Curve2d::Arc { .. } => Curve2d::Arc {
                    center: Vector2::new(x[offset], x[offset + 1]),
                    radius: x[offset + 2],
                    first: x[offset + 3],
                    last: x[offset + 4],
                },
                Curve2d::Other => *curve,
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Rows and 2-D jets
// ---------------------------------------------------------------------------

/// One residual with its gradient.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) value: f64,
    pub(crate) gradient: Vec<f64>,
}

impl Row {
    fn constant(value: f64, size: usize) -> Self {
        Self {
            value,
            gradient: vec![0.0; size],
        }
    }

    fn variable(value: f64, at: usize, size: usize) -> Self {
        let mut gradient = vec![0.0; size];
        gradient[at] = 1.0;
        Self { value, gradient }
    }

    fn add(&self, other: &Row) -> Row {
        Row {
            value: self.value + other.value,
            gradient: self
                .gradient
                .iter()
                .zip(&other.gradient)
                .map(|(a, b)| a + b)
                .collect(),
        }
    }

    fn sub(&self, other: &Row) -> Row {
        Row {
            value: self.value - other.value,
            gradient: self
                .gradient
                .iter()
                .zip(&other.gradient)
                .map(|(a, b)| a - b)
                .collect(),
        }
    }

    fn scale(&self, factor: f64) -> Row {
        Row {
            value: self.value * factor,
            gradient: self.gradient.iter().map(|a| a * factor).collect(),
        }
    }

    fn mul(&self, other: &Row) -> Row {
        Row {
            value: self.value * other.value,
            gradient: self
                .gradient
                .iter()
                .zip(&other.gradient)
                .map(|(a, b)| a * other.value + self.value * b)
                .collect(),
        }
    }

    /// `self / other`. The caller guarantees `other` is away from zero (the line
    /// and radius helpers reject a degenerate operand first).
    fn div(&self, other: &Row) -> Row {
        let inverse = 1.0 / other.value;
        Row {
            value: self.value * inverse,
            gradient: self
                .gradient
                .iter()
                .zip(&other.gradient)
                .map(|(a, b)| (a * other.value - self.value * b) * inverse * inverse)
                .collect(),
        }
    }
}

/// A vector of [`Row`]s: a point (or direction) whose coordinates carry their
/// derivatives.
#[derive(Clone, Debug)]
pub(crate) struct Jet2 {
    pub(crate) x: Row,
    pub(crate) y: Row,
}

impl Jet2 {
    fn constant(x: f64, y: f64, size: usize) -> Self {
        Self {
            x: Row::constant(x, size),
            y: Row::constant(y, size),
        }
    }

    fn add(&self, other: &Jet2) -> Jet2 {
        Jet2 {
            x: self.x.add(&other.x),
            y: self.y.add(&other.y),
        }
    }

    fn sub(&self, other: &Jet2) -> Jet2 {
        Jet2 {
            x: self.x.sub(&other.x),
            y: self.y.sub(&other.y),
        }
    }

    /// Multiply by a [`Row`] (a factor with a derivative), used by `r·cos θ`.
    fn scale_by(&self, factor: &Row) -> Jet2 {
        Jet2 {
            x: self.x.mul(factor),
            y: self.y.mul(factor),
        }
    }

    fn dot(&self, other: &Jet2) -> Row {
        self.x.mul(&other.x).add(&self.y.mul(&other.y))
    }

    fn squared_norm(&self) -> Row {
        self.dot(self)
    }

    fn norm(&self) -> Row {
        let squared = self.squared_norm();
        let value = squared.value.max(0.0).sqrt();
        if value <= f64::MIN_POSITIVE {
            return Row::constant(0.0, squared.gradient.len());
        }
        // d(sqrt(u)) = u' / (2 sqrt(u))
        Row {
            value,
            gradient: squared.gradient.iter().map(|d| d / (2.0 * value)).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// The rows one constraint contributes, with the constraint's index so the rank
/// analysis can report which constraint is redundant.
pub(crate) struct ConstraintRows {
    pub(crate) index: usize,
    pub(crate) rows: Vec<Row>,
}

/// The parameterized geometry of a reference, for the branches a residual takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Point,
    Line,
    Circle,
    RootX,
    RootY,
}

struct Evaluator<'a> {
    layout: &'a Layout,
    curves: &'a [Curve2d],
    x: &'a [f64],
    initial: &'a [f64],
    size: usize,
}

fn inapplicable(index: usize, reason: &'static str) -> SolveError {
    SolveError::InapplicableConstraint {
        constraint: index,
        reason,
    }
}

impl Evaluator<'_> {
    fn offset(&self, _index: usize, geometry: usize) -> Result<usize, SolveError> {
        self.curves
            .get(geometry)
            .ok_or(SolveError::UnsupportedGeometry { geometry })?;
        self.layout
            .offset(geometry)
            .ok_or(SolveError::UnsupportedGeometry { geometry })
    }

    /// Parameter `slot` of a geometry's block.
    fn param(&self, index: usize, geometry: usize, slot: usize) -> Result<Row, SolveError> {
        let offset = self.offset(index, geometry)?;
        Ok(Row::variable(
            self.x[offset + slot],
            offset + slot,
            self.size,
        ))
    }

    fn family(&self, reference: GeoRef) -> Result<Family, SolveError> {
        Ok(match reference.reference {
            Reference::RootXAxis => Family::RootX,
            Reference::RootYAxis => Family::RootY,
            Reference::Geometry(geometry) => match self.curves.get(geometry) {
                Some(Curve2d::Point(_)) => Family::Point,
                Some(Curve2d::Line { .. }) => Family::Line,
                Some(Curve2d::Circle { .. }) | Some(Curve2d::Arc { .. }) => Family::Circle,
                _ => return Err(SolveError::UnsupportedGeometry { geometry }),
            },
        })
    }

    /// The point a reference names. `Pos::None` means the geometry's start, which
    /// is what a constraint that names a whole curve means when it reads a point.
    fn point(&self, index: usize, reference: GeoRef) -> Result<Jet2, SolveError> {
        let size = self.size;
        if let Reference::Geometry(geometry) = reference.reference {
            let curve = *self.curves.get(geometry).ok_or(SolveError::UnsupportedGeometry {
                geometry,
            })?;
            self.offset(index, geometry)?;
            return Ok(match curve {
                Curve2d::Point(_) => Jet2 {
                    x: self.param(index, geometry, 0)?,
                    y: self.param(index, geometry, 1)?,
                },
                Curve2d::Line { .. } => match reference.pos {
                    Pos::End => Jet2 {
                        x: self.param(index, geometry, 2)?,
                        y: self.param(index, geometry, 3)?,
                    },
                    Pos::Mid => Jet2 {
                        x: self
                            .param(index, geometry, 0)?
                            .add(&self.param(index, geometry, 2)?)
                            .scale(0.5),
                        y: self
                            .param(index, geometry, 1)?
                            .add(&self.param(index, geometry, 3)?)
                            .scale(0.5),
                    },
                    _ => Jet2 {
                        x: self.param(index, geometry, 0)?,
                        y: self.param(index, geometry, 1)?,
                    },
                },
                Curve2d::Circle { .. } => {
                    let center = Jet2 {
                        x: self.param(index, geometry, 0)?,
                        y: self.param(index, geometry, 1)?,
                    };
                    if reference.pos == Pos::Mid {
                        center
                    } else {
                        // The seam point: centre + (r, 0), upstream's start/end.
                        center.add(&Jet2 {
                            x: self.param(index, geometry, 2)?,
                            y: Row::constant(0.0, size),
                        })
                    }
                }
                Curve2d::Arc { .. } => {
                    let center = Jet2 {
                        x: self.param(index, geometry, 0)?,
                        y: self.param(index, geometry, 1)?,
                    };
                    let radius = self.param(index, geometry, 2)?;
                    match reference.pos {
                        Pos::Start | Pos::End => {
                            let angle = self.param(
                                index,
                                geometry,
                                if reference.pos == Pos::Start { 3 } else { 4 },
                            )?;
                            // `r·cos θ`: the angle is a parameter, so the derivative
                            // has to run through the trigonometric factor too.
                            let (sin, cos) = angle.value.sin_cos();
                            let direction = Jet2 {
                                x: Row {
                                    value: cos,
                                    gradient: angle
                                        .gradient
                                        .iter()
                                        .map(|d| -sin * d)
                                        .collect(),
                                },
                                y: Row {
                                    value: sin,
                                    gradient: angle
                                        .gradient
                                        .iter()
                                        .map(|d| cos * d)
                                        .collect(),
                                },
                            };
                            center.add(&direction.scale_by(&radius))
                        }
                        _ => center,
                    }
                }
                Curve2d::Other => return Err(SolveError::UnsupportedGeometry { geometry }),
            });
        }
        Ok(Jet2::constant(0.0, 0.0, size))
    }

    /// A point and a non-zero direction along a reference.
    fn line(&self, index: usize, reference: GeoRef) -> Result<(Jet2, Jet2), SolveError> {
        match self.family(reference)? {
            Family::RootX => Ok((
                Jet2::constant(0.0, 0.0, self.size),
                Jet2::constant(1.0, 0.0, self.size),
            )),
            Family::RootY => Ok((
                Jet2::constant(0.0, 0.0, self.size),
                Jet2::constant(0.0, 1.0, self.size),
            )),
            Family::Line => {
                let Reference::Geometry(geometry) = reference.reference else {
                    unreachable!("a root reference is handled above");
                };
                let start = self.point(index, GeoRef::geometry(geometry))?;
                let end = self.point(index, GeoRef::geometry_at(geometry, Pos::End))?;
                let direction = end.sub(&start);
                if direction.norm().value <= 1e-12 {
                    return Err(inapplicable(index, "a line of zero length"));
                }
                Ok((start, direction))
            }
            _ => Err(inapplicable(index, "a curve that is not a line")),
        }
    }

    /// The centre of a circle or arc.
    fn center(&self, index: usize, reference: GeoRef) -> Result<Jet2, SolveError> {
        let Reference::Geometry(geometry) = reference.reference else {
            return Err(inapplicable(index, "the root axes have no centre"));
        };
        if self.family(reference)? != Family::Circle {
            return Err(inapplicable(index, "a curve without a radius"));
        }
        Ok(Jet2 {
            x: self.param(index, geometry, 0)?,
            y: self.param(index, geometry, 1)?,
        })
    }

    fn radius(&self, index: usize, reference: GeoRef) -> Result<Row, SolveError> {
        let Reference::Geometry(geometry) = reference.reference else {
            return Err(inapplicable(index, "the root axes have no radius"));
        };
        if self.family(reference)? != Family::Circle {
            return Err(inapplicable(index, "a curve without a radius"));
        }
        self.param(index, geometry, 2)
    }

    fn rows_for(&self, index: usize, constraint: &Constraint) -> Result<Vec<Row>, SolveError> {
        let second = || {
            constraint
                .second
                .ok_or(SolveError::MissingReference { constraint: index })
        };
        let third = || {
            constraint
                .third
                .ok_or(SolveError::MissingReference { constraint: index })
        };
        let first = constraint.first;
        let rows = match &constraint.kind {
            Kind::Coincident => {
                let a = self.point(index, first)?;
                let b = self.point(index, second()?)?;
                vec![a.x.sub(&b.x), a.y.sub(&b.y)]
            }
            Kind::PointOnObject => {
                let point = self.point(index, first)?;
                let other = second()?;
                match self.family(other)? {
                    Family::Line | Family::RootX | Family::RootY => {
                        let (origin, direction) = self.line(index, other)?;
                        let delta = point.sub(&origin);
                        // cross(direction, delta) / |direction| is the signed
                        // distance from the line, so the row is a length.
                        vec![direction
                            .x
                            .mul(&delta.y)
                            .sub(&direction.y.mul(&delta.x))
                            .div(&direction.norm())]
                    }
                    Family::Circle => {
                        let center = self.center(index, other)?;
                        let radius = self.radius(index, other)?;
                        vec![point.sub(&center).squared_norm().sub(&radius.mul(&radius))]
                    }
                    Family::Point => return Err(inapplicable(index, "a point is not a curve")),
                }
            }
            Kind::Horizontal | Kind::Vertical => {
                let (a, b) = match constraint.second {
                    Some(other) => (self.point(index, first)?, self.point(index, other)?),
                    None => {
                        let (start, direction) = self.line(index, first)?;
                        let end = start.add(&direction);
                        (start, end)
                    }
                };
                if matches!(constraint.kind, Kind::Horizontal) {
                    vec![a.y.sub(&b.y)]
                } else {
                    vec![a.x.sub(&b.x)]
                }
            }
            Kind::Parallel | Kind::Perpendicular => {
                let (_, d1) = self.line(index, first)?;
                let (_, d2) = self.line(index, second()?)?;
                let scale = d1.norm().mul(&d2.norm());
                if matches!(constraint.kind, Kind::Parallel) {
                    vec![d1.x.mul(&d2.y).sub(&d1.y.mul(&d2.x)).div(&scale)]
                } else {
                    vec![d1.dot(&d2).div(&scale)]
                }
            }
            Kind::Tangent => {
                let a = first;
                let b = second()?;
                let a_family = self.family(a)?;
                let b_family = self.family(b)?;
                match (a_family, b_family) {
                    (Family::Line | Family::RootX | Family::RootY, Family::Circle) => {
                        let (origin, direction) = self.line(index, a)?;
                        let center = self.center(index, b)?;
                        let radius = self.radius(index, b)?;
                        let delta = center.sub(&origin);
                        // |cross(u, c - p)| = r for an external tangency; the signed
                        // distance is the square root of a squared form, so use the
                        // squared distance to keep the row smooth.
                        let distance = direction
                            .x
                            .mul(&delta.y)
                            .sub(&direction.y.mul(&delta.x))
                            .div(&direction.norm());
                        vec![distance.mul(&distance).sub(&radius.mul(&radius))]
                    }
                    (Family::Circle, Family::Line | Family::RootX | Family::RootY) => {
                        let swapped = Constraint {
                            kind: Kind::Tangent,
                            first: b,
                            second: Some(a),
                            third: None,
                        };
                        return self.rows_for(index, &swapped);
                    }
                    (Family::Circle, Family::Circle) => {
                        let center_a = self.center(index, a)?;
                        let center_b = self.center(index, b)?;
                        let radius_a = self.radius(index, a)?;
                        let radius_b = self.radius(index, b)?;
                        let sum = radius_a.add(&radius_b);
                        vec![center_a
                            .sub(&center_b)
                            .squared_norm()
                            .sub(&sum.mul(&sum))]
                    }
                    _ => return Err(inapplicable(index, "tangency needs a line and a circle")),
                }
            }
            Kind::Equal => match (self.family(first)?, self.family(second()?)?) {
                (Family::Line, Family::Line) => {
                    let (_, d1) = self.line(index, first)?;
                    let (_, d2) = self.line(index, second()?)?;
                    vec![d1.norm().sub(&d2.norm())]
                }
                (Family::Circle, Family::Circle) => {
                    let r1 = self.radius(index, first)?;
                    let r2 = self.radius(index, second()?)?;
                    vec![r1.sub(&r2)]
                }
                _ => return Err(inapplicable(index, "equality needs two lines or two circles")),
            },
            Kind::Symmetric => {
                let a = self.point(index, first)?;
                let b = self.point(index, second()?)?;
                let about = self.point(index, third()?)?;
                // The midpoint of `a` and `b` is `about`.
                vec![
                    a.x.add(&b.x).sub(&about.x.scale(2.0)),
                    a.y.add(&b.y).sub(&about.y.scale(2.0)),
                ]
            }
            Kind::Block => {
                let Reference::Geometry(geometry) = first.reference else {
                    return Err(inapplicable(index, "the root axes are already fixed"));
                };
                let offset = self.offset(index, geometry)?;
                let count = self.curves[geometry].parameters();
                (0..count)
                    .map(|slot| {
                        Row::variable(self.x[offset + slot], offset + slot, self.size)
                            .sub(&Row::constant(self.initial[offset + slot], self.size))
                    })
                    .collect()
            }
            Kind::Distance(value) => {
                let a = self.point(index, first)?;
                let b = self.point(index, second()?)?;
                // Squared, so the row is smooth when the points meet.
                vec![a
                    .sub(&b)
                    .squared_norm()
                    .sub(&Row::constant(value * value, self.size))]
            }
            Kind::DistanceX(value) => {
                let a = self.point(index, first)?;
                let b = self.point(index, second()?)?;
                vec![a.x.sub(&b.x).sub(&Row::constant(*value, self.size))]
            }
            Kind::DistanceY(value) => {
                let a = self.point(index, first)?;
                let b = self.point(index, second()?)?;
                vec![a.y.sub(&b.y).sub(&Row::constant(*value, self.size))]
            }
            Kind::Radius(value) => {
                let radius = self.radius(index, first)?;
                vec![radius.sub(&Row::constant(*value, self.size))]
            }
            Kind::Diameter(value) => {
                let radius = self.radius(index, first)?;
                vec![radius.scale(2.0).sub(&Row::constant(*value, self.size))]
            }
            Kind::Angle(value) => {
                let (_, d1) = self.line(index, first)?;
                let (_, d2) = self.line(index, second()?)?;
                let scale = d1.norm().mul(&d2.norm());
                vec![d1
                    .dot(&d2)
                    .div(&scale)
                    .sub(&Row::constant(value.cos(), self.size))]
            }
        };
        Ok(rows)
    }
}

/// Evaluate every constraint at `x`.
///
/// `initial` is the parameter vector the solve started from; `Block` pins against
/// it, so pinning a geometry twice in one solve is idempotent.
pub(crate) fn evaluate(
    layout: &Layout,
    curves: &[Curve2d],
    initial: &[f64],
    x: &[f64],
    constraints: &[Constraint],
) -> Result<Vec<ConstraintRows>, SolveError> {
    let evaluator = Evaluator {
        layout,
        curves,
        x,
        initial,
        size: layout.len(),
    };
    let mut out = Vec::with_capacity(constraints.len());
    for (index, constraint) in constraints.iter().enumerate() {
        out.push(ConstraintRows {
            index,
            rows: evaluator.rows_for(index, constraint)?,
        });
    }
    Ok(out)
}

/// Evaluate the soft dragging priors at `x`.
///
/// Each prior adds two weighted rows, `w·(p_x − target_x)` and `w·(p_y − target_y)`,
/// which pull a free degree of freedom toward `target` inside the normal
/// equations without counting as a hard constraint: they never enter the
/// convergence test or the rank analysis. A prior on a root axis is refused — the
/// origin cannot be dragged, and silently ignoring it would look like a working
/// drag that does nothing.
pub(crate) fn evaluate_priors(
    layout: &Layout,
    curves: &[Curve2d],
    initial: &[f64],
    x: &[f64],
    priors: &[crate::SoftPrior],
) -> Result<Vec<Row>, SolveError> {
    if priors.is_empty() {
        return Ok(Vec::new());
    }
    let evaluator = Evaluator {
        layout,
        curves,
        x,
        initial,
        size: layout.len(),
    };
    let mut rows = Vec::with_capacity(priors.len() * 2);
    for (index, prior) in priors.iter().enumerate() {
        if !matches!(prior.reference.reference, Reference::Geometry(_)) {
            return Err(SolveError::InapplicablePrior { prior: index });
        }
        let point = evaluator.point(index, prior.reference)?;
        rows.push(
            point
                .x
                .sub(&Row::constant(prior.target.x, evaluator.size))
                .scale(prior.weight),
        );
        rows.push(
            point
                .y
                .sub(&Row::constant(prior.target.y, evaluator.size))
                .scale(prior.weight),
        );
    }
    Ok(rows)
}
