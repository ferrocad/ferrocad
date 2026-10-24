//! The analytic primitives and the [`EdgeCurve`] enum.
//!
//! These mirror upstream's `Part::GeomLine`, `GeomLineSegment`, `GeomCircle`,
//! `GeomArcOfCircle` and `GeomEllipse` (`src/Mod/Part/App/Geometry.h`). The
//! parameterisation follows the OCC/Part conventions:
//!
//! - a [`LineSegment`] uses `t` in `[0, 1]`;
//! - a [`Circle`], an [`Arc`] and an [`Ellipse`] use the angle in radians in the
//!   curve's own frame (`x_axis` at `0`, `normal × x_axis` at `π/2`);
//! - a [`Line`] is infinite and uses the distance along its direction.
//!
//! `tangent_at` returns a **unit** tangent, as OCC's `Geom_Curve::Tangent` does.
//! Angles are not normalised: an arc with `last < first` runs clockwise, which is
//! what [`Arc::reversed`] produces.

use ferrocad_types::Vector3;

use crate::{ANGULAR_TOLERANCE, CurveError, TOLERANCE};

const TAU: f64 = std::f64::consts::TAU;

// ---------------------------------------------------------------------------
// Vector2
// ---------------------------------------------------------------------------

/// A point or vector in a working plane's `(u, v)` frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vector2 {
    /// The `u` component.
    pub x: f64,
    /// The `v` component.
    pub y: f64,
}

impl Vector2 {
    /// A new `(u, v)` pair.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The origin of the plane.
    pub fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    /// Component-wise addition.
    pub fn add(&self, other: &Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }

    /// Component-wise subtraction.
    pub fn sub(&self, other: &Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }

    /// Scalar multiplication.
    pub fn scale(&self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }

    /// The dot product.
    pub fn dot(&self, other: &Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// The Euclidean length.
    pub fn length(&self) -> f64 {
        self.dot(self).sqrt()
    }

    /// The distance to `other`.
    pub fn distance(&self, other: &Self) -> f64 {
        self.sub(other).length()
    }

    /// Whether both components are within `tolerance`.
    pub fn is_equal(&self, other: &Self, tolerance: f64) -> bool {
        self.distance(other) <= tolerance
    }
}

/// The unit form of `vector`, or `None` when it is (near) zero or not finite.
fn unit(vector: Vector3) -> Option<Vector3> {
    let length = vector.length();
    if !length.is_finite() || length <= TOLERANCE {
        None
    } else {
        Some(vector.scale(1.0 / length))
    }
}

/// A unit vector perpendicular to the (unit) `normal`.
fn perpendicular(normal: Vector3) -> Vector3 {
    let axes = [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    ];
    // Pick the axis least aligned with the normal, then project it into the plane.
    let candidate = axes
        .into_iter()
        .min_by(|a, b| {
            normal
                .dot(a)
                .abs()
                .total_cmp(&normal.dot(b).abs())
        })
        .expect("three axes");
    unit(candidate.sub(&normal.scale(normal.dot(&candidate)))).expect("a perpendicular exists")
}

// ---------------------------------------------------------------------------
// Line and LineSegment
// ---------------------------------------------------------------------------

/// An infinite line: an origin and a unit direction (`Part::GeomLine`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    origin: Vector3,
    direction: Vector3,
}

impl Line {
    /// A line through `origin` along `direction` (normalised here).
    pub fn new(origin: Vector3, direction: Vector3) -> Result<Self, CurveError> {
        Ok(Self {
            origin,
            direction: unit(direction).ok_or(CurveError::Degenerate)?,
        })
    }

    /// The point the parameterisation starts from.
    pub fn origin(&self) -> Vector3 {
        self.origin
    }

    /// The unit direction.
    pub fn direction(&self) -> Vector3 {
        self.direction
    }

    /// The point at distance `u` from the origin.
    pub fn point_at(&self, u: f64) -> Vector3 {
        self.origin.add(&self.direction.scale(u))
    }

    /// The unit tangent (the direction, everywhere).
    pub fn tangent_at(&self, _u: f64) -> Vector3 {
        self.direction
    }

    /// The distance from `point` to the line.
    pub fn distance_to(&self, point: Vector3) -> f64 {
        let offset = point.sub(&self.origin);
        offset.sub(&self.direction.scale(offset.dot(&self.direction))).length()
    }
}

/// A bounded straight segment (`Part::GeomLineSegment`), parameterised `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineSegment {
    start: Vector3,
    end: Vector3,
}

impl LineSegment {
    /// The segment from `start` to `end`; coincident endpoints are rejected.
    pub fn new(start: Vector3, end: Vector3) -> Result<Self, CurveError> {
        if start.distance(&end) <= TOLERANCE {
            return Err(CurveError::Degenerate);
        }
        Ok(Self { start, end })
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.start
    }

    /// The second endpoint.
    pub fn end(&self) -> Vector3 {
        self.end
    }

    /// The point at `t` (`0` is the start, `1` the end).
    pub fn point_at(&self, t: f64) -> Vector3 {
        self.start.add(&self.end.sub(&self.start).scale(t))
    }

    /// The unit tangent (the same everywhere).
    pub fn tangent_at(&self, _t: f64) -> Vector3 {
        unit(self.end.sub(&self.start)).expect("the constructor rejects zero length")
    }

    /// The segment length.
    pub fn length(&self) -> f64 {
        self.start.distance(&self.end)
    }

    /// The same segment with its endpoints swapped.
    pub fn reversed(&self) -> Self {
        Self {
            start: self.end,
            end: self.start,
        }
    }
}

// ---------------------------------------------------------------------------
// Circle, Arc, Ellipse
// ---------------------------------------------------------------------------

/// A full circle in 3D (`Part::GeomCircle`): a centre, a normal and a radius.
///
/// The curve lies in the plane through `center` with normal `normal`; `x_axis`
/// is the direction parameterised by angle `0`, and `normal × x_axis` by `π/2`
/// (counter-clockwise looking down the normal).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    center: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    radius: f64,
}

impl Circle {
    /// A circle with an automatically chosen `x_axis`.
    pub fn new(center: Vector3, normal: Vector3, radius: f64) -> Result<Self, CurveError> {
        if !radius.is_finite() || radius <= TOLERANCE {
            return Err(CurveError::Degenerate);
        }
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        Ok(Self {
            center,
            x_axis: perpendicular(normal),
            normal,
            radius,
        })
    }

    /// A circle whose angle `0` points along `x_axis` (projected into the plane).
    pub fn with_axis(
        center: Vector3,
        normal: Vector3,
        x_axis: Vector3,
        radius: f64,
    ) -> Result<Self, CurveError> {
        if !radius.is_finite() || radius <= TOLERANCE {
            return Err(CurveError::Degenerate);
        }
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        let projected = x_axis.sub(&normal.scale(normal.dot(&x_axis)));
        Ok(Self {
            center,
            normal,
            x_axis: unit(projected).ok_or(CurveError::Degenerate)?,
            radius,
        })
    }

    /// The centre point.
    pub fn center(&self) -> Vector3 {
        self.center
    }

    /// The unit normal of the circle's plane.
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The direction parameterised by angle `0`.
    pub fn x_axis(&self) -> Vector3 {
        self.x_axis
    }

    /// The direction parameterised by angle `π/2` (`normal × x_axis`).
    pub fn y_axis(&self) -> Vector3 {
        self.normal.cross(&self.x_axis)
    }

