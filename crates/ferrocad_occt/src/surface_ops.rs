//! The OCCT surface and solid operations behind Part's remaining seam.
//!
//! These used to be a cxx bridge in `ferrocad_part_cxx::surface_ops`; they are
//! now plain Rust over the generic `opencascade-sys` bindings. Partial spheres,
//! variable-radius fillets, filled planar offsets, multi-shell solids, helices,
//! pipe shells and the two-profile thread loft are all expressed here. The fill
//! policy for `offset2d` (the band-nesting walk that mirrors FreeCAD's
//! `Shape.makeOffset2D`) lives in [`crate::policy::offset2d`].
//!
//! Every public function returns `Result<Shape, GeomError>`, the seam's error
//! type: a null result becomes `Part.OCCError` (upstream's `Base.CADKernelError`)
//! in Python, and arguments that cannot describe the operation are rejected here
//! before OCCT sees them.
//!
//! # Exactness
//!
//! Partial spheres, variable fillets, offsets, hollow solids, pipe sweeps and
//! the thread loft are exact OCCT constructions. A circular helix is
//! transcendental and has no polynomial OCCT curve: [`make_helix_edge`] builds
//! the same object upstream does, an edge whose **pcurve is exact** on an
//! analytic cylinder/cone, with `BRepLib::BuildCurves3d` producing the 3D
//! representative to its tolerance. [`make_thread`] similarly lofts two exact
//! 2D profiles on analytic cylinders.

use cxx::UniquePtr;
use ferrocad_curves::Vector3;
use ferrocad_geom::{GeomError, Shape};
use opencascade_sys as occt;

use occt::top_abs::TopAbs_Orientation;
use occt::top_abs::TopAbs_ShapeEnum;
use occt::top_tools::TopTools_ListOfShape;
use occt::topo_ds::{TopoDS, TopoDS_Shape};

use crate::error::OcctError;
use crate::shape::{downcast, erase};

/// Wrap an OCCT shape, reporting a null result as `NotDone`.
fn wrap(shape: &TopoDS_Shape, operation: &'static str) -> Result<Shape, GeomError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone { operation }.into());
    }
    Ok(erase(shape))
}

/// The shapes as an OCCT list, rejecting backends other than this one and null
/// handles.
fn shape_list(shapes: &[Shape], what: &'static str) -> Result<UniquePtr<TopTools_ListOfShape>, GeomError> {
    let mut list = occt::top_tools::new_list_of_shape();
    for shape in shapes {
        let occt_shape = downcast(shape)?;
        let guard = occt_shape.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what })?;
        list.pin_mut().Append(inner);
    }
    Ok(list)
}

fn finite(value: f64, what: &'static str) -> Result<(), GeomError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(OcctError::InvalidInput { what }.into())
    }
}

fn radians(degrees: f64) -> f64 {
    degrees * std::f64::consts::PI / 180.0
}

fn not_done(operation: &'static str) -> GeomError {
    OcctError::NotDone { operation }.into()
}

/// The point and unit tangent at the start of a spine edge or wire.
///
/// The OCCT adaptors evaluate every bounded edge, including a B-spline, so this
/// is the frame query the analytic `curve_of` seam does not expose. A spine that
/// is neither an edge nor a wire, or whose start tangent is degenerate, is
/// reported as unsupported by this operation.
pub fn spine_start_frame(spine: &Shape) -> Result<(Vector3, Vector3), GeomError> {
    let occt_spine = downcast(spine)?;
    let guard = occt_spine.borrow();
    let inner = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null spine" })?;

    let mut point = occt::gp::new_point(0.0, 0.0, 0.0);
    let mut tangent = occt::gp::new_vec(0.0, 0.0, 0.0);
    match inner.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_EDGE => {
            let curve = occt::b_rep_adaptor::BRepAdaptor_Curve_new(TopoDS::Edge(inner));
            let first = curve.FirstParameter();
            occt::b_rep_adaptor::BRepAdaptor_Curve_d1(
                &curve,
                first,
                point.pin_mut(),
                tangent.pin_mut(),
            );
        }
        TopAbs_ShapeEnum::TopAbs_WIRE => {
            let curve = occt::b_rep_adaptor::BRepAdaptor_CompCurve_new(TopoDS::Wire(inner));
            let first = curve.FirstParameter();
            occt::b_rep_adaptor::BRepAdaptor_CompCurve_d1(
                &curve,
                first,
                point.pin_mut(),
                tangent.pin_mut(),
            );
        }
        _ => {
            return Err(OcctError::NotDone {
                operation: "spine_start_frame",
            }
            .into());
        }
    }

    if tangent.Magnitude() <= occt::precision::Precision::Confusion() {
        return Err(OcctError::NotDone {
            operation: "spine_start_frame",
        }
        .into());
    }
    tangent.pin_mut().Normalize();
    Ok((
        Vector3::new(point.X(), point.Y(), point.Z()),
        Vector3::new(tangent.X(), tangent.Y(), tangent.Z()),
    ))
}

