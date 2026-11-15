//! The bridge between a sketch document object and `ferrocad_sketch_solver`.
//!
//! The document stores geometry as kernel shapes (`Geometry`, a
//! `Part::PropertyTopoShapeList`) and constraints as records naming stable ids
//! (`ConstraintData`); the solver works on a 2-D parameter vector and indices. This
//! module is the only place that knows both, and it holds the two conversions in
//! each direction:
//!
//! * a stored edge becomes a [`Curve2d`] — a line segment, a circle or an arc — or
//!   [`Curve2d::Other`] when the solver does not parameterize it (an ellipse, a
//!   spline), which keeps the geometry and refuses it as a constraint target;
//! * a stored constraint record becomes a solver [`Constraint`], with the stable
//!   id resolved to a geometry index and a document position mapped to a
//!   [`Pos`](ferrocad_sketch_solver::Pos).
//!
//! Everything is solved in the sketch's local XY plane, which is the plane
//! upstream's solver works in and the one `Placement` moves afterwards.

use ferrocad_curves::{Arc as CurveArc, Circle, EdgeCurve, LineSegment, Vector2, Vector3};
use ferrocad_geom::Shape;
use ferrocad_sketch_solver::{Constraint, Curve2d, GeoRef as SolverRef, Kind, Pos, Reference};

use crate::solids::{
    GeoRef as StoredRef, SketchConstraint, POINT_POS_END, POINT_POS_MID, POINT_POS_START,
};

fn flatten(point: Vector3) -> Vector2 {
    Vector2::new(point.x, point.y)
}

fn lift(point: Vector2) -> Vector3 {
    Vector3::new(point.x, point.y, 0.0)
}

/// The solver's view of one stored geometry edge.
///
/// An edge reports its *basis* curve (`Edge.Curve` is the infinite line or the full
/// circle), so the edge's own parameter range is what turns the basis into the
/// segment or the arc the sketch stores. A circular edge whose range is a whole
/// turn is a circle; anything shorter is an arc.
pub(crate) fn curve_of(shape: &Shape) -> Curve2d {
    let backend = ferrocad_part::backend();
    let range = || backend.edge_parameter_range(shape).unwrap_or((0.0, 0.0));
    match backend.curve_of(shape) {
        Ok(EdgeCurve::Line(line)) => {
            let (first, last) = range();
            Curve2d::Line {
                start: flatten(line.point_at(first)),
                end: flatten(line.point_at(last)),
            }
        }
        Ok(EdgeCurve::LineSegment(segment)) => Curve2d::Line {
            start: flatten(segment.start()),
            end: flatten(segment.end()),
        },
        Ok(EdgeCurve::Circle(circle)) => {
            let (first, last) = range();
            if (last - first).abs() >= std::f64::consts::TAU - 1e-9 {
                Curve2d::Circle {
                    center: flatten(circle.center()),
                    radius: circle.radius(),
                }
            } else {
                Curve2d::Arc {
                    center: flatten(circle.center()),
                    radius: circle.radius(),
                    first,
                    last,
                }
            }
        }
        Ok(EdgeCurve::Arc(arc)) => Curve2d::Arc {
            center: flatten(arc.circle().center()),
            radius: arc.circle().radius(),
            first: arc.first(),
            last: arc.last(),
        },
        // A `Part.Vertex` is a point geometry in a sketch (`Part.Point.toShape()`).
        _ => match backend.vertex_point(shape) {
            Some(point) => Curve2d::Point(flatten(point)),
            None => Curve2d::Other,
        },
    }
}

/// The stored edge a solved curve becomes; `None` for [`Curve2d::Other`], whose
/// geometry the solve never touched.
pub(crate) fn shape_of(curve: &Curve2d) -> Option<Shape> {
    let backend = ferrocad_part::backend();
    let edge = match *curve {
        Curve2d::Point(point) => return backend.make_vertex(lift(point)).ok(),
        Curve2d::Line { start, end } => {
            EdgeCurve::LineSegment(LineSegment::new(lift(start), lift(end)).ok()?)
        }
        Curve2d::Circle { center, radius } => {
            let normal = Vector3::new(0.0, 0.0, 1.0);
            EdgeCurve::Circle(Circle::new(lift(center), normal, radius).ok()?)
        }
        Curve2d::Arc {
            center,
            radius,
            first,
            last,
        } => {
            let normal = Vector3::new(0.0, 0.0, 1.0);
            let circle = Circle::new(lift(center), normal, radius).ok()?;
            EdgeCurve::Arc(CurveArc::new(circle, first, last).ok()?)
        }
        Curve2d::Other => return None,
    };
    backend.make_edge(&edge).ok()
}