    /// The radius.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// The point at angle `angle` (radians).
    pub fn point_at(&self, angle: f64) -> Vector3 {
        self.center
            .add(&self.x_axis.scale(self.radius * angle.cos()))
            .add(&self.y_axis().scale(self.radius * angle.sin()))
    }

    /// The unit tangent at `angle` (counter-clockwise looking down the normal).
    pub fn tangent_at(&self, angle: f64) -> Vector3 {
        self.x_axis
            .scale(-angle.sin())
            .add(&self.y_axis().scale(angle.cos()))
    }

    /// The circumference.
    pub fn length(&self) -> f64 {
        TAU * self.radius
    }
}

/// A circular arc (`Part::GeomArcOfCircle`), parameterised by angle.
///
/// `first` and `last` are stored as given: `last > first` runs counter-clockwise,
/// `last < first` clockwise. [`Arc::reversed`] swaps them, so the same geometry
/// is traversed the other way (unlike swapping the endpoints of a segment, this
/// is not the complementary arc).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    circle: Circle,
    first: f64,
    last: f64,
}

impl Arc {
    /// The arc of `circle` from `first` to `last` (radians); a zero sweep is rejected.
    pub fn new(circle: Circle, first: f64, last: f64) -> Result<Self, CurveError> {
        if !first.is_finite() || !last.is_finite() {
            return Err(CurveError::Degenerate);
        }
        if (last - first).abs() <= ANGULAR_TOLERANCE {
            return Err(CurveError::Degenerate);
        }
        Ok(Self {
            circle,
            first,
            last,
        })
    }

    /// The supporting circle.
    pub fn circle(&self) -> Circle {
        self.circle
    }

    /// The first angle.
    pub fn first(&self) -> f64 {
        self.first
    }

    /// The last angle.
    pub fn last(&self) -> f64 {
        self.last
    }

    /// The signed sweep (`last - first`; negative runs clockwise).
    pub fn sweep(&self) -> f64 {
        self.last - self.first
    }

    /// The point at `angle`.
    pub fn point_at(&self, angle: f64) -> Vector3 {
        self.circle.point_at(angle)
    }

    /// The unit tangent at `angle`.
    pub fn tangent_at(&self, angle: f64) -> Vector3 {
        self.circle.tangent_at(angle)
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.point_at(self.first)
    }

    /// The last endpoint.
    pub fn end(&self) -> Vector3 {
        self.point_at(self.last)
    }

    /// The arc length (`radius × |sweep|`).
    pub fn length(&self) -> f64 {
        self.circle.radius() * self.sweep().abs()
    }

    /// Whether the sweep is a full turn.
    pub fn is_closed(&self) -> bool {
        (self.sweep().abs() - TAU).abs() <= ANGULAR_TOLERANCE
    }

    /// The same arc traversed in the opposite direction.
    pub fn reversed(&self) -> Self {
        Self {
            circle: self.circle,
            first: self.last,
            last: self.first,
        }
    }
}

/// A full ellipse (`Part::GeomEllipse`): two radii in its own plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ellipse {
    center: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    major_radius: f64,
    minor_radius: f64,
}

impl Ellipse {
    /// An ellipse with the larger radius on the major axis.
    ///
    /// The radii are swapped when `major_radius < minor_radius`, so "major" always
    /// names the longer axis; `x_axis` is its direction.
    pub fn new(
        center: Vector3,
        normal: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<Self, CurveError> {
        if !major_radius.is_finite()
            || !minor_radius.is_finite()
            || major_radius <= TOLERANCE
            || minor_radius <= TOLERANCE
        {
            return Err(CurveError::Degenerate);
        }
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        let (major_radius, minor_radius) = if major_radius >= minor_radius {
            (major_radius, minor_radius)
        } else {
            (minor_radius, major_radius)
        };
        Ok(Self {
            center,
            x_axis: perpendicular(normal),
            normal,
            major_radius,
            minor_radius,
        })
    }

    /// An ellipse whose major axis points along `x_axis` (projected into the plane).
    ///
    /// When the radii arrive the other way round, they are swapped and the axis
    /// rotates to `normal × x_axis`, so "major" always names the longer axis.
    pub fn with_axis(
        center: Vector3,
        normal: Vector3,
        x_axis: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<Self, CurveError> {
        if !major_radius.is_finite()
            || !minor_radius.is_finite()
            || major_radius <= TOLERANCE
            || minor_radius <= TOLERANCE
        {
            return Err(CurveError::Degenerate);
        }
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        let projected = x_axis.sub(&normal.scale(normal.dot(&x_axis)));
        let projected = unit(projected).ok_or(CurveError::Degenerate)?;
        let (major_radius, minor_radius, x_axis) = if major_radius >= minor_radius {
            (major_radius, minor_radius, projected)
        } else {
            (minor_radius, major_radius, normal.cross(&projected))
        };
        Ok(Self {
            center,
            normal,
            x_axis,
            major_radius,
            minor_radius,
        })
    }

    /// The centre point.
    pub fn center(&self) -> Vector3 {
        self.center
    }

    /// The unit normal of the ellipse's plane.
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The direction of the major axis.
    pub fn x_axis(&self) -> Vector3 {
        self.x_axis
    }

    /// The direction of the minor axis (`normal × x_axis`).
    pub fn y_axis(&self) -> Vector3 {
        self.normal.cross(&self.x_axis)
    }

    /// The major radius.
    pub fn major_radius(&self) -> f64 {
        self.major_radius
    }

    /// The minor radius.
    pub fn minor_radius(&self) -> f64 {
        self.minor_radius
    }

    /// The point at `angle` (the *geometric* parameter, not arc length).
    pub fn point_at(&self, angle: f64) -> Vector3 {
        self.center
            .add(&self.x_axis.scale(self.major_radius * angle.cos()))
            .add(&self.y_axis().scale(self.minor_radius * angle.sin()))
    }

    /// The unit tangent at `angle`.
    pub fn tangent_at(&self, angle: f64) -> Vector3 {
        let tangent = self
            .x_axis
            .scale(-self.major_radius * angle.sin())
            .add(&self.y_axis().scale(self.minor_radius * angle.cos()));
        unit(tangent).expect("the radii are positive")
    }

    /// The circumference, by Ramanujan's second approximation (relative error
    /// below `1e-9` for any axis ratio). There is no closed form.
    pub fn length(&self) -> f64 {
        let (a, b) = (self.major_radius, self.minor_radius);
        let h = ((a - b) / (a + b)).powi(2);
        std::f64::consts::PI * (a + b) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()))
    }
}

// ---------------------------------------------------------------------------
// Hyperbola, Parabola and BezierCurve
// ---------------------------------------------------------------------------

/// Validate a parameter range that must run forwards and be finite.
fn forward_range(first: f64, last: f64) -> Result<(f64, f64), CurveError> {
    if !first.is_finite() || !last.is_finite() || !(last > first) {
        return Err(CurveError::InvalidParameter);
    }
    Ok((first, last))
}

/// A rational B-spline in the form the `Part.BSplineCurve` binding consumes:
/// a polynomial degree, unique knots, their multiplicities, poles and weights.
///
/// It is the exact conversion target for the conics and for a Bezier curve, so
/// the Part binding never has to approximate them by sampling.
#[derive(Debug, Clone, PartialEq)]
pub struct NurbsCurve {
    degree: i32,
    knots: Vec<f64>,
    mults: Vec<i32>,
    poles: Vec<Vector3>,
    weights: Vec<f64>,
}

