//! The OCCT shape algorithms Part exposes that are not part of the
//! [`GeometryBackend`](ferrocad_geom::GeometryBackend) seam: face domain tests,
//! face validation, projection, defeaturing, 3D offsetting, ordered wire
//! traversal and the B-spline surface data model.
//!
//! Everything here is a thin, safe wrapper over the sibling bridge
//! ([`crate::bridge`], `src/fc_part_ops.hxx`), in the same style as
//! [`crate::shapefix`] and [`crate::surface_ops`]. The seam stays
//! kernel-agnostic; these calls are OCCT-only operations a `dyn GeometryBackend`
//! cannot usefully abstract.
//!
//! Errors are [`GeomError`] (built from [`OcctError`]) so the Python layer maps
//! them to `Part.OCCError` exactly like the seam calls do.

use ferrocad_curves::Vector3;
use ferrocad_geom::{GeomError, Shape};
use opencascade_sys::top_tools::TopTools_ListOfShape;
use opencascade_sys::topo_ds::TopoDS_Shape;

use crate::bridge;
use crate::error::OcctError;
use crate::shape::downcast;

/// Build an OCCT list from kernel shapes.
fn shape_list(shapes: &[Shape]) -> Option<cxx::UniquePtr<TopTools_ListOfShape>> {
    let mut list = opencascade_sys::top_tools::new_list_of_shape();
    for shape in shapes {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        list.pin_mut().Append(guard.as_ref()?);
    }
    Some(list)
}

/// Take OCCT's output shape, or report the operation as not done.
fn result(shape: cxx::UniquePtr<TopoDS_Shape>, operation: &'static str) -> Result<Shape, GeomError> {
    if shape.is_null() {
        return Err(OcctError::NotDone { operation }.into());
    }
    Ok(Shape::new(crate::shape::OcctShape(shape)))
}

fn borrow<T>(
    shape: &Shape,
    f: impl FnOnce(&TopoDS_Shape) -> Result<T, OcctError>,
) -> Result<T, GeomError> {
    let occt = downcast(shape)?;
    let guard = occt.borrow();
    match guard.as_ref() {
        Some(inner) => f(inner).map_err(GeomError::from),
        None => Err(OcctError::InvalidInput { what: "null shape" }.into()),
    }
}

/// The exact topological kind of a shape: `"Compound"`, `"CompSolid"`,
/// `"Solid"`, `"Shell"`, `"Face"`, `"Wire"`, `"Edge"`, `"Vertex"` or
/// `"Shape"`, or `None` for a null shape.
///
/// This is `TopoDS_Shape::ShapeType`, not the sub-shape-count inference
/// `Shape.ShapeType` uses, so a compound holding one solid is still a compound.
pub fn shape_kind(shape: &Shape) -> Option<&'static str> {
    let occt = downcast(shape).ok()?;
    let guard = occt.borrow();
    let kind = bridge::fc_shape_kind(guard.as_ref()?);
    Some(match kind {
        0 => "Compound",
        1 => "CompSolid",
        2 => "Solid",
        3 => "Shell",
        4 => "Face",
        5 => "Wire",
        6 => "Edge",
        7 => "Vertex",
        _ => "Shape",
    })
}

/// `Face.isPartOfDomain(u, v)`: whether the parameter pair is inside the face.
pub fn face_is_part_of_domain(face: &Shape, u: f64, v: f64) -> Result<bool, GeomError> {
    borrow(face, |first| {
        Ok(bridge::fc_face_is_part_of_domain(first, u, v))
    })
}

/// `Face.validate()`: repair an invalid face. The returned shape is the face to
/// store (the input when it was already valid).
pub fn validate_face(face: &Shape) -> Result<Shape, GeomError> {
    borrow(face, |first| Ok(bridge::fc_validate_face(first)))
        .and_then(|output| result(output, "validate"))
}

/// `Shape.fix(precision, mintol, maxtol)`: the repaired shape
/// (`ShapeFix_Shape`, upstream's per-kind tool walk).
pub fn fix_shape(
    shape: &Shape,
    precision: f64,
    min_tolerance: f64,
    max_tolerance: f64,
) -> Result<Shape, GeomError> {
    borrow(shape, |first| {
        Ok(bridge::fc_fix_shape(
            first,
            precision,
            min_tolerance,
            max_tolerance,
        ))
    })
    .and_then(|output| result(output, "fix"))
}

/// The `(u, v)` of `point` on a face's surface (`GeomSurface.parameter`).
pub fn face_parameter(face: &Shape, point: Vector3) -> Result<(f64, f64), GeomError> {
    borrow(face, |first| {
        let (mut u, mut v) = (0.0, 0.0);
        if bridge::fc_face_parameter(first, point.x, point.y, point.z, &mut u, &mut v) {
            Ok((u, v))
        } else {
            Err(OcctError::NotDone {
                operation: "face parameter",
            })
        }
    })
}