/// The document's stored geometry, as the solver sees it.
pub(crate) fn curves_of(shapes: &[Shape]) -> Vec<Curve2d> {
    shapes.iter().map(curve_of).collect()
}

/// Rebuild the geometry list from the solved curves, keeping every element the
/// solver did not parameterize exactly as it was.
pub(crate) fn shapes_of(solved: &[Curve2d], stored: &[Shape]) -> Vec<Shape> {
    solved
        .iter()
        .zip(stored)
        .map(|(curve, base)| shape_of(curve).unwrap_or_else(|| base.clone()))
        .collect()
}

fn position(code: i64) -> Pos {
    match code {
        POINT_POS_START => Pos::Start,
        POINT_POS_END => Pos::End,
        POINT_POS_MID => Pos::Mid,
        _ => Pos::None,
    }
}

/// Resolve a stored reference to a solver reference.
fn reference(stored: &StoredRef, ids: &[String]) -> Result<Reference, String> {
    match stored {
        StoredRef::Id(id) => ids
            .iter()
            .position(|have| have == id)
            .map(Reference::Geometry)
            .ok_or_else(|| format!("no geometry has the id {id:?}")),
        StoredRef::Position(at) if *at < ids.len() => Ok(Reference::Geometry(*at)),
        StoredRef::Position(at) => Err(format!(
            "a constraint references position {at}, but the sketch has {n} geometry element(s)",
            n = ids.len()
        )),
        StoredRef::Root(-1) => Ok(Reference::RootXAxis),
        StoredRef::Root(-2) => Ok(Reference::RootYAxis),
        StoredRef::Root(axis) => Err(format!(
            "a constraint references {axis}; only the root axes (-1 X, -2 Y) are supported"
        )),
    }
}

fn geo_ref(stored: &StoredRef, pos: i64, ids: &[String]) -> Result<SolverRef, String> {
    Ok(SolverRef {
        reference: reference(stored, ids)?,
        pos: position(pos),
    })
}

/// The solver constraint a stored record says.
pub(crate) fn constraint_of(
    stored: &SketchConstraint,
    ids: &[String],
) -> Result<Constraint, String> {
    let first = geo_ref(&stored.first, stored.first_pos, ids)?;
    let second = match &stored.second {
        Some(reference) => Some(geo_ref(reference, stored.second_pos, ids)?),
        None => None,
    };
    let third = match &stored.third {
        Some(reference) => Some(geo_ref(reference, stored.third_pos, ids)?),
        None => None,
    };
    let kind = match stored.kind.as_str() {
        "Coincident" => Kind::Coincident,
        "PointOnObject" => Kind::PointOnObject,
        "Horizontal" => Kind::Horizontal,
        "Vertical" => Kind::Vertical,
        "Parallel" => Kind::Parallel,
        "Perpendicular" => Kind::Perpendicular,
        "Tangent" => Kind::Tangent,
        "Equal" => Kind::Equal,
        "Symmetric" => Kind::Symmetric,
        "Block" => Kind::Block,
        "Distance" => Kind::Distance(stored.value),
        "DistanceX" => Kind::DistanceX(stored.value),
        "DistanceY" => Kind::DistanceY(stored.value),
        "Radius" => Kind::Radius(stored.value),
        "Diameter" => Kind::Diameter(stored.value),
        "Angle" => Kind::Angle(stored.value),
        other => return Err(format!("unsupported sketch constraint type {other:?}")),
    };
    Ok(Constraint {
        kind,
        first,
        second,
        third,
    })
}

/// Every stored record, as solver constraints.
pub(crate) fn constraints_of(
    stored: &[SketchConstraint],
    ids: &[String],
) -> Result<Vec<Constraint>, String> {
    stored.iter().map(|record| constraint_of(record, ids)).collect()
}