impl NurbsCurve {
    /// A clamped rational B-spline. The multiplicities must add up to
    /// `poles.len() + degree + 1`, the knots must strictly increase and every
    /// weight must be positive.
    pub fn new(
        degree: i32,
        knots: Vec<f64>,
        mults: Vec<i32>,
        poles: Vec<Vector3>,
        weights: Vec<f64>,
    ) -> Result<Self, CurveError> {
        if degree < 1 || poles.len() < 2 || poles.len() != weights.len() {
            return Err(CurveError::Degenerate);
        }
        if degree > 25 {
            return Err(CurveError::DegreeTooHigh);
        }
        if knots.is_empty() || knots.len() != mults.len() {
            return Err(CurveError::Degenerate);
        }
        if knots.iter().any(|knot| !knot.is_finite())
            || !knots.windows(2).all(|pair| pair[0] < pair[1])
        {
            return Err(CurveError::InvalidParameter);
        }
        if mults.iter().any(|mult| *mult < 1 || *mult > degree + 1)
            || mults.iter().sum::<i32>() != poles.len() as i32 + degree + 1
        {
            return Err(CurveError::Degenerate);
        }
        if weights.iter().any(|weight| !weight.is_finite() || *weight <= 0.0) {
            return Err(CurveError::Degenerate);
        }
        Ok(Self {
            degree,
            knots,
            mults,
            poles,
            weights,
        })
    }

    /// The polynomial degree.
    pub fn degree(&self) -> i32 {
        self.degree
    }

    /// The unique knots, in increasing order.
    pub fn knots(&self) -> &[f64] {
        &self.knots
    }

    /// The multiplicity of each unique knot.
    pub fn mults(&self) -> &[i32] {
        &self.mults
    }

    /// The control poles.
    pub fn poles(&self) -> &[Vector3] {
        &self.poles
    }

    /// One weight per pole.
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }

    /// The first parameter of the clamped domain.
    pub fn first(&self) -> f64 {
        self.knots[0]
    }

    /// The last parameter of the clamped domain.
    pub fn last(&self) -> f64 {
        self.knots[self.knots.len() - 1]
    }

    /// Whether any weight departs from `1` (the curve then needs the rational form).
    pub fn is_rational(&self) -> bool {
        self.weights.iter().any(|weight| (*weight - 1.0).abs() > 1e-12)
    }

    /// The full knot vector (each unique knot repeated by its multiplicity).
    fn full_knots(&self) -> Vec<f64> {
        let mut full = Vec::new();
        for (knot, mult) in self.knots.iter().zip(&self.mults) {
            for _ in 0..*mult {
                full.push(*knot);
            }
        }
        full
    }

    /// The point at parameter `u` by rational de Boor evaluation.
    ///
    /// `None` when the description is not a clamped B-spline or the parameter
    /// is not finite.
    pub fn point_at(&self, u: f64) -> Option<Vector3> {
        if !u.is_finite() {
            return None;
        }
        let degree = self.degree as usize;
        let full = self.full_knots();
        let last_pole = self.poles.len().checked_sub(1)?;
        if full.len() != self.poles.len() + degree + 1 {
            return None;
        }
        let first = full[degree];
        let last = full[last_pole + 1];
        let u = u.max(first).min(last);
        // The index of the knot span that contains `u`.
        let mut span = degree;
        if u >= last {
            span = last_pole;
        } else {
            while span < last_pole && !(u >= full[span] && u < full[span + 1]) {
                span += 1;
            }
        }
        let homogeneous = |index: usize| {
            let weight = self.weights[index];
            [
                self.poles[index].x * weight,
                self.poles[index].y * weight,
                self.poles[index].z * weight,
                weight,
            ]
        };
        let mut d: Vec<[f64; 4]> = (0..=degree)
            .map(|index| homogeneous(span + index - degree))
            .collect();
        for r in 1..=degree {
            for j in (r..=degree).rev() {
                let knot = j + span - degree;
                let denominator = full[knot + degree + 1 - r] - full[knot];
                let alpha = if denominator.abs() > 0.0 {
                    (u - full[knot]) / denominator
                } else {
                    0.0
                };
                for component in 0..4 {
                    d[j][component] =
                        (1.0 - alpha) * d[j - 1][component] + alpha * d[j][component];
                }
            }
        }
        let result = d[degree];
        if result[3].abs() <= f64::EPSILON {
            return None;
        }
        Some(Vector3::new(
            result[0] / result[3],
            result[1] / result[3],
            result[2] / result[3],
        ))
    }
}

/// One branch of a hyperbola (`Part::GeomHyperbola`):
/// `X^2/a^2 - Y^2/b^2 = 1` in its own plane, on the positive side of the x axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hyperbola {
    center: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    major_radius: f64,
    minor_radius: f64,
}

impl Hyperbola {
    /// A hyperbola with an automatically chosen major-axis direction.
    pub fn new(
        center: Vector3,
        normal: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<Self, CurveError> {
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        Self::with_axis(
            center,
            normal,
            perpendicular(normal),
            major_radius,
            minor_radius,
        )
    }

    /// A hyperbola whose major axis points along `x_axis` (projected into the plane).
    pub fn with_axis(
        center: Vector3,
        normal: Vector3,
        x_axis: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<Self, CurveError> {
        if !major_radius.is_finite()
            || !minor_radius.is_finite()
            || major_radius <= TOLERANCE
            || minor_radius <= TOLERANCE
        {
            return Err(CurveError::Degenerate);
        }
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        let projected = x_axis.sub(&normal.scale(normal.dot(&x_axis)));
        Ok(Self {
            center,
            normal,
            x_axis: unit(projected).ok_or(CurveError::Degenerate)?,
            major_radius,
            minor_radius,
        })
    }

    /// The centre point.
    pub fn center(&self) -> Vector3 {
        self.center
    }

    /// The unit normal of the hyperbola's plane.
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The direction of the major axis (parameter `0`).
    pub fn x_axis(&self) -> Vector3 {
        self.x_axis
    }

    /// The direction of the minor axis (`normal × x_axis`).
    pub fn y_axis(&self) -> Vector3 {
        self.normal.cross(&self.x_axis)
    }

    /// The major radius.
    pub fn major_radius(&self) -> f64 {
        self.major_radius
    }

    /// The minor radius.
    pub fn minor_radius(&self) -> f64 {
        self.minor_radius
    }

    /// The focal distance (`sqrt(a^2 + b^2)`).
    pub fn focal(&self) -> f64 {
        (self.major_radius * self.major_radius + self.minor_radius * self.minor_radius).sqrt()
    }

    /// The focus on the positive side of the major axis.
    pub fn focus1(&self) -> Vector3 {
        self.center.add(&self.x_axis.scale(self.focal()))
    }

    /// The focus on the negative side of the major axis.
    pub fn focus2(&self) -> Vector3 {
        self.center.sub(&self.x_axis.scale(self.focal()))
    }

    /// The eccentricity `focal / major_radius` (greater than `1`).
    pub fn eccentricity(&self) -> f64 {
        self.focal() / self.major_radius
    }

    /// The point at the OCC parameter `u`:
    /// `center + a*cosh(u)*x_axis + b*sinh(u)*y_axis`.
    pub fn point_at(&self, u: f64) -> Vector3 {
        self.center
            .add(&self.x_axis.scale(self.major_radius * u.cosh()))
            .add(&self.y_axis().scale(self.minor_radius * u.sinh()))
    }

    /// The unit tangent at `u`.
    pub fn tangent_at(&self, u: f64) -> Vector3 {
        let tangent = self
            .x_axis
            .scale(self.major_radius * u.sinh())
            .add(&self.y_axis().scale(self.minor_radius * u.cosh()));
        unit(tangent).expect("the radii are positive")
    }

    /// The portion of the hyperbola between `first` and `last`.
    pub fn trim(&self, first: f64, last: f64) -> Result<ArcOfHyperbola, CurveError> {
        ArcOfHyperbola::new(*self, first, last)
    }

    /// The exact rational quadratic B-spline over `[first, last]`.
    ///
    /// The middle weight is `cosh((last - first) / 2)` and the middle pole is
    /// the mid-curve point pulled towards the centre, which is the intersection
    /// of the endpoint tangents.
    pub fn nurbs(&self, first: f64, last: f64) -> Result<NurbsCurve, CurveError> {
        let (first, last) = forward_range(first, last)?;
        let half = 0.5 * (last - first);
        // `cosh` overflows long before this, and such a segment is meaningless.
        if half >= 700.0 {
            return Err(CurveError::InvalidParameter);
        }
        let middle = self.point_at(0.5 * (first + last));
        let weight = half.cosh();
        let shoulder = self
            .center
            .add(&middle.sub(&self.center).scale(1.0 / weight));
        NurbsCurve::new(
            2,
            vec![first, last],
            vec![3, 3],
            vec![self.point_at(first), shoulder, self.point_at(last)],
            vec![1.0, weight, 1.0],
        )
    }
}

/// A portion of a hyperbola (`Part::GeomArcOfHyperbola`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcOfHyperbola {
    hyperbola: Hyperbola,
    first: f64,
    last: f64,
}

impl ArcOfHyperbola {
    /// The hyperbola between `first` and `last`; the range must run forwards.
    pub fn new(hyperbola: Hyperbola, first: f64, last: f64) -> Result<Self, CurveError> {
        let (first, last) = forward_range(first, last)?;
        Ok(Self {
            hyperbola,
            first,
            last,
        })
    }

