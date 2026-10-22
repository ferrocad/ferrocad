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
}