/// The direction of a straight edge, from its parametric start to its end and
/// swapped for a reversed edge (upstream's `Extrusion::fetchAxisLink`).
///
/// A shape that is not an edge, or whose curve is not a line, is reported as
/// not done; the caller distinguishes the two cases before calling.
pub fn edge_line_direction(edge: &Shape) -> Result<Vector3, GeomError> {
    let occt_edge = downcast(edge)?;
    let guard = occt_edge.borrow();
    let inner = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null edge" })?;
    if inner.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE {
        return Err(OcctError::NotDone {
            operation: "edge_line_direction",
        }
        .into());
    }

    let curve = occt::b_rep_adaptor::BRepAdaptor_Curve_new(TopoDS::Edge(inner));
    if curve.GetType() != occt::geom_abs::GeomAbs_CurveType::GeomAbs_Line {
        return Err(OcctError::NotDone {
            operation: "edge_line_direction",
        }
        .into());
    }
    let start = occt::b_rep_adaptor::BRepAdaptor_Curve_value(&curve, curve.FirstParameter());
    let end = occt::b_rep_adaptor::BRepAdaptor_Curve_value(&curve, curve.LastParameter());
    let (start, end) = if matches!(
        inner.Orientation(),
        TopAbs_Orientation::TopAbs_REVERSED
    ) {
        (end, start)
    } else {
        (start, end)
    };
    let direction = Vector3::new(end.X() - start.X(), end.Y() - start.Y(), end.Z() - start.Z());
    if direction.length() <= occt::precision::Precision::Confusion() {
        return Err(OcctError::NotDone {
            operation: "edge_line_direction",
        }
        .into());
    }
    Ok(direction)
}

/// The normal of the plane a shape lies on (upstream's
/// `Extrusion::calculateShapeNormal`).
pub fn shape_plane_normal(shape: &Shape) -> Result<Vector3, GeomError> {
    let occt_shape = downcast(shape)?;
    let guard = occt_shape.borrow();
    let inner = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;

    let finder = occt::b_rep_lib::BRepLib_FindSurface_new(inner, -1.0, true);
    if !finder.Found() {
        return Err(OcctError::NotDone {
            operation: "shape_plane_normal",
        }
        .into());
    }
    let surface = occt::b_rep_lib::BRepLib_FindSurface_Surface(&finder);
    let adaptor =
        occt::geom_adaptor::GeomAdaptor_Surface_new(&surface).map_err(|_| not_done("shape_plane_normal"))?;
    let plane = occt::geom_adaptor::GeomAdaptor_Surface_Plane(&adaptor)
        .map_err(|_| not_done("shape_plane_normal"))?;
    let axis = occt::gp::gp_Pln_Axis(&plane);
    let mut normal = occt::gp::gp_Ax1_Direction(&axis);

    let explorer = occt::top_exp::TopExp_Explorer_new(inner, TopAbs_ShapeEnum::TopAbs_FACE);
    if explorer.More() {
        let face_surface = occt::b_rep_adaptor::BRepAdaptor_Surface_new(TopoDS::Face(explorer.Current()));
        let face_plane = occt::b_rep_adaptor::BRepAdaptor_Surface_Plane(&face_surface)
            .map_err(|_| not_done("shape_plane_normal"))?;
        let face_axis = occt::gp::gp_Pln_Axis(&face_plane);
        normal = occt::gp::gp_Ax1_Direction(&face_axis);
        if matches!(
            explorer.Current().Orientation(),
            TopAbs_Orientation::TopAbs_REVERSED
        ) {
            normal = occt::gp::gp_Dir_Reversed(&normal);
        }
    }

    Ok(Vector3::new(normal.X(), normal.Y(), normal.Z()))
}