    /// The supporting hyperbola.
    pub fn hyperbola(&self) -> Hyperbola {
        self.hyperbola
    }

    /// The first parameter.
    pub fn first(&self) -> f64 {
        self.first
    }

    /// The last parameter.
    pub fn last(&self) -> f64 {
        self.last
    }

    /// The point at parameter `u` of the supporting hyperbola.
    pub fn point_at(&self, u: f64) -> Vector3 {
        self.hyperbola.point_at(u)
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.hyperbola.point_at(self.first)
    }

    /// The last endpoint.
    pub fn end(&self) -> Vector3 {
        self.hyperbola.point_at(self.last)
    }

    /// The exact rational quadratic B-spline over the arc.
    pub fn nurbs(&self) -> Result<NurbsCurve, CurveError> {
        self.hyperbola.nurbs(self.first, self.last)
    }
}

/// A parabola (`Part::GeomParabola`): `Y^2 = 2*p*X` with `p = 2*focal` in its
/// own plane, opening towards the positive x axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parabola {
    vertex: Vector3,
    normal: Vector3,
    x_axis: Vector3,
    focal: f64,
}

impl Parabola {
    /// A parabola with an automatically chosen axis direction.
    pub fn new(vertex: Vector3, normal: Vector3, focal: f64) -> Result<Self, CurveError> {
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        Self::with_axis(vertex, normal, perpendicular(normal), focal)
    }

    /// A parabola whose symmetry axis points along `x_axis` (projected into the plane).
    pub fn with_axis(
        vertex: Vector3,
        normal: Vector3,
        x_axis: Vector3,
        focal: f64,
    ) -> Result<Self, CurveError> {
        if !focal.is_finite() || focal <= TOLERANCE {
            return Err(CurveError::Degenerate);
        }
        let normal = unit(normal).ok_or(CurveError::Degenerate)?;
        let projected = x_axis.sub(&normal.scale(normal.dot(&x_axis)));
        Ok(Self {
            vertex,
            normal,
            x_axis: unit(projected).ok_or(CurveError::Degenerate)?,
            focal,
        })
    }

    /// The vertex (OCC's location).
    pub fn vertex(&self) -> Vector3 {
        self.vertex
    }

    /// The unit normal of the parabola's plane.
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The symmetry axis direction (parameter `0`).
    pub fn x_axis(&self) -> Vector3 {
        self.x_axis
    }

    /// The direction of the directrix (`normal × x_axis`).
    pub fn y_axis(&self) -> Vector3 {
        self.normal.cross(&self.x_axis)
    }

    /// The focal length (distance from the vertex to the focus).
    pub fn focal(&self) -> f64 {
        self.focal
    }

    /// The focus.
    pub fn focus(&self) -> Vector3 {
        self.vertex.add(&self.x_axis.scale(self.focal))
    }

    /// The parameter (twice the focal length).
    pub fn parameter(&self) -> f64 {
        2.0 * self.focal
    }

    /// The eccentricity (`1` for every parabola).
    pub fn eccentricity(&self) -> f64 {
        1.0
    }

    /// The point at the OCC parameter `u`:
    /// `vertex + (u^2 / (4*focal))*x_axis + u*y_axis`.
    pub fn point_at(&self, u: f64) -> Vector3 {
        self.vertex
            .add(&self.x_axis.scale(u * u / (4.0 * self.focal)))
            .add(&self.y_axis().scale(u))
    }

    /// The unit tangent at `u`.
    pub fn tangent_at(&self, u: f64) -> Vector3 {
        let tangent = self
            .x_axis
            .scale(u / (2.0 * self.focal))
            .add(&self.y_axis());
        unit(tangent).expect("the focal length is positive")
    }

    /// The portion of the parabola between `first` and `last`.
    pub fn trim(&self, first: f64, last: f64) -> Result<ArcOfParabola, CurveError> {
        ArcOfParabola::new(*self, first, last)
    }

    /// The exact quadratic (non-rational) B-spline over `[first, last]`.
    ///
    /// The middle pole is the mid-curve point moved away from the focus by
    /// `(half-range)^2 / (4*focal)`, which is the intersection of the endpoint
    /// tangents.
    pub fn nurbs(&self, first: f64, last: f64) -> Result<NurbsCurve, CurveError> {
        let (first, last) = forward_range(first, last)?;
        let half = 0.5 * (last - first);
        let shift = half * half / (4.0 * self.focal);
        if !shift.is_finite() {
            return Err(CurveError::InvalidParameter);
        }
        let middle = self.point_at(0.5 * (first + last));
        let shoulder = middle.sub(&self.x_axis.scale(shift));
        NurbsCurve::new(
            2,
            vec![first, last],
            vec![3, 3],
            vec![self.point_at(first), shoulder, self.point_at(last)],
            vec![1.0, 1.0, 1.0],
        )
    }
}

/// A portion of a parabola (`Part::GeomArcOfParabola`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcOfParabola {
    parabola: Parabola,
    first: f64,
    last: f64,
}

impl ArcOfParabola {
    /// The parabola between `first` and `last`; the range must run forwards.
    pub fn new(parabola: Parabola, first: f64, last: f64) -> Result<Self, CurveError> {
        let (first, last) = forward_range(first, last)?;
        Ok(Self {
            parabola,
            first,
            last,
        })
    }