/// Project `tool` onto `target` along `direction` (`parallel`) or from the
/// viewpoint `direction`.
pub fn project(
    tool: &Shape,
    target: &Shape,
    direction: Vector3,
    parallel: bool,
) -> Result<Shape, GeomError> {
    let tool_occt = downcast(tool)?;
    let target_occt = downcast(target)?;
    let tool_guard = tool_occt.borrow();
    let target_guard = target_occt.borrow();
    let first = tool_guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;
    let second = target_guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;
    let output = bridge::fc_project_shape(
        first,
        second,
        direction.x,
        direction.y,
        direction.z,
        parallel,
    );
    result(
        output,
        if parallel {
            "makeParallelProjection"
        } else {
            "makePerspectiveProjection"
        },
    )
}

/// `Shape.defeaturing(faces)`: remove `faces` and heal the gap.
pub fn defeature(shape: &Shape, faces: &[Shape]) -> Result<Shape, GeomError> {
    let faces = shape_list(faces)
        .ok_or_else(|| GeomError::from(OcctError::InvalidInput { what: "defeaturing face" }))?;
    borrow(shape, |first| Ok(bridge::fc_defeature(first, &faces)))
        .and_then(|output| result(output, "defeaturing"))
}

/// `Shape.makeOffsetShape(offset, tolerance, …)`: a 3D offset (`fill` closes a
/// shell with walls between the input boundary and its offset).
#[allow(clippy::too_many_arguments)]
pub fn offset_shape(
    shape: &Shape,
    offset: f64,
    tolerance: f64,
    inter: bool,
    self_inter: bool,
    offset_mode: i32,
    join: i32,
    fill: bool,
) -> Result<Shape, GeomError> {
    borrow(shape, |first| {
        Ok(bridge::fc_offset_shape(
            first,
            offset,
            tolerance,
            inter,
            self_inter,
            offset_mode,
            join,
            fill,
        ))
    })
    .and_then(|output| result(output, "makeOffsetShape"))
}

/// `Wire.OrderedEdges`: the wire's edges in connection order.
pub fn wire_ordered_edges(wire: &Shape) -> Result<Vec<Shape>, GeomError> {
    let compound = borrow(wire, |first| Ok(bridge::fc_wire_ordered_edges(first)))
        .and_then(|output| result(output, "OrderedEdges"))?;
    Ok(child_shapes(&compound))
}

/// The direct children of a shape (the compound `wire_ordered_edges` returns).
fn child_shapes(compound: &Shape) -> Vec<Shape> {
    borrow(compound, |inner| {
        let count = bridge::fc_shape_child_count(inner);
        let mut children = Vec::with_capacity(count.max(0) as usize);
        for index in 1..=count {
            let child = bridge::fc_shape_child(inner, index);
            if !child.is_null() {
                children.push(Shape::new(crate::shape::OcctShape(child)));
            }
        }
        Ok(children)
    })
    .unwrap_or_default()
}

/// A B-spline surface, described by everything Part's surface API reads.
///
/// This is the neutral shape of the layout in `fc_part_ops.hxx`; the Python
/// `Part.BSplineSurface` holds one and the bridge rebuilds an OCCT
/// `Geom_BSplineSurface` from it on demand.
#[derive(Clone, Debug, PartialEq)]
pub struct BsplineSurface {
    pub u_degree: i32,
    pub v_degree: i32,
    pub u_periodic: bool,
    pub v_periodic: bool,
    /// `(first, last)` parameter bounds.
    pub u_bounds: (f64, f64),
    pub v_bounds: (f64, f64),
    /// Poles indexed `[u][v]`, each with its rational weight.
    pub poles: Vec<Vec<(Vector3, f64)>>,
    /// Unique knots with their multiplicities, in increasing order.
    pub u_knots: Vec<(f64, i32)>,
    pub v_knots: Vec<(f64, i32)>,
}