/// A cylinder, or a slice of one, of `radius` and `height`, based at `base`
/// with its axis along `direction`. `angle` is the angular sweep in radians
/// (`2π` is the full cylinder).
///
/// This maps onto upstream's
/// `BRepPrimAPI_MakeCylinder(gp_Ax2, radius, height, angle)`.
pub fn make_cylinder_slice(
    radius: f64,
    height: f64,
    base: Vector3,
    direction: Vector3,
    angle: f64,
) -> Result<Shape, GeomError> {
    finite(radius, "cylinder radius")?;
    finite(height, "cylinder height")?;
    finite(angle, "cylinder angle")?;
    let length = direction.length();
    if !length.is_finite() || length <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "cylinder direction",
        }
        .into());
    }
    let direction = direction.scale(1.0 / length);
    let origin = occt::gp::new_point(base.x, base.y, base.z);
    let axis_dir = occt::gp::gp_Dir_new(direction.x, direction.y, direction.z);
    let axis = occt::gp::gp_Ax2_new(&origin, &axis_dir);
    let mut make = occt::b_rep_prim_api::BRepPrimAPI_MakeCylinder_angle(&axis, radius, height, angle)
        .map_err(|_| not_done("makeCylinder"))?;
    wrap(make.pin_mut().Shape(), "makeCylinder")
}

/// A sphere, or a slice of one, of `radius` centred at `center`.
///
/// `u1`/`u2` are the longitude range (degrees, around the local +Z axis,
/// measured from the local +X direction) and `v1`/`v2` the latitude range
/// (degrees from the equator: `-90` is the south pole, `+90` the north pole).
/// The full sphere is `(0, 360, -90, 90)`.
///
/// This maps onto `BRepPrimAPI_MakeSphere(axes, R, v1, v2, u_span)`, whose
/// longitude sweep always starts at `u = 0`; the slice is rotated by `u1`
/// about the same axis afterwards, which is exact.
pub fn make_sphere_slice(
    radius: f64,
    center: Vector3,
    u1: f64,
    u2: f64,
    v1: f64,
    v2: f64,
) -> Result<Shape, GeomError> {
    finite(radius, "the sphere radius must be finite")?;
    if radius <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the sphere radius must be positive",
        }
        .into());
    }
    for (angle, what) in [
        (u1, "the first longitude must be finite"),
        (u2, "the second longitude must be finite"),
        (v1, "the first latitude must be finite"),
        (v2, "the second latitude must be finite"),
    ] {
        finite(angle, what)?;
    }
    if u2 - u1 <= 0.0 || u2 - u1 > 360.0 + 1e-7 {
        return Err(OcctError::InvalidInput {
            what: "the longitude range must be positive and at most 360 degrees",
        }
        .into());
    }
    if v2 - v1 <= 0.0 || v1 < -90.0 - 1e-7 || v2 > 90.0 + 1e-7 {
        return Err(OcctError::InvalidInput {
            what: "the latitude range must be positive and within -90..90 degrees",
        }
        .into());
    }

    let origin = occt::gp::new_point(center.x, center.y, center.z);
    let z = occt::gp::gp_Dir_new(0.0, 0.0, 1.0);
    let axis = occt::gp::gp_Ax2_new(&origin, &z);
    let mut make = occt::b_rep_prim_api::BRepPrimAPI_MakeSphere_full(
        &axis,
        radius,
        radians(v1),
        radians(v2),
        radians(u2 - u1),
    )
    .map_err(|_| not_done("make_sphere_slice"))?;
    let mut shape = occt::topo_ds::TopoDS_Shape_to_owned(make.pin_mut().Shape());
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "make_sphere_slice",
        }
        .into());
    }

    if u1.abs() > 1e-12 {
        let axis1 = occt::gp::gp_Ax1_new(&origin, &z);
        let mut rotation = occt::gp::new_transform();
        rotation.pin_mut().SetRotation(&axis1, radians(u1));
        let mut transform =
            occt::b_rep_builder_api::BRepBuilderAPI_Transform_new(&shape, &rotation, true);
        if !transform.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_sphere_slice",
            }
            .into());
        }
        shape = occt::topo_ds::TopoDS_Shape_to_owned(transform.pin_mut().Shape());
    }
    wrap(&shape, "make_sphere_slice")
}

