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

    /// The portion of the ellipse between `first` and `last` (radians).
    pub fn trim(&self, first: f64, last: f64) -> Result<EllipticalArc, CurveError> {
        EllipticalArc::new(*self, first, last)
    }
}

/// A portion of an ellipse (`Part::GeomArcOfEllipse`), parameterised by angle.
///
/// Like [`Arc`], `first` and `last` are stored as given, so a negative sweep runs
/// clockwise and [`EllipticalArc::reversed`] traverses the same geometry the other
/// way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipticalArc {
    ellipse: Ellipse,
    first: f64,
    last: f64,
}

impl EllipticalArc {
    /// The ellipse between `first` and `last` (radians); a zero sweep is rejected.
    pub fn new(ellipse: Ellipse, first: f64, last: f64) -> Result<Self, CurveError> {
        if !first.is_finite() || !last.is_finite() || (last - first).abs() <= ANGULAR_TOLERANCE {
            return Err(CurveError::Degenerate);
        }
        Ok(Self {
            ellipse,
            first,
            last,
        })
    }

    /// The supporting ellipse.
    pub fn ellipse(&self) -> Ellipse {
        self.ellipse
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
        self.ellipse.point_at(angle)
    }

    /// The unit tangent at `angle`.
    pub fn tangent_at(&self, angle: f64) -> Vector3 {
        self.ellipse.tangent_at(angle)
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.point_at(self.first)
    }

    /// The last endpoint.
    pub fn end(&self) -> Vector3 {
        self.point_at(self.last)
    }

    /// The arc length, by sampling the ellipse's speed (there is no closed form).
    pub fn length(&self) -> f64 {
        sampled_length(&|angle| self.point_at(angle), self.first, self.last)
    }

    /// Whether the sweep is a full turn.
    pub fn is_closed(&self) -> bool {
        (self.sweep().abs() - TAU).abs() <= ANGULAR_TOLERANCE
    }