impl BsplineSurface {
    /// The flat layout the bridge reads (see `fc_part_ops.hxx`).
    fn to_flat(&self) -> Vec<f64> {
        let mut out = vec![
            f64::from(self.u_degree),
            f64::from(self.v_degree),
            if self.u_periodic { 1.0 } else { 0.0 },
            if self.v_periodic { 1.0 } else { 0.0 },
            self.u_bounds.0,
            self.u_bounds.1,
            self.v_bounds.0,
            self.v_bounds.1,
            self.poles.len() as f64,
            self.poles.first().map_or(0.0, |row| row.len() as f64),
        ];
        for row in &self.poles {
            for (point, weight) in row {
                out.extend_from_slice(&[point.x, point.y, point.z, *weight]);
            }
        }
        out.push(self.u_knots.len() as f64);
        for (knot, multiplicity) in &self.u_knots {
            out.extend_from_slice(&[*knot, f64::from(*multiplicity)]);
        }
        out.push(self.v_knots.len() as f64);
        for (knot, multiplicity) in &self.v_knots {
            out.extend_from_slice(&[*knot, f64::from(*multiplicity)]);
        }
        out
    }

    /// Read the flat layout back into a description.
    fn from_flat(data: &[f64]) -> Option<Self> {
        if data.len() < 10 {
            return None;
        }
        let u_degree = data[0] as i32;
        let v_degree = data[1] as i32;
        let u_periodic = data[2] != 0.0;
        let v_periodic = data[3] != 0.0;
        let u_bounds = (data[4], data[5]);
        let v_bounds = (data[6], data[7]);
        let nu = data[8] as usize;
        let nv = data[9] as usize;
        if nu < 2 || nv < 2 {
            return None;
        }
        let mut at = 10;
        let mut poles = Vec::with_capacity(nu);
        for _ in 0..nu {
            let mut row = Vec::with_capacity(nv);
            for _ in 0..nv {
                let (x, y, z, weight) = (
                    *data.get(at)?,
                    *data.get(at + 1)?,
                    *data.get(at + 2)?,
                    *data.get(at + 3)?,
                );
                row.push((Vector3::new(x, y, z), weight));
                at += 4;
            }
            poles.push(row);
        }
        let u_knots = read_knots(data, &mut at)?;
        let v_knots = read_knots(data, &mut at)?;
        Some(BsplineSurface {
            u_degree,
            v_degree,
            u_periodic,
            v_periodic,
            u_bounds,
            v_bounds,
            poles,
            u_knots,
            v_knots,
        })
    }
}

fn read_knots(data: &[f64], at: &mut usize) -> Option<Vec<(f64, i32)>> {
    let count = *data.get(*at)? as usize;
    *at += 1;
    let mut knots = Vec::with_capacity(count);
    for _ in 0..count {
        knots.push((*data.get(*at)?, *data.get(*at + 1)? as i32));
        *at += 2;
    }
    Some(knots)
}

/// The B-spline surface behind a face, or `None` for any other surface kind.
pub fn bspline_surface_data(face: &Shape) -> Option<BsplineSurface> {
    let raw = borrow(face, |inner| Ok(bridge::fc_bspline_surface_data(inner))).ok()?;
    if raw.is_empty() {
        None
    } else {
        BsplineSurface::from_flat(&raw)
    }
}

/// The face of a B-spline surface over its whole parameter domain.
pub fn bspline_surface_face(surface: &BsplineSurface) -> Result<Shape, GeomError> {
    result(
        bridge::fc_bspline_surface_face_from_data(&surface.to_flat()),
        "BSplineSurface.toShape",
    )
}

/// `BSplineSurface.value(u, v)`.
pub fn bspline_surface_value(
    surface: &BsplineSurface,
    u: f64,
    v: f64,
) -> Result<Vector3, GeomError> {
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    if bridge::fc_bspline_surface_value(&surface.to_flat(), u, v, &mut x, &mut y, &mut z) {
        Ok(Vector3::new(x, y, z))
    } else {
        Err(OcctError::NotDone {
            operation: "BSplineSurface.value",
        }
        .into())
    }
}

/// `BSplineSurface.parameter(point)`.
pub fn bspline_surface_parameter(
    surface: &BsplineSurface,
    point: Vector3,
) -> Result<(f64, f64), GeomError> {
    let (mut u, mut v) = (0.0, 0.0);
    if bridge::fc_bspline_surface_parameter(
        &surface.to_flat(),
        point.x,
        point.y,
        point.z,
        &mut u,
        &mut v,
    ) {
        Ok((u, v))
    } else {
        Err(OcctError::NotDone {
            operation: "BSplineSurface.parameter",
        }
        .into())
    }
}

/// `Part.Face(bspline_surface, wires)`: trim the surface with boundary wires.
pub fn face_on_bspline_surface(
    surface: &BsplineSurface,
    wires: &[Shape],
) -> Result<Shape, GeomError> {
    let wires = shape_list(wires)
        .ok_or_else(|| GeomError::from(OcctError::InvalidInput { what: "boundary wire" }))?;
    result(
        bridge::fc_face_on_bspline_surface(&surface.to_flat(), &wires),
        "makeFaceOnBSplineSurface",
    )
}