/// Round `edges` of `shape` with a linear radius law from `start_radii[i]` to
/// `end_radii[i]` along each edge (`BRepFilletAPI_MakeFillet::Add(R1, R2, E)`).
///
/// The three slices must be the same length; every radius must be positive.
pub fn fillet_variable(
    shape: &Shape,
    edges: &[Shape],
    start_radii: &[f64],
    end_radii: &[f64],
) -> Result<Shape, GeomError> {
    if edges.is_empty() {
        return Err(OcctError::InvalidInput {
            what: "a variable fillet needs at least one edge",
        }
        .into());
    }
    if edges.len() != start_radii.len() || edges.len() != end_radii.len() {
        return Err(OcctError::InvalidInput {
            what: "the edge and radius lists must have the same length",
        }
        .into());
    }
    for (&start, &end) in start_radii.iter().zip(end_radii) {
        finite(start, "a fillet radius must be finite")?;
        finite(end, "a fillet radius must be finite")?;
        if start <= 0.0 || end <= 0.0 {
            return Err(OcctError::InvalidInput {
                what: "a fillet radius must be positive",
            }
            .into());
        }
    }
    let occt_shape = downcast(shape)?;
    let guard = occt_shape.borrow();
    let inner = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;
    let list = shape_list(edges, "a fillet edge must be an OCCT edge")?;

    let mut make = occt::b_rep_fillet_api::BRepFilletAPI_MakeFillet_new(inner);
    let mut iterator = occt::top_tools::TopTools_ListIteratorOfListOfShape_new(&list);
    let mut index = 0usize;
    while iterator.More() {
        let edge = iterator.Value();
        if edge.IsNull() || edge.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE {
            return Err(OcctError::InvalidInput {
                what: "a fillet edge must be an OCCT edge",
            }
            .into());
        }
        let start = start_radii[index];
        let end = end_radii[index];
        let edge = TopoDS::Edge(edge);
        if (start - end).abs() <= occt::precision::Precision::Confusion() {
            make.pin_mut().add_edge(start, edge);
        } else {
            make.pin_mut().add_edge_two_radius(start, end, edge);
        }
        iterator.pin_mut().Next();
        index += 1;
    }
    if let Err(e) = occt::b_rep_fillet_api::BRepFilletAPI_MakeFillet_build(make.pin_mut()) {
        return Err(OcctError::Exception {
            operation: "fillet_variable",
            message: e.what().to_string(),
        }
        .into());
    }
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "fillet_variable",
        }
        .into());
    }
    wrap(make.pin_mut().Shape(), "fillet_variable")
}

/// Offset `shape` by `distance` in its plane (`Shape.makeOffset2D`).
///
/// `kind` is upstream's `JoinType` (0 = arc, 1 = tangent, 2 = intersection).
/// With `fill`, the result is the face **between** the source and its offset
/// (the upstream meaning), not just the offset boundary: closed wires become
/// bands by nesting, and an open wire is closed with straight connectors when
/// `open_result` is set. Without `fill`, a face input is rebuilt from the offset
/// wires and a wire/edge input returns the offset boundary.
pub fn offset2d(
    shape: &Shape,
    distance: f64,
    kind: i32,
    fill: bool,
    open_result: bool,
) -> Result<Shape, GeomError> {
    finite(distance, "the offset distance must be finite")?;
    if fill && distance.abs() <= 1e-7 {
        return Err(OcctError::InvalidInput {
            what: "a filled offset cannot have a zero distance",
        }
        .into());
    }
    if !(0..=2).contains(&kind) {
        return Err(OcctError::InvalidInput {
            what: "the join kind must be 0 (arc), 1 (tangent) or 2 (intersection)",
        }
        .into());
    }
    let occt_shape = downcast(shape)?;
    let guard = occt_shape.borrow();
    let inner = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null shape" })?;
    let offset = crate::policy::offset2d(inner, distance, kind, fill, open_result)?;
    wrap(&offset, "offset2d")
}

/// A solid from `shells`: the first shell is the outer skin and every later
/// shell is a cavity, as `BRepBuilderAPI_MakeSolid` + `OrientClosedSolid`.
pub fn make_hollow_solid(shells: &[Shape]) -> Result<Shape, GeomError> {
    if shells.is_empty() {
        return Err(OcctError::InvalidInput {
            what: "a solid needs at least one shell",
        }
        .into());
    }
    let list = shape_list(shells, "a solid shell must be an OCCT shell")?;
    let mut iterator = occt::top_tools::TopTools_ListIteratorOfListOfShape_new(&list);
    if iterator.Value().IsNull() || iterator.Value().ShapeType() != TopAbs_ShapeEnum::TopAbs_SHELL {
        return Err(OcctError::InvalidInput {
            what: "a solid shell must be an OCCT shell",
        }
        .into());
    }
    let mut make = occt::b_rep_builder_api::BRepBuilderAPI_MakeSolid_new(TopoDS::Shell(iterator.Value()));
    iterator.pin_mut().Next();
    while iterator.More() {
        let shell = iterator.Value();
        if shell.IsNull() || shell.ShapeType() != TopAbs_ShapeEnum::TopAbs_SHELL {
            return Err(OcctError::InvalidInput {
                what: "a solid shell must be an OCCT shell",
            }
            .into());
        }
        let reversed = occt::topo_ds::Reversed(shell);
        make.pin_mut().add_shell(TopoDS::Shell(&reversed));
        iterator.pin_mut().Next();
    }
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "make_hollow_solid",
        }
        .into());
    }
    let mut solid = occt::topo_ds::TopoDS_Solid_to_owned(make.pin_mut().Solid());
    occt::b_rep_lib::BRepLib::OrientClosedSolid(solid.pin_mut());
    wrap(occt::topo_ds::cast_solid_to_shape(&solid), "make_hollow_solid")
}

