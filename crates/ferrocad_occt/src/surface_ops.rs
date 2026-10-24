//! The OCCT surface and solid operations behind Part's remaining seam.
//!
//! A sibling cxx bridge over `include/fc_surface_ops.hxx` (the same pattern as
//! [`crate::chfi2d`] and [`crate::bridge`]): `opencascade-sys` 0.3 does not
//! bridge partial spheres, variable-radius fillets, filled planar offsets,
//! multi-shell solids, helices or pipe shells, so the header wraps the OCCT
//! algorithms over the crate's own `TopoDS_Shape` type.
//!
//! Every public function returns `Result<Shape, GeomError>`, the seam's error
//! type: a null result becomes `Part.OCCError` (upstream's
//! `Base.CADKernelError`) in Python, and arguments that cannot describe the
//! operation are rejected here before OCCT sees them.
//!
//! # Exactness
//!
//! Partial spheres, variable fillets, offsets, hollow solids and pipe sweeps are
//! exact OCCT constructions. A circular helix is transcendental and has no
//! polynomial OCCT curve: [`make_helix_edge`] builds the same object upstream
//! does, an edge whose **pcurve is exact** on an analytic cylinder/cone, with
//! `BRepLib::BuildCurves3d` producing the 3D representative to its tolerance.

use cxx::UniquePtr;
use ferrocad_curves::Vector3;
use ferrocad_geom::{GeomError, Shape};
use opencascade_sys as occt;

use occt::top_tools::TopTools_ListOfShape;
use occt::topo_ds::TopoDS_Shape;

use crate::error::OcctError;
use crate::shape::{downcast, OcctShape};

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("ferrocad_occt/include/fc_surface_ops.hxx");

        // Reuse the crate's opaque types. cxx shares these across bridges by path.
        type TopoDS_Shape = opencascade_sys::topo_ds::TopoDS_Shape;
        type TopTools_ListOfShape = opencascade_sys::top_tools::TopTools_ListOfShape;

        /// A sphere slice (see the header for the `u`/`v` conventions).
        fn fc_make_sphere_slice(
            radius: f64,
            x: f64,
            y: f64,
            z: f64,
            u1: f64,
            u2: f64,
            v1: f64,
            v2: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A fillet with a per-edge radius law (`Add(start, end, edge)`).
        fn fc_fillet_variable(
            shape: &TopoDS_Shape,
            edges: &TopTools_ListOfShape,
            start_radii: &[f64],
            end_radii: &[f64],
        ) -> UniquePtr<TopoDS_Shape>;

        /// `BRepOffsetAPI_MakeOffset`, optionally filled into the band face.
        fn fc_offset2d(
            shape: &TopoDS_Shape,
            distance: f64,
            kind: i32,
            fill: bool,
            open_result: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A solid from an outer shell plus cavity shells.
        fn fc_make_hollow_solid(shells: &TopTools_ListOfShape) -> UniquePtr<TopoDS_Shape>;

        /// An edge on the analytic cylinder/cone whose pcurve is the helix line.
        fn fc_make_helix_edge(
            pitch: f64,
            height: f64,
            radius: f64,
            angle: f64,
            left_handed: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// `BRepOffsetAPI_MakePipeShell` with `SetMode`/`SetTransitionMode`.
        fn fc_make_pipe_shell(
            spine: &TopoDS_Shape,
            profiles: &TopTools_ListOfShape,
            solid: bool,
            frenet: bool,
        ) -> UniquePtr<TopoDS_Shape>;
    }
}

/// Wrap a bridge result, reporting a null result as OCCT's `NotDone`.
fn owned(shape: UniquePtr<TopoDS_Shape>, operation: &'static str) -> Result<Shape, GeomError> {
    if shape.is_null() {
        return Err(OcctError::NotDone { operation }.into());
    }
    Ok(Shape::new(OcctShape(shape)))
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
    owned(
        ffi::fc_make_sphere_slice(radius, center.x, center.y, center.z, u1, u2, v1, v2),
        "make_sphere_slice",
    )
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
    owned(
        ffi::fc_fillet_variable(inner, &list, start_radii, end_radii),
        "fillet_variable",
    )
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
    owned(
        ffi::fc_offset2d(inner, distance, kind, fill, open_result),
        "offset2d",
    )
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
    owned(ffi::fc_make_hollow_solid(&list), "make_hollow_solid")
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
    owned(
        ffi::fc_make_helix_edge(pitch, height, radius, angle, left_handed),
        "make_helix_edge",
    )
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
    let list = shape_list(profiles, "a pipe profile must be an OCCT shape")?;
    owned(
        ffi::fc_make_pipe_shell(inner, &list, solid, frenet),
        "make_pipe_shell",
    )
}