    /// The supporting parabola.
    pub fn parabola(&self) -> Parabola {
        self.parabola
    }

    /// The first parameter.
    pub fn first(&self) -> f64 {
        self.first
    }

    /// The last parameter.
    pub fn last(&self) -> f64 {
        self.last
    }

    /// The point at parameter `u` of the supporting parabola.
    pub fn point_at(&self, u: f64) -> Vector3 {
        self.parabola.point_at(u)
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.parabola.point_at(self.first)
    }

    /// The last endpoint.
    pub fn end(&self) -> Vector3 {
        self.parabola.point_at(self.last)
    }

    /// The exact quadratic B-spline over the arc.
    pub fn nurbs(&self) -> Result<NurbsCurve, CurveError> {
        self.parabola.nurbs(self.first, self.last)
    }
}

/// The homogeneous form `(w*x, w*y, w*z, w)` of a rational Bezier curve.
fn homogeneous(poles: &[Vector3], weights: &[f64]) -> Vec<[f64; 4]> {
    poles
        .iter()
        .zip(weights)
        .map(|(pole, weight)| {
            [
                pole.x * weight,
                pole.y * weight,
                pole.z * weight,
                *weight,
            ]
        })
        .collect()
}

/// Evaluate a Bernstein polynomial given its control values, by de Casteljau.
fn de_casteljau(control: &[[f64; 4]], t: f64) -> [f64; 4] {
    let mut work = control.to_vec();
    for remaining in (1..work.len()).rev() {
        for index in 0..remaining {
            for component in 0..4 {
                work[index][component] =
                    (1.0 - t) * work[index][component] + t * work[index + 1][component];
            }
        }
    }
    work.first().copied().unwrap_or([0.0; 4])
}

/// Split a Bernstein polynomial at `t`, returning its `[0, t]` and `[t, 1]` halves.
fn split_bezier(control: &[[f64; 4]], t: f64) -> (Vec<[f64; 4]>, Vec<[f64; 4]>) {
    let count = control.len();
    let mut left = vec![[0.0; 4]; count];
    let mut right = vec![[0.0; 4]; count];
    if count == 0 {
        return (left, right);
    }
    let mut work = control.to_vec();
    left[0] = work[0];
    right[count - 1] = work[count - 1];
    for level in 1..count {
        for index in 0..(count - level) {
            for component in 0..4 {
                work[index][component] = (1.0 - t) * work[index][component]
                    + t * work[index + 1][component];
            }
        }
        left[level] = work[0];
        right[count - 1 - level] = work[count - 1 - level];
    }
    (left, right)
}

/// Convert homogeneous control points back to poles and positive weights.
fn from_homogeneous(control: &[[f64; 4]]) -> (Vec<Vector3>, Vec<f64>) {
    let mut poles = Vec::with_capacity(control.len());
    let mut weights = Vec::with_capacity(control.len());
    for value in control {
        let weight = value[3];
        poles.push(Vector3::new(
            value[0] / weight,
            value[1] / weight,
            value[2] / weight,
        ));
        weights.push(weight);
    }
    (poles, weights)
}

/// A rational or non-rational Bezier curve (`Part::GeomBezierCurve`),
/// parameterised over `[first, last]` (uniform `[0, 1]` by default).
#[derive(Debug, Clone, PartialEq)]
pub struct BezierCurve {
    poles: Vec<Vector3>,
    weights: Vec<f64>,
    first: f64,
    last: f64,
}

impl Default for BezierCurve {
    /// Upstream's default `GeomBezierCurve`: two poles on `+Z`, over `[0, 1]`.
    fn default() -> Self {
        Self {
            poles: vec![Vector3::zero(), Vector3::new(0.0, 0.0, 1.0)],
            weights: vec![1.0, 1.0],
            first: 0.0,
            last: 1.0,
        }
    }
}

impl BezierCurve {
    /// A Bezier curve over `[0, 1]`.
    pub fn new(poles: Vec<Vector3>, weights: Vec<f64>) -> Result<Self, CurveError> {
        Self::with_range(poles, weights, 0.0, 1.0)
    }

    /// A Bezier curve over `[first, last]`.
    pub fn with_range(
        poles: Vec<Vector3>,
        weights: Vec<f64>,
        first: f64,
        last: f64,
    ) -> Result<Self, CurveError> {
        if poles.len() < 2 {
            return Err(CurveError::TooFewPoints);
        }
        if poles.len() != weights.len() {
            return Err(CurveError::Degenerate);
        }
        if poles.len() > 26 {
            return Err(CurveError::DegreeTooHigh);
        }
        if poles
            .iter()
            .any(|pole| !pole.x.is_finite() || !pole.y.is_finite() || !pole.z.is_finite())
        {
            return Err(CurveError::Degenerate);
        }
        if weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight <= 0.0)
        {
            return Err(CurveError::Degenerate);
        }
        let (first, last) = forward_range(first, last)?;
        Ok(Self {
            poles,
            weights,
            first,
            last,
        })
    }

    /// The control poles.
    pub fn poles(&self) -> &[Vector3] {
        &self.poles
    }

    /// One weight per pole.
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }

    /// The first parameter.
    pub fn first(&self) -> f64 {
        self.first
    }

    /// The last parameter.
    pub fn last(&self) -> f64 {
        self.last
    }

    /// The polynomial degree (`poles.len() - 1`).
    pub fn degree(&self) -> i32 {
        self.poles.len() as i32 - 1
    }

    /// Whether any weight departs from `1`.
    pub fn is_rational(&self) -> bool {
        self.weights.iter().any(|weight| (*weight - 1.0).abs() > 1e-12)
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.point_at(self.first)
    }

    /// The last endpoint.
    pub fn end(&self) -> Vector3 {
        self.point_at(self.last)
    }

    /// Whether the curve ends where it starts.
    pub fn is_closed(&self) -> bool {
        self.start().distance(&self.end()) <= TOLERANCE
    }

    /// The normalized parameter of `u`.
    fn parameter(&self, u: f64) -> f64 {
        (u - self.first) / (self.last - self.first)
    }

    /// The point at parameter `u` (rational de Casteljau).
    pub fn point_at(&self, u: f64) -> Vector3 {
        let value = de_casteljau(&homogeneous(&self.poles, &self.weights), self.parameter(u));
        Vector3::new(value[0] / value[3], value[1] / value[3], value[2] / value[3])
    }

    /// The unit tangent at `u`.
    pub fn tangent_at(&self, u: f64) -> Vector3 {
        let degree = self.degree() as f64;
        let control = homogeneous(&self.poles, &self.weights);
        let derivative: Vec<[f64; 4]> = control
            .windows(2)
            .map(|pair| {
                let mut value = [0.0; 4];
                for component in 0..4 {
                    value[component] = degree * (pair[1][component] - pair[0][component]);
                }
                value
            })
            .collect();
        let t = self.parameter(u);
        let value = de_casteljau(&control, t);
        let slope = de_casteljau(&derivative, t);
        let weight = value[3];
        let point = Vector3::new(value[0] / weight, value[1] / weight, value[2] / weight);
        let tangent = Vector3::new(
            (slope[0] - slope[3] * point.x) / weight,
            (slope[1] - slope[3] * point.y) / weight,
            (slope[2] - slope[3] * point.z) / weight,
        );
        unit(tangent).unwrap_or_else(Vector3::zero)
    }

    /// The portion of this Bezier curve between `first` and `last`.
    ///
    /// De Casteljau subdivision keeps the curve exactly the same; the result
    /// carries the same parameters (it is **not** renormalised to `[0, 1]`),
    /// as OCC's `Geom_BezierCurve::Segment`. The range must lie inside the
    /// curve's own range.
    pub fn trim(&self, first: f64, last: f64) -> Result<Self, CurveError> {
        let (first, last) = forward_range(first, last)?;
        if first < self.first - TOLERANCE || last > self.last + TOLERANCE {
            return Err(CurveError::InvalidParameter);
        }
        let first = first.clamp(self.first, self.last);
        let last = last.clamp(self.first, self.last);
        if !(last > first) {
            return Err(CurveError::InvalidParameter);
        }
        let span = self.last - self.first;
        let t1 = (first - self.first) / span;
        let t2 = (last - self.first) / span;
        let control = homogeneous(&self.poles, &self.weights);
        let (_, right) = split_bezier(&control, t1);
        let segment = if t2 >= 1.0 {
            right
        } else {
            let s = (t2 - t1) / (1.0 - t1);
            split_bezier(&right, s).0
        };
        let (poles, weights) = from_homogeneous(&segment);
        Self::with_range(poles, weights, first, last)
    }

    /// Move pole `index` (0-based) and optionally replace its weight.
    pub fn set_pole(
        &mut self,
        index: usize,
        pole: Vector3,
        weight: Option<f64>,
    ) -> Result<(), CurveError> {
        if index >= self.poles.len()
            || !pole.x.is_finite()
            || !pole.y.is_finite()
            || !pole.z.is_finite()
        {
            return Err(CurveError::Degenerate);
        }
        self.poles[index] = pole;
        if let Some(weight) = weight {
            if !weight.is_finite() || weight <= 0.0 {
                return Err(CurveError::Degenerate);
            }
            self.weights[index] = weight;
        }
        Ok(())
    }

    /// Replace every pole; the weights become `1` and the range is kept.
    pub fn set_poles(&mut self, poles: Vec<Vector3>) -> Result<(), CurveError> {
        let weights = vec![1.0; poles.len()];
        *self = Self::with_range(poles, weights, self.first, self.last)?;
        Ok(())
    }

    /// Replace the weight of pole `index` (0-based).
    pub fn set_weight(&mut self, index: usize, weight: f64) -> Result<(), CurveError> {
        if index >= self.weights.len() || !weight.is_finite() || weight <= 0.0 {
            return Err(CurveError::Degenerate);
        }
        self.weights[index] = weight;
        Ok(())
    }

    /// The exact B-spline form: the same poles and weights, with the two end
    /// knots carrying `degree + 1` multiplicities.
    pub fn nurbs(&self) -> NurbsCurve {
        let degree = self.degree();
        NurbsCurve::new(
            degree,
            vec![self.first, self.last],
            vec![degree + 1, degree + 1],
            self.poles.clone(),
            self.weights.clone(),
        )
        .expect("a Bezier curve is a valid clamped B-spline")
    }
}