/// The circular or conical helix of `pitch`, `height` and base `radius` about
/// +Z, as one edge on an analytic cylinder/cone.
///
/// `angle` is the cone's half angle in degrees; zero (or negative) selects a
/// cylinder. The conical height is measured along the axis, so the radius grows
/// by `height * tan(angle)`. `left_handed` mirrors the turn direction.
///
/// A circular helix is transcendental: this edge has an **exact pcurve** (a
/// straight line in the surface's parameter space) and OCCT's
/// `BRepLib::BuildCurves3d` builds its 3D representative to tolerance, exactly
/// as upstream's `TopoShape::makeHelix`.
pub fn make_helix_edge(
    pitch: f64,
    height: f64,
    radius: f64,
    angle: f64,
    left_handed: bool,
) -> Result<Shape, GeomError> {
    finite(pitch, "the helix pitch must be finite")?;
    finite(height, "the helix height must be finite")?;
    finite(radius, "the helix radius must be finite")?;
    finite(angle, "the helix angle must be finite")?;
    if pitch.abs() <= 1e-7 {
        return Err(OcctError::InvalidInput {
            what: "the helix pitch is too small",
        }
        .into());
    }
    if height.abs() <= 1e-7 {
        return Err(OcctError::InvalidInput {
            what: "the helix height is too small",
        }
        .into());
    }
    if (height > 0.0 && pitch < 0.0) || (height < 0.0 && pitch > 0.0) {
        return Err(OcctError::InvalidInput {
            what: "the helix pitch and height signs are incompatible",
        }
        .into());
    }
    if angle <= 1e-9 && radius <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the helix radius is too small",
        }
        .into());
    }

    let conical = angle > 1e-9;
    let semi_angle = if conical { radians(angle) } else { 0.0 };
    if conical && (semi_angle <= 1e-7 || semi_angle >= std::f64::consts::FRAC_PI_2 - 1e-12) {
        return Err(OcctError::InvalidInput {
            what: "the helix cone angle is out of range",
        }
        .into());
    }

    let origin = occt::gp::new_point(0.0, 0.0, 0.0);
    let z = occt::gp::gp_Dir_new(0.0, 0.0, 1.0);
    let axis = occt::gp::gp_Ax2_new(&origin, &z);
    let ax3 = occt::gp::gp_Ax3_from_gp_Ax2(&axis);
    let surface = if conical {
        let conical = occt::geom::Geom_ConicalSurface_new(&ax3, semi_angle, radius)
            .map_err(|_| not_done("make_helix_edge"))?;
        occt::geom::conical_to_surface(&conical)
    } else {
        let cylinder = occt::geom::Geom_CylindricalSurface_new(&ax3, radius);
        occt::geom::cylinder_to_surface(&cylinder)
    };

    let direction = if left_handed { -1.0 } else { 1.0 };
    let start = occt::gp::new_point_2d(0.0, 0.0);
    let u = direction * std::f64::consts::TAU * (height / pitch);
    let v = if conical {
        height / semi_angle.cos()
    } else {
        height
    };
    let segment = occt::gc::GCE2d_MakeSegment_point_point(&start, &occt::gp::new_point_2d(u, v));
    let segment_curve = occt::geom2d::HandleGeom2d_TrimmedCurve_to_curve(&segment);
    let mut make = occt::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2d(&segment_curve, &surface);
    let edge = occt::topo_ds::TopoDS_Edge_to_owned(make.pin_mut().Edge());
    if occt::topo_ds::cast_edge_to_shape(&edge).IsNull() {
        return Err(OcctError::NotDone {
            operation: "make_helix_edge",
        }
        .into());
    }
    occt::b_rep_lib::BRepLib::BuildCurves3d(occt::topo_ds::cast_edge_to_shape(&edge));
    wrap(occt::topo_ds::cast_edge_to_shape(&edge), "make_helix_edge")
}