    /// The same arc traversed in the opposite direction.
    pub fn reversed(&self) -> Self {
        Self {
            ellipse: self.ellipse,
            first: self.last,
            last: self.first,
        }
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

/// A curve's arc length by sampling `point_at`; `512` straight pieces follow a
/// smooth curve closely enough for a display/measurement length.
fn sampled_length(point_at: &dyn Fn(f64) -> Vector3, first: f64, last: f64) -> f64 {
    const STEPS: usize = 512;
    let mut length = 0.0;
    let mut previous = point_at(first);
    for step in 1..=STEPS {
        let parameter = first + (last - first) * step as f64 / STEPS as f64;
        let current = point_at(parameter);
        length += previous.distance(&current);
        previous = current;
    }
    length
}

/// A rational B-spline in the form the `Part.BSplineCurve` binding consumes:
/// a polynomial degree, unique knots, their multiplicities, poles and weights.
///
/// It is the exact conversion target for the conics and for a Bezier curve, so
/// the Part binding never has to approximate them by sampling.
///
/// [`NurbsCurve::new`] describes a clamped spline; [`NurbsCurve::new_periodic`]
/// describes a periodic one, whose poles and knots repeat with the period (see
/// [`is_periodic`](Self::is_periodic)).
#[derive(Debug, Clone, PartialEq)]
pub struct NurbsCurve {
    degree: i32,
    knots: Vec<f64>,
    mults: Vec<i32>,
    poles: Vec<Vector3>,
    weights: Vec<f64>,
    periodic: bool,
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
            periodic: false,
        })
    }

    /// A periodic rational B-spline.
    ///
    /// Periodicity changes the knot/multiplicity relation. The poles are cyclic,
    /// `Pole(i + n) = Pole(i)`, and so are the knots,
    /// `Knot(i + k) = Knot(i) + period`. The stored knot table therefore includes
    /// the knot that ends the period (its multiplicity repeats the first one), and
    /// the multiplicities sum to `poles.len() + mults[0]` instead of the clamped
    /// `poles.len() + degree + 1`. The period is
    /// `knots.last() - knots.first()`.
    pub fn new_periodic(
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
        if knots.len() < 2 || knots.len() != mults.len() {
            return Err(CurveError::Degenerate);
        }
        if knots.iter().any(|knot| !knot.is_finite())
            || !knots.windows(2).all(|pair| pair[0] < pair[1])
        {
            return Err(CurveError::InvalidParameter);
        }
        if mults[0] != mults[mults.len() - 1]
            || mults.iter().any(|mult| *mult < 1 || *mult > degree)
            || mults.iter().sum::<i32>() != poles.len() as i32 + mults[0]
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
            periodic: true,
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

    /// Whether the poles and knots repeat with the period (OCCT's
    /// `Geom_BSplineCurve::IsPeriodic`).
    pub fn is_periodic(&self) -> bool {
        self.periodic
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

    /// The knot sequence and cyclic pole map that evaluate a periodic curve on
    /// `[first, last]` as an ordinary B-spline.
    ///
    /// OCCT stores one period of knots plus the period-end knot. Writing that
    /// sequence out with its multiplicities and adding the
    /// `degree + 1 - mult(first)` knots that wrap in from either side gives a
    /// flat knot vector whose first `degree` knots precede the domain; the poles
    /// are then simply the original poles repeated cyclically.
    fn periodic_extension(&self) -> Option<(Vec<f64>, Vec<usize>)> {
        let degree = self.degree as usize;
        let first_mult = *self.mults.first()? as usize;
        let last_mult = *self.mults.last()? as usize;
        if first_mult == 0 || last_mult == 0 {
            return None;
        }
        let mut sequence = Vec::new();
        for (knot, mult) in self.knots.iter().zip(&self.mults) {
            for _ in 0..*mult {
                sequence.push(*knot);
            }
        }
        let period = self.last() - self.first();
        if !(period > 0.0) {
            return None;
        }
        // The continuity at the first knot: how many knots wrap in from each end.
        let wrap = (degree + 1).saturating_sub(first_mult);
        let first_last = sequence.len().checked_sub(last_mult)?;
        if wrap == 0 || first_last < wrap || first_mult + wrap > sequence.len() {
            return None;
        }
        let mut extended = Vec::with_capacity(sequence.len() + 2 * wrap);
        for value in &sequence[first_last - wrap..first_last] {
            extended.push(*value - period);
        }
        extended.extend_from_slice(&sequence);
        for value in &sequence[first_mult..first_mult + wrap] {
            extended.push(*value + period);
        }
        let pole_count = extended.len().checked_sub(degree + 1)?;
        if self.poles.is_empty() {
            return None;
        }
        let cyclic = (0..pole_count).map(|index| index % self.poles.len()).collect();
        Some((extended, cyclic))
    }

    /// The homogeneous (weighted) control point at `index`.
    fn homogeneous(&self, index: usize) -> [f64; 4] {
        let weight = self.weights[index];
        [
            self.poles[index].x * weight,
            self.poles[index].y * weight,
            self.poles[index].z * weight,
            weight,
        ]
    }

    /// The point at parameter `u` by rational de Boor evaluation.
    ///
    /// A periodic parameter outside `[first, last]` wraps, and `last` is the same
    /// point as `first`. `None` when the description is invalid or the parameter
    /// is not finite.
    pub fn point_at(&self, u: f64) -> Option<Vector3> {
        if !u.is_finite() {
            return None;
        }
        let degree = self.degree as usize;
        if self.periodic {
            let (full, cyclic) = self.periodic_extension()?;
            let last_index = full.len().checked_sub(degree + 1)?;
            let first = *full.get(degree)?;
            let last = *full.get(last_index)?;
            let period = last - first;
            if !(period > 0.0) {
                return None;
            }
            // `last` is the start of the next period; every other value wraps.
            let u = if u == last {
                first
            } else {
                first + (u - first).rem_euclid(period)
            };
            let mut span = degree;
            if u >= last {
                span = last_index - 1;
            } else {
                while span + 1 < last_index && !(u >= full[span] && u < full[span + 1]) {
                    span += 1;
                }
            }
            return de_boor(degree, &full, u, span, |index| {
                self.homogeneous(cyclic[index])
            });
        }
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
        de_boor(degree, &full, u, span, |index| self.homogeneous(index))
    }

    /// The unit tangent at `u`, by a central difference of [`point_at`](Self::point_at).
    ///
    /// The curve is only stored as poles/knots here, so this avoids a second
    /// derivative implementation; the step is `1e-6` of the knot span.
    pub fn tangent_at(&self, u: f64) -> Vector3 {
        let (first, last) = (self.first(), self.last());
        if !(last > first) {
            return Vector3::zero();
        }
        let step = ((last - first) * 1e-6).max(1e-9);
        // A periodic parameter wraps through the seam, so it is not clamped.
        let (low, high) = if self.periodic {
            (u - step, u + step)
        } else {
            ((u - step).max(first), (u + step).min(last))
        };
        if !(high > low) {
            return Vector3::zero();
        }
        let (Some(low), Some(high)) = (self.point_at(low), self.point_at(high)) else {
            return Vector3::zero();
        };
        unit(high.sub(&low)).unwrap_or_else(Vector3::zero)
    }

    /// The arc length over `[first, last]` by sampling [`point_at`](Self::point_at).
    pub fn length(&self, first: f64, last: f64) -> f64 {
        let fallback = self.point_at(first).unwrap_or_else(Vector3::zero);
        sampled_length(
            &|u| self.point_at(u).unwrap_or(fallback),
            first.min(last),
            first.max(last),
        )
    }

    /// The same curve traversed in the opposite parameter direction.
    ///
    /// The control points and weights are reversed, and every knot `k` becomes
    /// `first + last - k`, so the new curve's domain is unchanged. A periodic
    /// curve's poles are cyclic, so the reversed list is rotated back onto the
    /// seam; otherwise its parameterisation would start one arc late.
    pub fn reversed(&self) -> Self {
        let (first, last) = (self.first(), self.last());
        let mut poles: Vec<Vector3> = self.poles.iter().rev().copied().collect();
        let mut weights: Vec<f64> = self.weights.iter().rev().copied().collect();
        if self.periodic {
            let count = self.poles.len();
            let first_mult = self.mults.first().copied().unwrap_or(1) as i64;
            let extended_poles = count as i64 + self.degree as i64 + 1 - first_mult;
            let shift = (count as i64 - extended_poles).rem_euclid(count as i64) as usize;
            poles.rotate_left(shift);
            weights.rotate_left(shift);
        }
        Self {
            degree: self.degree,
            knots: self
                .knots
                .iter()
                .rev()
                .map(|knot| first + last - knot)
                .collect(),
            mults: self.mults.iter().rev().copied().collect(),
            poles,
            weights,
            periodic: self.periodic,
        }
    }
}

/// Rational de Boor evaluation of one span.
///
/// `span` indexes the flat knot vector; `pole(index)` returns the homogeneous
/// control point for the basis function at flat knot index `index` (a clamped
/// curve uses the index directly, a periodic one maps it through the cyclic
/// pole list).
fn de_boor<F>(degree: usize, knots: &[f64], u: f64, span: usize, pole: F) -> Option<Vector3>
where
    F: Fn(usize) -> [f64; 4],
{
    let mut d: Vec<[f64; 4]> = (0..=degree).map(|index| pole(span + index - degree)).collect();
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let knot = j + span - degree;
            let denominator = knots[knot + degree + 1 - r] - knots[knot];
            let alpha = if denominator.abs() > 0.0 {
                (u - knots[knot]) / denominator
            } else {
                0.0
            };
            for component in 0..4 {
                d[j][component] = (1.0 - alpha) * d[j - 1][component] + alpha * d[j][component];
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

    /// The arc length, by sampling the supporting hyperbola.
    pub fn length(&self) -> f64 {
        sampled_length(&|u| self.point_at(u), self.first, self.last)
    }

    /// The same arc traversed in the opposite direction.
    pub fn reversed(&self) -> Self {
        Self {
            hyperbola: self.hyperbola,
            first: self.last,
            last: self.first,
        }
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

    /// The arc length, by sampling the parabola.
    pub fn length(&self) -> f64 {
        sampled_length(&|u| self.point_at(u), self.first, self.last)
    }

    /// The same arc traversed in the opposite direction.
    pub fn reversed(&self) -> Self {
        Self {
            parabola: self.parabola,
            first: self.last,
            last: self.first,
        }
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

    /// The arc length over the curve's range, by sampling.
    pub fn length(&self) -> f64 {
        sampled_length(&|u| self.point_at(u), self.first, self.last)
    }

    /// The same curve traversed in the opposite parameter direction.
    pub fn reversed(&self) -> Self {
        Self {
            poles: self.poles.iter().rev().copied().collect(),
            weights: self.weights.iter().rev().copied().collect(),
            first: self.first,
            last: self.last,
        }
    }
}

// ---------------------------------------------------------------------------
// OffsetCurve
// ---------------------------------------------------------------------------

/// An offset of another curve (`Part::GeomOffsetCurve`): the basis curve moved a
/// constant distance along the normal `tangent × direction`.
///
/// OCCT's parameterisation is the basis curve's, so `first`/`last` are the basis
/// parameter range and may be trimmed further by an edge. The basis is boxed
/// because an offset can wrap any [`EdgeCurve`] (and an offset curve can even be
/// the basis of another one).
#[derive(Debug, Clone, PartialEq)]
pub struct OffsetCurve {
    basis: Box<EdgeCurve>,
    offset: f64,
    direction: Vector3,
    first: f64,
    last: f64,
}

impl OffsetCurve {
    /// The offset of `basis` over `[first, last]`.
    pub fn new(
        basis: EdgeCurve,
        offset: f64,
        direction: Vector3,
        first: f64,
        last: f64,
    ) -> Result<Self, CurveError> {
        if !offset.is_finite() {
            return Err(CurveError::Degenerate);
        }
        if !first.is_finite() || !last.is_finite() || last <= first {
            return Err(CurveError::InvalidParameter);
        }
        Ok(Self {
            basis: Box::new(basis),
            offset,
            direction: unit(direction).ok_or(CurveError::Degenerate)?,
            first,
            last,
        })
    }

    /// The offset of a bounded basis over the basis' own parameter range.
    pub fn of(basis: EdgeCurve, offset: f64, direction: Vector3) -> Result<Self, CurveError> {
        let (first, last) = basis.parameter_range().ok_or(CurveError::NotBounded)?;
        Self::new(basis, offset, direction, first, last)
    }

    /// The curve being offset.
    pub fn basis(&self) -> &EdgeCurve {
        &self.basis
    }

    /// The signed offset distance.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The unit direction the offset is measured against.
    pub fn direction(&self) -> Vector3 {
        self.direction
    }

    /// The first parameter (the basis parameterisation).
    pub fn first(&self) -> f64 {
        self.first
    }

    /// The last parameter (the basis parameterisation).
    pub fn last(&self) -> f64 {
        self.last
    }

    /// The point at `u`: `basis(u) + offset * (tangent × direction)`, normalised so
    /// the shift has length `|offset|` (OCCT's `Geom_OffsetCurve::Value`).
    pub fn point_at(&self, u: f64) -> Vector3 {
        let normal = self.basis.tangent_at(u).cross(&self.direction);
        let length = normal.length();
        let shift = if length > TOLERANCE {
            normal.scale(self.offset / length)
        } else {
            Vector3::zero()
        };
        self.basis.point_at(u).add(&shift)
    }

    /// The unit tangent at `u`, by a central difference of [`point_at`](Self::point_at).
    pub fn tangent_at(&self, u: f64) -> Vector3 {
        let step = ((self.last - self.first) * 1e-6).max(1e-9);
        let low = (u - step).max(self.first);
        let high = (u + step).min(self.last);
        if !(high > low) {
            return self.basis.tangent_at(u);
        }
        unit(self.point_at(high).sub(&self.point_at(low))).unwrap_or_else(|| self.basis.tangent_at(u))
    }

    /// The first endpoint.
    pub fn start(&self) -> Vector3 {
        self.point_at(self.first)
    }

    /// The last endpoint.
    pub fn end(&self) -> Vector3 {
        self.point_at(self.last)
    }

    /// The arc length over `[first, last]`, by sampling.
    pub fn length(&self) -> f64 {
        sampled_length(&|u| self.point_at(u), self.first, self.last)
    }

    /// Whether the offset ends where it starts.
    pub fn is_closed(&self) -> bool {
        self.start().distance(&self.end()) <= TOLERANCE
    }

    /// The same geometry traversed the other way.
    ///
    /// The offset's parameterisation is the basis curve's, so reversing is just a
    /// swap of the traversal bounds: the point and its offset normal stay the
    /// same function of the parameter.
    pub fn reversed(&self) -> Self {
        Self {
            basis: self.basis.clone(),
            offset: self.offset,
            direction: self.direction,
            first: self.last,
            last: self.first,
        }
    }
}

// ---------------------------------------------------------------------------
// EdgeCurve
// ---------------------------------------------------------------------------

/// One analytic curve, as a `Part` edge's geometry (`edge.Curve` upstream).
///
/// The infinite [`Line`], [`Hyperbola`] and [`Parabola`] have no endpoints, so they
/// cannot be part of a [`Wire`]. Owned poles/weights make this `Clone` rather than
/// `Copy`.
#[derive(Debug, Clone, PartialEq)]
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
    /// A portion of an ellipse.
    EllipticalArc(EllipticalArc),
    /// A whole branch of a hyperbola (unbounded).
    Hyperbola(Hyperbola),
    /// A portion of a hyperbola.
    ArcOfHyperbola(ArcOfHyperbola),
    /// A whole parabola (unbounded).
    Parabola(Parabola),
    /// A portion of a parabola.
    ArcOfParabola(ArcOfParabola),
    /// A rational or non-rational Bezier curve.
    BezierCurve(BezierCurve),
    /// A rational B-spline with its poles, weights, knots and multiplicities.
    BSplineCurve(NurbsCurve),
    /// A constant offset of another curve.
    OffsetCurve(OffsetCurve),
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
            EdgeCurve::EllipticalArc(curve) => curve.point_at(parameter),
            EdgeCurve::Hyperbola(curve) => curve.point_at(parameter),
            EdgeCurve::ArcOfHyperbola(curve) => curve.point_at(parameter),
            EdgeCurve::Parabola(curve) => curve.point_at(parameter),
            EdgeCurve::ArcOfParabola(curve) => curve.point_at(parameter),
            EdgeCurve::BezierCurve(curve) => curve.point_at(parameter),
            EdgeCurve::BSplineCurve(curve) => {
                curve.point_at(parameter).unwrap_or_else(Vector3::zero)
            }
            EdgeCurve::OffsetCurve(curve) => curve.point_at(parameter),
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
            EdgeCurve::EllipticalArc(curve) => curve.tangent_at(parameter),
            EdgeCurve::Hyperbola(curve) => curve.tangent_at(parameter),
            EdgeCurve::ArcOfHyperbola(curve) => curve.hyperbola().tangent_at(parameter),
            EdgeCurve::Parabola(curve) => curve.tangent_at(parameter),
            EdgeCurve::ArcOfParabola(curve) => curve.parabola().tangent_at(parameter),
            EdgeCurve::BezierCurve(curve) => curve.tangent_at(parameter),
            EdgeCurve::BSplineCurve(curve) => curve.tangent_at(parameter),
            EdgeCurve::OffsetCurve(curve) => curve.tangent_at(parameter),
        }
    }

    /// The length (`f64::INFINITY` for the unbounded kinds; the elliptical and
    /// spline values are sampled approximations).
    pub fn length(&self) -> f64 {
        match self {
            EdgeCurve::Line(_) | EdgeCurve::Hyperbola(_) | EdgeCurve::Parabola(_) => f64::INFINITY,
            EdgeCurve::LineSegment(curve) => curve.length(),
            EdgeCurve::Circle(curve) => curve.length(),
            EdgeCurve::Arc(curve) => curve.length(),
            EdgeCurve::Ellipse(curve) => curve.length(),
            EdgeCurve::EllipticalArc(curve) => curve.length(),
            EdgeCurve::ArcOfHyperbola(curve) => curve.length(),
            EdgeCurve::ArcOfParabola(curve) => curve.length(),
            EdgeCurve::BezierCurve(curve) => curve.length(),
            EdgeCurve::BSplineCurve(curve) => curve.length(curve.first(), curve.last()),
            EdgeCurve::OffsetCurve(curve) => curve.length(),
        }
    }

    /// The start and end points, or `None` for an unbounded curve.
    ///
    /// A circle or a full ellipse reports the same point twice.
    pub fn endpoints(&self) -> Option<(Vector3, Vector3)> {
        match self {
            EdgeCurve::Line(_) | EdgeCurve::Hyperbola(_) | EdgeCurve::Parabola(_) => None,
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
            EdgeCurve::EllipticalArc(curve) => Some((curve.start(), curve.end())),
            EdgeCurve::ArcOfHyperbola(curve) => Some((curve.start(), curve.end())),
            EdgeCurve::ArcOfParabola(curve) => Some((curve.start(), curve.end())),
            EdgeCurve::BezierCurve(curve) => Some((curve.start(), curve.end())),
            EdgeCurve::BSplineCurve(curve) => Some((
                curve.point_at(curve.first()).unwrap_or_else(Vector3::zero),
                curve.point_at(curve.last()).unwrap_or_else(Vector3::zero),
            )),
            EdgeCurve::OffsetCurve(curve) => Some((curve.start(), curve.end())),
        }
    }

    /// Whether the curve ends where it starts.
    pub fn is_closed(&self) -> bool {
        match self {
            EdgeCurve::Line(_)
            | EdgeCurve::LineSegment(_)
            | EdgeCurve::Hyperbola(_)
            | EdgeCurve::Parabola(_)
            | EdgeCurve::ArcOfHyperbola(_)
            | EdgeCurve::ArcOfParabola(_) => false,
            EdgeCurve::Circle(_) | EdgeCurve::Ellipse(_) => true,
            EdgeCurve::Arc(curve) => curve.is_closed(),
            EdgeCurve::EllipticalArc(curve) => curve.is_closed(),
            EdgeCurve::BezierCurve(curve) => curve.is_closed(),
            EdgeCurve::BSplineCurve(curve) => {
                let start = curve.point_at(curve.first());
                let end = curve.point_at(curve.last());
                match (start, end) {
                    (Some(start), Some(end)) => start.distance(&end) <= TOLERANCE,
                    _ => false,
                }
            }
            EdgeCurve::OffsetCurve(curve) => curve.is_closed(),
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
            EdgeCurve::EllipticalArc(curve) => EdgeCurve::EllipticalArc(curve.reversed()),
            EdgeCurve::Hyperbola(curve) => EdgeCurve::Hyperbola(*curve),
            EdgeCurve::ArcOfHyperbola(curve) => EdgeCurve::ArcOfHyperbola(curve.reversed()),
            EdgeCurve::Parabola(curve) => EdgeCurve::Parabola(*curve),
            EdgeCurve::ArcOfParabola(curve) => EdgeCurve::ArcOfParabola(curve.reversed()),
            EdgeCurve::BezierCurve(curve) => EdgeCurve::BezierCurve(curve.reversed()),
            EdgeCurve::BSplineCurve(curve) => EdgeCurve::BSplineCurve(curve.reversed()),
            EdgeCurve::OffsetCurve(curve) => EdgeCurve::OffsetCurve(curve.reversed()),
        }
    }

    /// The parameter range `[first, last]`, or `None` for an unbounded curve.
    ///
    /// A bounded line segment is `[0, 1]` and a full circle/ellipse is
    /// `[0, 2π]`; the arcs report their own (possibly reversed) range.
    pub fn parameter_range(&self) -> Option<(f64, f64)> {
        match self {
            EdgeCurve::Line(_) | EdgeCurve::Hyperbola(_) | EdgeCurve::Parabola(_) => None,
            EdgeCurve::LineSegment(_) => Some((0.0, 1.0)),
            EdgeCurve::Circle(_) | EdgeCurve::Ellipse(_) => Some((0.0, TAU)),
            EdgeCurve::Arc(curve) => Some((curve.first(), curve.last())),
            EdgeCurve::EllipticalArc(curve) => Some((curve.first(), curve.last())),
            EdgeCurve::ArcOfHyperbola(curve) => Some((curve.first(), curve.last())),
            EdgeCurve::ArcOfParabola(curve) => Some((curve.first(), curve.last())),
            EdgeCurve::BezierCurve(curve) => Some((curve.first(), curve.last())),
            EdgeCurve::BSplineCurve(curve) => Some((curve.first(), curve.last())),
            EdgeCurve::OffsetCurve(curve) => Some((curve.first(), curve.last())),
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
            EdgeCurve::EllipticalArc(_) => "ArcOfEllipse",
            EdgeCurve::Hyperbola(_) => "Hyperbola",
            EdgeCurve::ArcOfHyperbola(_) => "ArcOfHyperbola",
            EdgeCurve::Parabola(_) => "Parabola",
            EdgeCurve::ArcOfParabola(_) => "ArcOfParabola",
            EdgeCurve::BezierCurve(_) => "BezierCurve",
            EdgeCurve::BSplineCurve(_) => "BSplineCurve",
            EdgeCurve::OffsetCurve(_) => "OffsetCurve",
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
    fn an_elliptical_arc_keeps_its_bounds_and_reverses() {
        let ellipse = Ellipse::with_axis(
            Vector3::zero(),
            axis_z(),
            axis_x(),
            3.0,
            1.0,
        )
        .unwrap();
        let arc = ellipse.trim(0.5, 1.25).unwrap();
        assert_eq!(arc.first(), 0.5);
        assert_eq!(arc.last(), 1.25);
        assert!(close(arc.start(), ellipse.point_at(0.5)));
        assert!(close(arc.end(), ellipse.point_at(1.25)));
        let reversed = arc.reversed();
        assert!(close(reversed.start(), arc.end()));
        assert!(close(reversed.end(), arc.start()));
        assert!(arc.length() > 0.0);
    }

    #[test]
    fn a_conic_arc_is_a_bounded_edge_curve() {
        let hyperbola = Hyperbola::with_axis(
            Vector3::zero(),
            axis_z(),
            axis_x(),
            2.0,
            1.0,
        )
        .unwrap();
        let arc = EdgeCurve::ArcOfHyperbola(hyperbola.trim(-0.5, 0.75).unwrap());
        assert_eq!(arc.kind(), "ArcOfHyperbola");
        assert_eq!(arc.parameter_range(), Some((-0.5, 0.75)));
        assert!(arc.endpoints().is_some());
        assert!(!arc.is_closed());
        assert!(arc.length() > 0.0);
        let reversed = arc.reversed();
        assert!(reversed
            .endpoints()
            .unwrap()
            .0
            .distance(&arc.endpoints().unwrap().1)
            < TOL);

        let parabola = Parabola::with_axis(Vector3::zero(), axis_z(), axis_x(), 0.5).unwrap();
        let arc = EdgeCurve::ArcOfParabola(parabola.trim(-1.0, 2.0).unwrap());
        assert_eq!(arc.kind(), "ArcOfParabola");
        assert_eq!(arc.parameter_range(), Some((-1.0, 2.0)));
        assert!(arc.endpoints().is_some());

        // The whole conics are unbounded, like an infinite line.
        assert!(EdgeCurve::Hyperbola(hyperbola).endpoints().is_none());
        assert!(EdgeCurve::Parabola(parabola).endpoints().is_none());
    }

    #[test]
    fn a_bspline_reversal_mirrors_its_knots() {
        let spline = NurbsCurve::new(
            2,
            vec![0.0, 1.0],
            vec![3, 3],
            vec![
                Vector3::zero(),
                Vector3::new(1.0, 2.0, 0.0),
                Vector3::new(2.0, 0.0, 0.0),
            ],
            vec![1.0, 1.0, 1.0],
        )
        .unwrap();
        let reversed = spline.reversed();
        for parameter in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(close(
                reversed.point_at(parameter).unwrap(),
                spline.point_at(1.0 - parameter).unwrap()
            ));
        }
        assert!((reversed.tangent_at(0.0).dot(&spline.tangent_at(1.0)) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_offset_curve_follows_the_basis_normal() {
        let circle = Circle::new(Vector3::zero(), axis_z(), 2.0).unwrap();
        let basis = EdgeCurve::Circle(circle);
        let offset = OffsetCurve::new(
            basis.clone(),
            0.5,
            axis_z(),
            0.0,
            std::f64::consts::TAU,
        )
        .unwrap();
        assert_eq!(offset.basis().kind(), "Circle");
        assert!((offset.offset() - 0.5).abs() < TOL);
        // A +Z offset of a circle at +X moves outwards.
        assert!(close(offset.point_at(0.0), Vector3::new(2.5, 0.0, 0.0)));
        let edge = EdgeCurve::OffsetCurve(offset);
        assert_eq!(edge.kind(), "OffsetCurve");
        assert!(edge.parameter_range().is_some());
        assert!(edge.endpoints().is_some());

        // Reversing swaps the traversal bounds without moving the curve.
        let arc_offset = OffsetCurve::new(
            basis,
            0.5,
            axis_z(),
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .unwrap();
        let reversed = arc_offset.reversed();
        assert!(close(reversed.start(), arc_offset.end()));
        assert!(close(reversed.end(), arc_offset.start()));
        assert!((reversed.offset() - arc_offset.offset()).abs() < TOL);
    }

    /// The periodic cubic in OCCT's own form for the lateral face of a cylinder
    /// (`BRepBuilderAPI_NurbsConvert` on a full circle): a rational quadratic with
    /// six poles, four knots each doubled, and the period `2π`.
    fn periodic_circle() -> NurbsCurve {
        let axis = std::f64::consts::TAU / 3.0;
        NurbsCurve::new_periodic(
            2,
            vec![0.0, axis, 2.0 * axis, 3.0 * axis],
            vec![2, 2, 2, 2],
            vec![
                Vector3::new(5.0, 0.0, 10.0),
                Vector3::new(5.0, 8.660_254_037_844_387, 10.0),
                Vector3::new(-2.5, 4.330_127_018_922_193, 10.0),
                Vector3::new(-10.0, 0.0, 10.0),
                Vector3::new(-2.5, -4.330_127_018_922_193, 10.0),
                Vector3::new(5.0, -8.660_254_037_844_387, 10.0),
            ],
            vec![1.0, 0.5, 1.0, 0.5, 1.0, 0.5],
        )
        .unwrap()
    }

    #[test]
    fn a_periodic_bspline_wraps_its_knots_and_poles() {
        let spline = periodic_circle();
        assert!(spline.is_periodic());
        assert_eq!(spline.degree(), 2);
        assert_eq!(spline.poles().len(), 6);
        assert_eq!(spline.first(), 0.0);
        assert!((spline.last() - std::f64::consts::TAU).abs() < TOL);
        assert!(spline.is_rational());

        // The curve passes through the arc joints at the doubled knots.
        assert!(close(spline.point_at(0.0).unwrap(), Vector3::new(5.0, 0.0, 10.0)));
        assert!(close(
            spline.point_at(std::f64::consts::TAU / 3.0).unwrap(),
            Vector3::new(-2.5, 4.330_127_018_922_193, 10.0)
        ));
        // The period end is the start point, and parameters wrap.
        assert!(close(
            spline.point_at(std::f64::consts::TAU).unwrap(),
            spline.point_at(0.0).unwrap()
        ));
        assert!(close(
            spline.point_at(-std::f64::consts::FRAC_PI_3).unwrap(),
            spline.point_at(5.0 * std::f64::consts::FRAC_PI_3).unwrap()
        ));

        // Every point lies on the radius-5 circle in the z = 10 plane.
        for step in 0..24 {
            let parameter = std::f64::consts::TAU * step as f64 / 24.0;
            let point = spline.point_at(parameter).unwrap();
            let radius = (point.x * point.x + point.y * point.y).sqrt();
            assert!((radius - 5.0).abs() < 1e-9, "radius {radius} at {parameter}");
            assert!((point.z - 10.0).abs() < 1e-12);
        }
        assert!((spline.length(spline.first(), spline.last()) - 10.0 * std::f64::consts::PI).abs() < 1e-2);

        // Reversing keeps the periodic form and mirrors the parameterisation.
        let reversed = spline.reversed();
        assert!(reversed.is_periodic());
        for parameter in [0.0, 0.4, 1.7, 4.2, 6.1] {
            assert!(close(
                reversed.point_at(parameter).unwrap(),
                spline
                    .point_at(std::f64::consts::TAU - parameter)
                    .unwrap()
            ));
        }
    }

    #[test]
    fn the_clamped_and_periodic_forms_reject_each_other_s_knots() {
        let axis = std::f64::consts::TAU / 3.0;
        let poles = periodic_circle().poles().to_vec();
        let weights = periodic_circle().weights().to_vec();

        // The clamped relation is `sum(mults) = poles + degree + 1`.
        assert!(
            NurbsCurve::new(
                2,
                vec![0.0, axis, 2.0 * axis, 3.0 * axis],
                vec![2, 2, 2, 2],
                poles.clone(),
                weights.clone(),
            )
            .is_err()
        );

        // A periodic end multiplicity is at most the degree.
        assert!(
            NurbsCurve::new_periodic(
                2,
                vec![0.0, 1.0],
                vec![3, 3],
                vec![
                    Vector3::zero(),
                    Vector3::new(1.0, 1.0, 0.0),
                    Vector3::new(2.0, 0.0, 0.0),
                ],
                vec![1.0, 1.0, 1.0],
            )
            .is_err()
        );
    }
}