// ---------------------------------------------------------------------------
// EdgeCurve
// ---------------------------------------------------------------------------

/// One analytic curve, as a `Part` edge's geometry (`edge.Curve` upstream).
///
/// The infinite [`Line`] has no endpoints, so it cannot be part of a [`Wire`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EdgeCurve {
    /// An infinite line.
    Line(Line),
    /// A bounded straight segment.
    LineSegment(LineSegment),
    /// A full circle.
    Circle(Circle),
    /// A circular arc.
    Arc(Arc),
    /// A full ellipse.
    Ellipse(Ellipse),
}

impl EdgeCurve {
    /// The point at `parameter` (see the module conventions).
    pub fn point_at(&self, parameter: f64) -> Vector3 {
        match self {
            EdgeCurve::Line(curve) => curve.point_at(parameter),
            EdgeCurve::LineSegment(curve) => curve.point_at(parameter),
            EdgeCurve::Circle(curve) => curve.point_at(parameter),
            EdgeCurve::Arc(curve) => curve.point_at(parameter),
            EdgeCurve::Ellipse(curve) => curve.point_at(parameter),
        }
    }

    /// The unit tangent at `parameter`.
    pub fn tangent_at(&self, parameter: f64) -> Vector3 {
        match self {
            EdgeCurve::Line(curve) => curve.tangent_at(parameter),
            EdgeCurve::LineSegment(curve) => curve.tangent_at(parameter),
            EdgeCurve::Circle(curve) => curve.tangent_at(parameter),
            EdgeCurve::Arc(curve) => curve.tangent_at(parameter),
            EdgeCurve::Ellipse(curve) => curve.tangent_at(parameter),
        }
    }

    /// The length (`f64::INFINITY` for an infinite line; the ellipse value is an
    /// approximation).
    pub fn length(&self) -> f64 {
        match self {
            EdgeCurve::Line(_) => f64::INFINITY,
            EdgeCurve::LineSegment(curve) => curve.length(),
            EdgeCurve::Circle(curve) => curve.length(),
            EdgeCurve::Arc(curve) => curve.length(),
            EdgeCurve::Ellipse(curve) => curve.length(),
        }
    }

    /// The start and end points, or `None` for an infinite [`Line`].
    ///
    /// A circle or an ellipse reports the same point twice.
    pub fn endpoints(&self) -> Option<(Vector3, Vector3)> {
        match self {
            EdgeCurve::Line(_) => None,
            EdgeCurve::LineSegment(curve) => Some((curve.start(), curve.end())),
            EdgeCurve::Circle(curve) => {
                let point = curve.point_at(0.0);
                Some((point, point))
            }
            EdgeCurve::Arc(curve) => Some((curve.start(), curve.end())),
            EdgeCurve::Ellipse(curve) => {
                let point = curve.point_at(0.0);
                Some((point, point))
            }
        }
    }

    /// Whether the curve ends where it starts.
    pub fn is_closed(&self) -> bool {
        match self {
            EdgeCurve::Line(_) | EdgeCurve::LineSegment(_) => false,
            EdgeCurve::Circle(_) | EdgeCurve::Ellipse(_) => true,
            EdgeCurve::Arc(curve) => curve.is_closed(),
        }
    }

    /// The same geometry traversed the other way, when it has a direction.
    ///
    /// A full circle or ellipse is unchanged; an infinite line has no direction to
    /// reverse in this model.
    pub fn reversed(&self) -> Self {
        match self {
            EdgeCurve::Line(curve) => EdgeCurve::Line(*curve),
            EdgeCurve::LineSegment(curve) => EdgeCurve::LineSegment(curve.reversed()),
            EdgeCurve::Circle(curve) => EdgeCurve::Circle(*curve),
            EdgeCurve::Arc(curve) => EdgeCurve::Arc(curve.reversed()),
            EdgeCurve::Ellipse(curve) => EdgeCurve::Ellipse(*curve),
        }
    }