/// Sweep `profiles` along `spine` with `BRepOffsetAPI_MakePipeShell`
/// (`SetMode`/`SetTransitionMode`).
///
/// The spine may be a wire or a single edge. `frenet` selects a Frenet
/// trihedron instead of the corrected Frenet one; the transition is OCCT's
/// `BRepBuilderAPI_Transformed`. With `solid`, the swept shell is closed into a
/// solid.
pub fn make_pipe_shell(
    spine: &Shape,
    profiles: &[Shape],
    solid: bool,
    frenet: bool,
) -> Result<Shape, GeomError> {
    if profiles.is_empty() {
        return Err(OcctError::InvalidInput {
            what: "a pipe needs at least one profile",
        }
        .into());
    }
    let occt_spine = downcast(spine)?;
    let guard = occt_spine.borrow();
    let inner = guard
        .as_ref()
        .ok_or(OcctError::InvalidInput { what: "null spine" })?;

    let wire = match inner.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_WIRE => occt::topo_ds::TopoDS_Wire_to_owned(TopoDS::Wire(inner)),
        TopAbs_ShapeEnum::TopAbs_EDGE => {
            let mut make = occt::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
            make.pin_mut().add_edge(TopoDS::Edge(inner));
            if !make.IsDone() {
                return Err(OcctError::NotDone {
                    operation: "make_pipe_shell",
                }
                .into());
            }
            occt::topo_ds::TopoDS_Wire_to_owned(make.pin_mut().Wire())
        }
        _ => {
            return Err(OcctError::InvalidInput {
                what: "the spine must be a wire or an edge",
            }
            .into());
        }
    };

    let list = shape_list(profiles, "a pipe profile must be an OCCT shape")?;
    let mut make = occt::b_rep_offset_api::BRepOffsetAPI_MakePipeShell_new(&wire);
    make.pin_mut().SetMode(frenet);
    make.pin_mut()
        .SetTransitionMode(occt::b_rep_offset_api::BRepBuilderAPI_TransitionMode::BRepBuilderAPI_Transformed);

    let mut iterator = occt::top_tools::TopTools_ListIteratorOfListOfShape_new(&list);
    while iterator.More() {
        let profile = iterator.Value();
        if profile.IsNull() {
            return Err(OcctError::InvalidInput {
                what: "a pipe profile must be an OCCT shape",
            }
            .into());
        }
        match profile.ShapeType() {
            TopAbs_ShapeEnum::TopAbs_WIRE | TopAbs_ShapeEnum::TopAbs_VERTEX => {
                make.pin_mut().Add(profile, false, false);
            }
            TopAbs_ShapeEnum::TopAbs_EDGE => {
                let mut profile_wire = occt::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
                profile_wire.pin_mut().add_edge(TopoDS::Edge(profile));
                if !profile_wire.IsDone() {
                    return Err(OcctError::NotDone {
                        operation: "make_pipe_shell",
                    }
                    .into());
                }
                make.pin_mut()
                    .Add(occt::topo_ds::cast_wire_to_shape(profile_wire.pin_mut().Wire()), false, false);
            }
            TopAbs_ShapeEnum::TopAbs_FACE => {
                let explorer =
                    occt::top_exp::TopExp_Explorer_new(profile, TopAbs_ShapeEnum::TopAbs_WIRE);
                if !explorer.More() {
                    return Err(OcctError::InvalidInput {
                        what: "a face profile needs an outer wire",
                    }
                    .into());
                }
                make.pin_mut().Add(explorer.Current(), false, false);
            }
            _ => {
                return Err(OcctError::InvalidInput {
                    what: "a pipe profile must be a wire, edge, vertex or face",
                }
                .into());
            }
        }
        iterator.pin_mut().Next();
    }
    if !make.IsReady() {
        return Err(OcctError::NotDone {
            operation: "make_pipe_shell",
        }
        .into());
    }
    let progress = occt::message::Message_ProgressRange_new();
    make.pin_mut().Build(&progress);
    if solid && !make.pin_mut().MakeSolid() {
        return Err(OcctError::NotDone {
            operation: "make_pipe_shell",
        }
        .into());
    }
    let mut result = occt::topo_ds::TopoDS_Shape_to_owned(make.pin_mut().Shape());
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "make_pipe_shell",
        }
        .into());
    }

    // The builder may wrap the solid in a one-child compound; return the solid
    // itself so `Shape.ShapeType` is stable.
    if solid && result.ShapeType() == TopAbs_ShapeEnum::TopAbs_COMPOUND {
        let mut iterator = occt::topo_ds::TopoDS_Iterator_new(&result);
        let first = if iterator.More() {
            occt::topo_ds::TopoDS_Shape_to_owned(iterator.Value())
        } else {
            occt::topo_ds::TopoDS_Shape_to_owned(&result)
        };
        iterator.pin_mut().Next();
        if !first.IsNull()
            && !iterator.More()
            && first.ShapeType() == TopAbs_ShapeEnum::TopAbs_SOLID
        {
            result = first;
        }
    }
    wrap(&result, "make_pipe_shell")
}