    /// The upstream class name (`"LineSegment"`, `"ArcOfCircle"`, …).
    pub fn kind(&self) -> &'static str {
        match self {
            EdgeCurve::Line(_) => "Line",
            EdgeCurve::LineSegment(_) => "LineSegment",
            EdgeCurve::Circle(_) => "Circle",
            EdgeCurve::Arc(_) => "ArcOfCircle",
            EdgeCurve::Ellipse(_) => "Ellipse",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 1e-12;

    fn close(a: Vector3, b: Vector3) -> bool {
        a.distance(&b) <= 1e-9
    }

    #[test]
    fn vector2_arithmetic_and_distance() {
        let a = Vector2::new(3.0, 4.0);
        assert!((a.length() - 5.0).abs() < TOL);
        assert!((a.distance(&Vector2::zero()) - 5.0).abs() < TOL);
        assert_eq!(a.add(&Vector2::new(1.0, -1.0)), Vector2::new(4.0, 3.0));
        assert!((a.dot(&a) - 25.0).abs() < TOL);
    }

    #[test]
    fn a_segment_is_parameterised_from_start_to_end() {
        let segment =
            LineSegment::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(1.0, 2.0, 6.0)).unwrap();
        assert!(close(segment.point_at(0.0), segment.start()));
        assert!(close(segment.point_at(1.0), segment.end()));
        assert!((segment.point_at(0.5).z - 4.5).abs() < TOL);
        assert!((segment.length() - 3.0).abs() < TOL);
        assert!(close(
            segment.tangent_at(0.25),
            Vector3::new(0.0, 0.0, 1.0)
        ));
    }

    #[test]
    fn a_reversed_segment_traverses_the_other_way() {
        let segment =
            LineSegment::new(Vector3::zero(), Vector3::new(2.0, 0.0, 0.0)).unwrap();
        let reversed = segment.reversed();
        assert!(close(reversed.start(), segment.end()));
        assert!(close(reversed.end(), segment.start()));
        assert!(close(
            reversed.tangent_at(0.5),
            Vector3::new(-1.0, 0.0, 0.0)
        ));
    }

    #[test]
    fn a_degenerate_segment_is_rejected() {
        assert_eq!(
            LineSegment::new(Vector3::new(1.0, 1.0, 1.0), Vector3::new(1.0, 1.0, 1.0)),
            Err(CurveError::Degenerate)
        );
        assert_eq!(
            Line::new(Vector3::zero(), Vector3::zero()),
            Err(CurveError::Degenerate)
        );
    }

    #[test]
    fn a_circle_uses_its_normal_and_x_axis_for_the_plane() {
        let circle = Circle::new(
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 2.0),
            5.0,
        )
        .unwrap();
        assert!((circle.normal().length() - 1.0).abs() < TOL, "the normal is unit");
        let x = circle.x_axis();
        assert!(x.dot(&circle.normal()).abs() < 1e-9, "x_axis lies in the plane");
        assert!(close(circle.point_at(0.0), circle.center().add(&x.scale(5.0))));
        let quarter = circle.point_at(std::f64::consts::FRAC_PI_2);
        assert!(close(
            quarter,
            circle.center().add(&circle.y_axis().scale(5.0))
        ));
        assert!((circle.length() - std::f64::consts::TAU * 5.0).abs() < 1e-9);
        assert!(close(circle.tangent_at(0.0), circle.y_axis()));
    }

    #[test]
    fn a_circle_with_an_explicit_axis_keeps_it() {
        let circle = Circle::with_axis(
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 1.0, 5.0),
            2.0,
        )
        .unwrap();
        let expected = Vector3::new(1.0, 1.0, 0.0).normalize();
        assert!(close(circle.x_axis(), expected), "the axis is projected into the plane");
        assert!(close(circle.point_at(0.0), expected.scale(2.0)));
    }

    #[test]
    fn an_arc_sweeps_between_its_angles_and_reverses() {
        let circle = Circle::new(
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
        )
        .unwrap();
        let arc = Arc::new(circle, 0.0, std::f64::consts::FRAC_PI_2).unwrap();
        assert!((arc.sweep() - std::f64::consts::FRAC_PI_2).abs() < TOL);
        assert!((arc.length() - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!(!arc.is_closed());

        let reversed = arc.reversed();
        assert!(close(reversed.start(), arc.end()));
        assert!(close(reversed.end(), arc.start()));
        assert!((reversed.sweep() + arc.sweep()).abs() < TOL);
        assert!((reversed.length() - arc.length()).abs() < 1e-9);
    }

    #[test]
    fn a_zero_sweep_arc_is_rejected_and_a_full_turn_is_closed() {
        let circle = Circle::new(Vector3::zero(), Vector3::new(0.0, 0.0, 1.0), 1.0).unwrap();
        assert_eq!(
            Arc::new(circle, 1.0, 1.0),
            Err(CurveError::Degenerate)
        );
        let full = Arc::new(circle, 0.0, std::f64::consts::TAU).unwrap();
        assert!(full.is_closed());
        assert!((full.length() - std::f64::consts::TAU).abs() < 1e-9);
    }

    #[test]
    fn an_ellipse_parameterises_both_radii() {
        let ellipse = Ellipse::new(
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
            4.0,
            2.0,
        )
        .unwrap();
        assert!((ellipse.major_radius() - 4.0).abs() < TOL);
        assert!((ellipse.minor_radius() - 2.0).abs() < TOL);
        assert!(close(ellipse.point_at(0.0), ellipse.x_axis().scale(4.0)));
        assert!(close(
            ellipse.point_at(std::f64::consts::FRAC_PI_2),
            ellipse.y_axis().scale(2.0)
        ));
        // The perimeter is between the inscribed and circumscribed circles.
        let pi = std::f64::consts::PI;
        assert!(ellipse.length() > pi * (4.0 + 2.0) * 0.9);
        assert!(ellipse.length() < pi * (4.0 + 2.0) * 1.05);
        assert!(ellipse.tangent_at(0.0).length() - 1.0 < 1e-12);
    }

    #[test]
    fn the_larger_radius_becomes_the_major_one() {
        let ellipse = Ellipse::new(
            Vector3::zero(),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
            3.0,
        )
        .unwrap();
        assert!((ellipse.major_radius() - 3.0).abs() < TOL);
        assert!((ellipse.minor_radius() - 1.0).abs() < TOL);
    }

    #[test]
    fn an_edge_curve_dispatches_and_names_its_upstream_class() {
        let segment = EdgeCurve::LineSegment(
            LineSegment::new(Vector3::zero(), Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        );
        assert_eq!(segment.kind(), "LineSegment");
        assert_eq!(segment.length(), 1.0);
        assert!(!segment.is_closed());
        let (start, end) = segment.endpoints().expect("a segment is bounded");
        assert!(close(start, Vector3::zero()));
        assert!(close(end, Vector3::new(1.0, 0.0, 0.0)));

        let line = EdgeCurve::Line(Line::new(Vector3::zero(), Vector3::new(1.0, 0.0, 0.0)).unwrap());
        assert!(line.endpoints().is_none(), "an infinite line has no endpoints");
        assert!(line.length().is_infinite());
    }

    fn axis_x() -> Vector3 {
        Vector3::new(1.0, 0.0, 0.0)
    }

    fn axis_z() -> Vector3 {
        Vector3::new(0.0, 0.0, 1.0)
    }

    #[test]
    fn a_hyperbola_follows_its_parameterisation() {
        let hyperbola = Hyperbola::with_axis(
            Vector3::new(1.0, 2.0, 3.0),
            axis_z(),
            axis_x(),
            3.0,
            2.0,
        )
        .unwrap();
        assert!(close(
            hyperbola.point_at(0.0),
            Vector3::new(4.0, 2.0, 3.0)
        ));
        for u in [-2.0, -0.5, 0.0, 0.5, 2.0] {
            let offset = hyperbola.point_at(u).sub(&hyperbola.center());
            let x = offset.dot(&hyperbola.x_axis());
            let y = offset.dot(&hyperbola.y_axis());
            assert!(
                (x * x / 9.0 - y * y / 4.0 - 1.0).abs() < 1e-9,
                "point at {u} is off the hyperbola"
            );
        }
        let focal = 13.0f64.sqrt();
        assert!((hyperbola.focal() - focal).abs() < 1e-12);
        assert!(close(
            hyperbola.focus1(),
            hyperbola.center().add(&axis_x().scale(focal))
        ));
        assert!(close(
            hyperbola.focus2(),
            hyperbola.center().sub(&axis_x().scale(focal))
        ));
        assert!((hyperbola.eccentricity() - focal / 3.0).abs() < 1e-12);
        assert!(close(hyperbola.tangent_at(0.0), hyperbola.y_axis()));
    }

    #[test]
    fn a_hyperbola_nurbs_is_exact() {
        let hyperbola = Hyperbola::with_axis(
            Vector3::new(-1.0, 4.0, 2.0),
            axis_z(),
            axis_x(),
            3.0,
            2.0,
        )
        .unwrap();
        let spline = hyperbola.nurbs(-1.0, 2.0).unwrap();
        assert_eq!(spline.degree(), 2);
        assert_eq!(spline.mults(), &[3, 3]);
        assert_eq!(spline.knots(), &[-1.0, 2.0]);
        assert!(spline.is_rational());
        assert!(close(
            spline.point_at(-1.0).unwrap(),
            hyperbola.point_at(-1.0)
        ));
        assert!(close(
            spline.point_at(2.0).unwrap(),
            hyperbola.point_at(2.0)
        ));
        // The B-spline parameter is not the hyperbola parameter, but every point
        // it produces is on the hyperbola.
        for t in [-1.0, -0.25, 0.5, 1.25, 2.0] {
            let offset = spline.point_at(t).unwrap().sub(&hyperbola.center());
            let x = offset.dot(&hyperbola.x_axis());
            let y = offset.dot(&hyperbola.y_axis());
            assert!(
                (x * x / 9.0 - y * y / 4.0 - 1.0).abs() < 1e-9,
                "NURBS point at {t} is off the hyperbola"
            );
        }
    }

    #[test]
    fn a_parabola_is_the_locus_of_equal_distances() {
        let parabola =
            Parabola::with_axis(Vector3::new(1.0, 1.0, 1.0), axis_z(), axis_x(), 2.0).unwrap();
        assert!(close(
            parabola.point_at(0.0),
            Vector3::new(1.0, 1.0, 1.0)
        ));
        for u in [-3.0, -1.0, 0.5, 2.0] {
            let point = parabola.point_at(u);
            let to_focus = point.distance(&parabola.focus());
            let x = point.sub(&parabola.vertex()).dot(&parabola.x_axis());
            assert!(
                (to_focus - (x + parabola.focal())).abs() < 1e-9,
                "point at {u} is not equidistant from the focus and the directrix"
            );
        }
        assert!((parabola.parameter() - 4.0).abs() < 1e-12);
        assert!((parabola.eccentricity() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_parabola_nurbs_is_exact() {
        let parabola = Parabola::with_axis(Vector3::zero(), axis_z(), axis_x(), 2.0).unwrap();
        let spline = parabola.nurbs(-2.0, 3.0).unwrap();
        assert_eq!(spline.degree(), 2);
        assert!(!spline.is_rational());
        assert!(close(
            spline.point_at(-2.0).unwrap(),
            parabola.point_at(-2.0)
        ));
        assert!(close(
            spline.point_at(3.0).unwrap(),
            parabola.point_at(3.0)
        ));
        for t in [-2.0, -0.5, 1.0, 3.0] {
            let point = spline.point_at(t).unwrap();
            let to_focus = point.distance(&parabola.focus());
            let x = point.sub(&parabola.vertex()).dot(&parabola.x_axis());
            assert!(
                (to_focus - (x + parabola.focal())).abs() < 1e-9,
                "B-spline point at {t} is off the parabola"
            );
        }
    }

    #[test]
    fn a_conic_arc_trims_to_its_range() {
        let hyperbola =
            Hyperbola::with_axis(Vector3::zero(), axis_z(), axis_x(), 3.0, 2.0).unwrap();
        let arc = hyperbola.trim(0.5, 2.0).unwrap();
        assert!((arc.first() - 0.5).abs() < TOL);
        assert!((arc.last() - 2.0).abs() < TOL);
        assert!(close(arc.start(), hyperbola.point_at(0.5)));
        assert!(close(arc.end(), hyperbola.point_at(2.0)));

        let parabola = Parabola::with_axis(Vector3::zero(), axis_z(), axis_x(), 1.0).unwrap();
        let arc = parabola.trim(-1.0, 1.5).unwrap();
        assert!(close(arc.start(), parabola.point_at(-1.0)));
        assert!(close(arc.end(), parabola.point_at(1.5)));
    }

    #[test]
    fn a_bezier_curve_is_evaluated_by_de_casteljau() {
        let bezier = BezierCurve::new(
            vec![
                Vector3::zero(),
                Vector3::new(1.0, 2.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            vec![1.0, 1.0, 1.0],
        )
        .unwrap();
        assert!(close(bezier.start(), Vector3::zero()));
        assert!(close(bezier.end(), Vector3::new(2.0, 0.0, 0.0)));
        assert!(close(bezier.point_at(0.5), Vector3::new(1.0, 1.0, 0.0)));
        assert_eq!(bezier.degree(), 2);
        assert!(!bezier.is_rational());
        assert!(!bezier.is_closed());
        assert!(close(
            bezier.tangent_at(0.0),
            Vector3::new(1.0, 2.0, 0.0).normalize()
        ));
    }

    #[test]
    fn a_rational_bezier_is_a_quarter_circle() {
        let bezier = BezierCurve::new(
            vec![
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(1.0, 1.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ],
            vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0],
        )
        .unwrap();
        assert!(bezier.is_rational());
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(
                (bezier.point_at(t).length() - 1.0).abs() < 1e-12,
                "point at {t} is off the unit circle"
            );
        }
    }

    #[test]
    fn trimming_a_bezier_preserves_its_points() {
        let bezier = BezierCurve::new(
            vec![
                Vector3::zero(),
                Vector3::new(1.0, 2.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            vec![1.0, 1.0, 1.0],
        )
        .unwrap();
        let part = bezier.trim(0.25, 0.75).unwrap();
        assert!((part.first() - 0.25).abs() < TOL);
        assert!((part.last() - 0.75).abs() < TOL);
        for parameter in [0.25, 0.4, 0.5, 0.6, 0.75] {
            assert!(close(part.point_at(parameter), bezier.point_at(parameter)));
        }
    }

    #[test]
    fn a_bezier_nurbs_keeps_its_poles_and_degree() {
        let bezier = BezierCurve::with_range(
            vec![
                Vector3::zero(),
                Vector3::new(1.0, 2.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            vec![1.0, 1.0, 1.0],
            2.0,
            5.0,
        )
        .unwrap();
        let spline = bezier.nurbs();
        assert_eq!(spline.degree(), 2);
        assert_eq!(spline.knots(), &[2.0, 5.0]);
        assert_eq!(spline.mults(), &[3, 3]);
        assert_eq!(spline.poles(), bezier.poles());
        for parameter in [2.0, 3.0, 4.0, 5.0] {
            assert!(close(
                spline.point_at(parameter).unwrap(),
                bezier.point_at(parameter)
            ));
        }
    }
}