/// Upstream's `TopoShape::makeThread`: a solid lofted between two thread-profile
/// wires on coaxial cylinders of `radius` and `radius + depth`.
///
/// Each profile is a half-ellipse in the surface's `(u, v)` parameter space
/// (major axis `2*pi`, minor axis `pitch` on the inner cylinder and `pitch / 4`
/// on the outer one) closed by its major-axis chord. The 2D curves become edges
/// on their cylinders, `BRepLib::BuildCurves3d` builds the 3D representatives,
/// and `BRepOffsetAPI_ThruSections` lofts the two wires into a solid — exactly
/// upstream's construction.
pub fn make_thread(pitch: f64, depth: f64, height: f64, radius: f64) -> Result<Shape, GeomError> {
    finite(pitch, "the thread pitch must be finite")?;
    finite(depth, "the thread depth must be finite")?;
    finite(height, "the thread height must be finite")?;
    finite(radius, "the thread radius must be finite")?;
    if pitch <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the thread pitch must be positive",
        }
        .into());
    }
    if depth <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the thread depth must be positive",
        }
        .into());
    }
    if height <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the thread height must be positive",
        }
        .into());
    }
    if radius <= 0.0 {
        return Err(OcctError::InvalidInput {
            what: "the thread radius must be positive",
        }
        .into());
    }

    let origin = occt::gp::new_point(0.0, 0.0, 0.0);
    let z = occt::gp::gp_Dir_new(0.0, 0.0, 1.0);
    let axis = occt::gp::gp_Ax2_new(&origin, &z);
    let ax3 = occt::gp::gp_Ax3_from_gp_Ax2(&axis);
    let inner = occt::geom::Geom_CylindricalSurface_new(&ax3, radius);
    let outer = occt::geom::Geom_CylindricalSurface_new(&ax3, radius + depth);
    let inner_surface = occt::geom::cylinder_to_surface(&inner);
    let outer_surface = occt::geom::cylinder_to_surface(&outer);

    let tau = std::f64::consts::TAU;
    let center = occt::gp::new_point_2d(tau, height / 2.0);
    let dir2d = occt::gp::gp_Dir2d_new(tau, height / 4.0);
    let thread_axis = occt::gp::gp_Ax2d_new(&center, &dir2d);

    let ellipse_inner = occt::geom2d::Geom2d_Ellipse_new(&thread_axis, tau, pitch);
    let ellipse_outer = occt::geom2d::Geom2d_Ellipse_new(&thread_axis, tau, pitch / 4.0);
    let inner_curve = occt::geom2d::ellipse_to_HandleGeom2d_Curve(&ellipse_inner);
    let outer_curve = occt::geom2d::ellipse_to_HandleGeom2d_Curve(&ellipse_outer);
    let arc_inner = occt::geom2d::Geom2d_TrimmedCurve_new(&inner_curve, 0.0, std::f64::consts::PI);
    let arc_outer = occt::geom2d::Geom2d_TrimmedCurve_new(&outer_curve, 0.0, std::f64::consts::PI);
    let arc_inner_curve = occt::geom2d::HandleGeom2d_TrimmedCurve_to_curve(&arc_inner);
    let arc_outer_curve = occt::geom2d::HandleGeom2d_TrimmedCurve_to_curve(&arc_outer);

    let start = occt::geom2d::ellipse_value(&ellipse_inner, 0.0);
    let end = occt::geom2d::ellipse_value(&ellipse_inner, std::f64::consts::PI);
    let segment = occt::gc::GCE2d_MakeSegment_point_point(&start, &end);
    let segment_curve = occt::geom2d::HandleGeom2d_TrimmedCurve_to_curve(&segment);

    let mut inner_arc = occt::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2d(&arc_inner_curve, &inner_surface);
    let mut inner_chord = occt::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2d(&segment_curve, &inner_surface);
    let mut outer_arc = occt::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2d(&arc_outer_curve, &outer_surface);
    let mut outer_chord = occt::b_rep_builder_api::BRepBuilderAPI_MakeEdge_CurveSurface2d(&segment_curve, &outer_surface);

    let inner_arc_edge = occt::topo_ds::TopoDS_Edge_to_owned(inner_arc.pin_mut().Edge());
    let inner_chord_edge = occt::topo_ds::TopoDS_Edge_to_owned(inner_chord.pin_mut().Edge());
    let outer_arc_edge = occt::topo_ds::TopoDS_Edge_to_owned(outer_arc.pin_mut().Edge());
    let outer_chord_edge = occt::topo_ds::TopoDS_Edge_to_owned(outer_chord.pin_mut().Edge());
    if occt::topo_ds::cast_edge_to_shape(&inner_arc_edge).IsNull()
        || occt::topo_ds::cast_edge_to_shape(&inner_chord_edge).IsNull()
        || occt::topo_ds::cast_edge_to_shape(&outer_arc_edge).IsNull()
        || occt::topo_ds::cast_edge_to_shape(&outer_chord_edge).IsNull()
    {
        return Err(OcctError::NotDone {
            operation: "make_thread",
        }
        .into());
    }

    let mut inner_wire = occt::b_rep_builder_api::BRepBuilderAPI_MakeWire_edge_edge(&inner_arc_edge, &inner_chord_edge);
    let mut outer_wire = occt::b_rep_builder_api::BRepBuilderAPI_MakeWire_edge_edge(&outer_arc_edge, &outer_chord_edge);
    if !inner_wire.IsDone() || !outer_wire.IsDone() {
        return Err(OcctError::NotDone {
            operation: "make_thread",
        }
        .into());
    }
    occt::b_rep_lib::BRepLib::BuildCurves3d(occt::topo_ds::cast_wire_to_shape(inner_wire.pin_mut().Wire()));
    occt::b_rep_lib::BRepLib::BuildCurves3d(occt::topo_ds::cast_wire_to_shape(outer_wire.pin_mut().Wire()));

    let mut loft = occt::b_rep_offset_api::BRepOffsetAPI_ThruSections_new(true);
    loft.pin_mut().AddWire(inner_wire.pin_mut().Wire());
    loft.pin_mut().AddWire(outer_wire.pin_mut().Wire());
    loft.pin_mut().CheckCompatibility(false);
    let progress = occt::message::Message_ProgressRange_new();
    loft.pin_mut().Build(&progress);
    if !loft.IsDone() {
        return Err(OcctError::NotDone {
            operation: "make_thread",
        }
        .into());
    }
    wrap(loft.pin_mut().Shape(), "make_thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::sub_shape_map;
    use ferrocad_geom::TessellationOptions;
    use occt::top_abs::TopAbs_ShapeEnum;
    use occt::topo_ds::TopoDS;

    const RADIUS: f64 = 2.0;
    const DEPTH: f64 = 0.5;

    /// The thread is upstream's two-profile loft: one closed shell whose
    /// vertices lie on the inner (`radius`) and outer (`radius + depth`)
    /// cylinders, with a positive volume and a non-empty tessellation.
    #[test]
    fn make_thread_is_a_closed_solid_in_its_radial_band() {
        let shape = make_thread(1.0, DEPTH, 4.0, RADIUS).expect("the thread solid builds");
        let occt_shape = downcast(&shape).expect("an OCCT shape");
        let guard = occt_shape.borrow();
        let inner = guard.as_ref().expect("a non-null shape");

        assert_eq!(inner.ShapeType(), TopAbs_ShapeEnum::TopAbs_SOLID);
        let shells = sub_shape_map(inner, TopAbs_ShapeEnum::TopAbs_SHELL);
        assert_eq!(shells.Extent(), 1, "one shell");
        let vertices = sub_shape_map(inner, TopAbs_ShapeEnum::TopAbs_VERTEX);
        assert!(vertices.Extent() > 0, "non-empty");

        // `only_closed` ignores open shells, so a positive mass proves the shell
        // is closed as well as enclosing volume.
        let mut props = occt::g_prop::GProps_new();
        occt::b_rep_g_prop::BRepGProp::VolumeProperties(inner, props.pin_mut(), true, false, false);
        let volume = props.Mass();
        assert!(volume > 0.0, "positive volume, got {volume}");

        // Every vertex sits on one of the two cylinders, so the radial band is
        // exactly [radius, radius + depth].
        let mut min_radius = f64::INFINITY;
        let mut max_radius = 0.0_f64;
        for index in 1..=vertices.Extent() {
            let vertex = TopoDS::Vertex(vertices.FindKey(index));
            let point = occt::b_rep::BRep_Tool_Pnt(vertex);
            let radius = point.X().hypot(point.Y());
            min_radius = min_radius.min(radius);
            max_radius = max_radius.max(radius);
        }
        assert!(
            (min_radius - RADIUS).abs() < 1e-6,
            "inner radius was {min_radius}"
        );
        assert!(
            (max_radius - (RADIUS + DEPTH)).abs() < 1e-6,
            "outer radius was {max_radius}"
        );

        let mesh = crate::topology::tessellate(&shape, &TessellationOptions::default())
            .expect("the thread tessellates");
        assert!(!mesh.triangles.is_empty(), "triangles after tessellation");
    }
}
