//! The OCCT shape algorithms Part exposes that are not part of the
//! [`GeometryBackend`](ferrocad_geom::GeometryBackend) seam: face domain tests,
//! face validation, projection, defeaturing, 3D offsetting, ordered wire
//! traversal and the B-spline surface data model.
//!
//! Everything here is a thin, safe wrapper over the sys bindings
//! ([`opencascade_sys`]). FreeCAD's operation policy lives in [`crate::policy`];
//! the seam stays kernel-agnostic and these calls are OCCT-only operations a
//! `dyn GeometryBackend` cannot usefully abstract.
//!
//! Errors are [`GeomError`] (built from [`OcctError`]) so the Python layer maps
//! them to `Part.OCCError` exactly like the seam calls do.

use cxx::UniquePtr;
use ferrocad_curves::Vector3;
use ferrocad_geom::{BsplineSurfaceData, GeomError, Shape};
use opencascade_sys as ffi;

use ffi::top_abs::{TopAbs_Orientation, TopAbs_ShapeEnum};
use ffi::top_tools::TopTools_ListOfShape;
use ffi::topo_ds::{TopoDS, TopoDS_Shape};

use crate::error::OcctError;
use crate::shape::downcast;

/// Build an OCCT list from kernel shapes.
fn shape_list(shapes: &[Shape]) -> Option<UniquePtr<TopTools_ListOfShape>> {
    let mut list = ffi::top_tools::new_list_of_shape();
    for shape in shapes {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        list.pin_mut().Append(guard.as_ref()?);
    }
    Some(list)
}

/// Take OCCT's output shape, or report the operation as not done.
fn result(shape: UniquePtr<TopoDS_Shape>, operation: &'static str) -> Result<Shape, GeomError> {
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
pub fn shape_kind(shape: &Shape) -> Option<&'static str> {
    let occt = downcast(shape).ok()?;
    let guard = occt.borrow();
    let kind = guard.as_ref()?.ShapeType();
    Some(match kind {
        TopAbs_ShapeEnum::TopAbs_COMPOUND => "Compound",
        TopAbs_ShapeEnum::TopAbs_COMPSOLID => "CompSolid",
        TopAbs_ShapeEnum::TopAbs_SOLID => "Solid",
        TopAbs_ShapeEnum::TopAbs_SHELL => "Shell",
        TopAbs_ShapeEnum::TopAbs_FACE => "Face",
        TopAbs_ShapeEnum::TopAbs_WIRE => "Wire",
        TopAbs_ShapeEnum::TopAbs_EDGE => "Edge",
        TopAbs_ShapeEnum::TopAbs_VERTEX => "Vertex",
        _ => "Shape",
    })
}

/// `Face.isPartOfDomain(u, v)`: whether the parameter pair is inside the face.
pub fn face_is_part_of_domain(face: &Shape, u: f64, v: f64) -> Result<bool, GeomError> {
    borrow(face, |first| {
        if first.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE {
            return Err(OcctError::InvalidInput {
                what: "face_is_part_of_domain expects a face",
            });
        }
        let state = ffi::b_rep_top_adaptor::BRepTopAdaptor_FClass2d_Perform(TopoDS::Face(first), u, v)
            .map_err(|e| OcctError::Exception {
                operation: "face_is_part_of_domain",
                message: e.what().to_string(),
            })?;
        Ok(state == ffi::b_rep_class::TopAbs_State::TopAbs_ON
            || state == ffi::b_rep_class::TopAbs_State::TopAbs_IN)
    })
}

/// `Face.validate()`: repair an invalid face.
pub fn validate_face(face: &Shape) -> Result<Shape, GeomError> {
    borrow(face, |first| crate::policy::validate_face(first))
        .and_then(|output| result(output, "validate"))
}

/// `Shape.fix(precision, mintol, maxtol)`: the repaired shape.
pub fn fix_shape(
    shape: &Shape,
    precision: f64,
    min_tolerance: f64,
    max_tolerance: f64,
) -> Result<Shape, GeomError> {
    borrow(shape, |first| {
        crate::policy::fix_shape(first, precision, min_tolerance, max_tolerance)
    })
    .and_then(|output| result(output, "fix"))
}

/// The `(u, v)` of `point` on a face's surface.
pub fn face_parameter(face: &Shape, point: Vector3) -> Result<(f64, f64), GeomError> {
    borrow(face, |first| {
        if first.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE {
            return Err(OcctError::InvalidInput {
                what: "face_parameter expects a face",
            });
        }
        let surface = ffi::b_rep::BRep_Tool_Surface(TopoDS::Face(first));
        if surface.IsNull() {
            return Err(OcctError::NotDone {
                operation: "face parameter",
            });
        }
        let mut analysis = ffi::shape_analysis::ShapeAnalysis_Surface_new(&surface).map_err(|e| {
            OcctError::Exception {
                operation: "face parameter",
                message: e.what().to_string(),
            }
        })?;
        let pnt = ffi::gp::new_point(point.x, point.y, point.z);
        let uv = ffi::shape_analysis::ShapeAnalysis_Surface_ValueOfUV(
            analysis.pin_mut(),
            &pnt,
            ffi::precision::Precision::Confusion(),
        )
        .map_err(|e| OcctError::Exception {
            operation: "face parameter",
            message: e.what().to_string(),
        })?;
        Ok((uv.X(), uv.Y()))
    })
}

/// Project `tool` onto `target` along `direction` (parallel) or from the
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
    let output = crate::policy::project(first, second, direction, parallel)?;
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
    borrow(shape, |first| crate::policy::defeature(first, &faces))
        .and_then(|output| result(output, "defeaturing"))
}

/// `Shape.makeOffsetShape(offset, tolerance, …)`: a 3D offset.
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
        crate::policy::offset_shape(
            first,
            offset,
            tolerance,
            inter,
            self_inter,
            offset_mode,
            join,
            fill,
        )
    })
    .and_then(|output| result(output, "makeOffsetShape"))
}

/// `Wire.OrderedEdges`: the wire's edges in connection order.
pub fn wire_ordered_edges(wire: &Shape) -> Result<Vec<Shape>, GeomError> {
    let compound = borrow(wire, |first| {
        if first.ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
            return Err(OcctError::InvalidInput {
                what: "wire_ordered_edges expects a wire",
            });
        }
        let mut compound = ffi::topo_ds::TopoDS_Compound_new();
        let builder = ffi::b_rep::BRep_Builder_new();
        let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
        builder.MakeCompound(compound.pin_mut());
        let mut compound_shape = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
        let mut xp = ffi::b_rep_tools::BRepTools_WireExplorer_new(TopoDS::Wire(first));
        while xp.More() {
            builder.Add(
                compound_shape.pin_mut(),
                ffi::topo_ds::cast_edge_to_shape(xp.Current()),
            );
            xp.pin_mut().Next();
        }
        Ok(compound_shape)
    })
    .and_then(|output| result(output, "OrderedEdges"))?;
    Ok(child_shapes(&compound))
}

/// The vertex an edge starts at in its own orientation, or `None`.
pub fn edge_first_vertex(edge: &Shape) -> Option<Shape> {
    let occt = downcast(edge).ok()?;
    let guard = occt.borrow();
    let vertex = crate::curve::edge_first_vertex(guard.as_ref()?);
    vertex.map(|vertex| crate::shape::erase(&vertex))
}

/// The vertex an edge ends at in its own orientation, or `None`.
pub fn edge_last_vertex(edge: &Shape) -> Option<Shape> {
    let occt = downcast(edge).ok()?;
    let guard = occt.borrow();
    let vertex = crate::curve::edge_last_vertex(guard.as_ref()?);
    vertex.map(|vertex| crate::shape::erase(&vertex))
}

/// A wire's stored first and last vertex, or `None`.
pub fn wire_end_vertices(wire: &Shape) -> Option<(Shape, Shape)> {
    let occt = downcast(wire).ok()?;
    let guard = occt.borrow();
    let inner = guard.as_ref()?;
    let first = crate::curve::wire_first_vertex(inner)?;
    let last = crate::curve::wire_last_vertex(inner)?;
    Some((
        crate::shape::erase(&first),
        crate::shape::erase(&last),
    ))
}

/// The direct children of a shape.
fn child_shapes(compound: &Shape) -> Vec<Shape> {
    borrow(compound, |inner| {
        Ok(crate::history::children(inner)
            .into_iter()
            .filter(|child| !child.is_null())
            .map(|child| Shape::new(crate::shape::OcctShape(child)))
            .collect())
    })
    .unwrap_or_default()
}

/// A B-spline surface, described by everything Part's surface API reads.
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
    /// Build the OCCT `Geom_BSplineSurface` this description names.
    pub(crate) fn build(&self) -> Result<UniquePtr<ffi::geom::Handle_Geom_BSplineSurface>, OcctError> {
        let nu = self.poles.len();
        let nv = self.poles.first().map_or(0, Vec::len);
        if self.u_degree < 1
            || self.v_degree < 1
            || nu < 2
            || nv < 2
            || self.poles.iter().any(|row| row.len() != nv)
        {
            return Err(OcctError::InvalidInput {
                what: "a B-spline surface needs at least two by two poles",
            });
        }
        let mut poles = ffi::t_col_gp::TColgp_Array2OfPnt_new(1, nu as i32, 1, nv as i32);
        let mut weights = ffi::t_col_std::TColStd_Array2OfReal_new(1, nu as i32, 1, nv as i32);
        for (i, row) in self.poles.iter().enumerate() {
            for (j, (point, weight)) in row.iter().enumerate() {
                let pnt = ffi::gp::new_point(point.x, point.y, point.z);
                poles
                    .pin_mut()
                    .SetValue((i + 1) as i32, (j + 1) as i32, &pnt);
                ffi::t_col_std::TColStd_Array2OfReal_SetValue(
                    weights.pin_mut(),
                    (i + 1) as i32,
                    (j + 1) as i32,
                    *weight,
                );
            }
        }

        let mut uknots = ffi::t_col_std::TColStd_Array1OfReal_new(1, self.u_knots.len() as i32);
        let mut umults = ffi::t_col_std::TColStd_Array1OfInteger_new(1, self.u_knots.len() as i32);
        for (i, (knot, mult)) in self.u_knots.iter().enumerate() {
            ffi::t_col_std::TColStd_Array1OfReal_SetValue(uknots.pin_mut(), (i + 1) as i32, *knot);
            ffi::t_col_std::TColStd_Array1OfInteger_SetValue(umults.pin_mut(), (i + 1) as i32, *mult);
        }
        let mut vknots = ffi::t_col_std::TColStd_Array1OfReal_new(1, self.v_knots.len() as i32);
        let mut vmults = ffi::t_col_std::TColStd_Array1OfInteger_new(1, self.v_knots.len() as i32);
        for (i, (knot, mult)) in self.v_knots.iter().enumerate() {
            ffi::t_col_std::TColStd_Array1OfReal_SetValue(vknots.pin_mut(), (i + 1) as i32, *knot);
            ffi::t_col_std::TColStd_Array1OfInteger_SetValue(vmults.pin_mut(), (i + 1) as i32, *mult);
        }

        ffi::geom::Geom_BSplineSurface_new_full(
            &poles,
            &weights,
            &uknots,
            &vknots,
            &umults,
            &vmults,
            self.u_degree,
            self.v_degree,
            self.u_periodic,
            self.v_periodic,
        )
        .map_err(|e| OcctError::Exception {
            operation: "BSplineSurface.build",
            message: e.what().to_string(),
        })
    }

    /// Read a B-spline surface handle back into this description.
    pub(crate) fn from_surface(surface: &ffi::geom::Handle_Geom_Surface) -> Option<Self> {
        let bspline = ffi::geom::downcast_to_bspline_surface(surface);
        if bspline.IsNull() {
            return None;
        }
        let nu = ffi::geom::HandleGeomBSplineSurface_NbUPoles(&bspline);
        let nv = ffi::geom::HandleGeomBSplineSurface_NbVPoles(&bspline);
        let mut u1 = 0.0;
        let mut u2 = 0.0;
        let mut v1 = 0.0;
        let mut v2 = 0.0;
        ffi::geom::HandleGeomBSplineSurface_Bounds(&bspline, &mut u1, &mut u2, &mut v1, &mut v2);

        let mut poles = Vec::with_capacity(nu as usize);
        for i in 1..=nu {
            let mut row = Vec::with_capacity(nv as usize);
            for j in 1..=nv {
                let point = ffi::geom::HandleGeomBSplineSurface_Pole(&bspline, i, j);
                let weight = ffi::geom::HandleGeomBSplineSurface_Weight(&bspline, i, j);
                row.push((Vector3::new(point.X(), point.Y(), point.Z()), weight));
            }
            poles.push(row);
        }

        let u_knots = (1..=ffi::geom::HandleGeomBSplineSurface_NbUKnots(&bspline))
            .map(|i| {
                (
                    ffi::geom::HandleGeomBSplineSurface_UKnot(&bspline, i),
                    ffi::geom::HandleGeomBSplineSurface_UMultiplicity(&bspline, i),
                )
            })
            .collect();
        let v_knots = (1..=ffi::geom::HandleGeomBSplineSurface_NbVKnots(&bspline))
            .map(|i| {
                (
                    ffi::geom::HandleGeomBSplineSurface_VKnot(&bspline, i),
                    ffi::geom::HandleGeomBSplineSurface_VMultiplicity(&bspline, i),
                )
            })
            .collect();

        Some(BsplineSurface {
            u_degree: ffi::geom::HandleGeomBSplineSurface_UDegree(&bspline),
            v_degree: ffi::geom::HandleGeomBSplineSurface_VDegree(&bspline),
            u_periodic: ffi::geom::HandleGeomBSplineSurface_IsUPeriodic(&bspline),
            v_periodic: ffi::geom::HandleGeomBSplineSurface_IsVPeriodic(&bspline),
            u_bounds: (u1, u2),
            v_bounds: (v1, v2),
            poles,
            u_knots,
            v_knots,
        })
    }
}

/// The B-spline surface behind a face, or `None` for any other surface kind.
pub fn bspline_surface_data(face: &Shape) -> Option<BsplineSurface> {
    let occt = downcast(face).ok()?;
    let guard = occt.borrow();
    let inner = guard.as_ref()?;
    if inner.ShapeType() != TopAbs_ShapeEnum::TopAbs_FACE {
        return None;
    }
    let surface = ffi::b_rep::BRep_Tool_Surface(TopoDS::Face(inner));
    BsplineSurface::from_surface(&surface)
}

/// The face of a B-spline surface over its whole parameter domain.
pub fn bspline_surface_face(surface: &BsplineSurface) -> Result<Shape, GeomError> {
    let bspline = surface.build()?;
    let support = ffi::geom::bspline_to_surface(&bspline);
    let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_surface_range(
        &support,
        surface.u_bounds.0,
        surface.u_bounds.1,
        surface.v_bounds.0,
        surface.v_bounds.1,
        ffi::precision::Precision::Confusion(),
    )
    .map_err(|e| OcctError::Exception {
        operation: "BSplineSurface.toShape",
        message: e.what().to_string(),
    })?;
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "BSplineSurface.toShape",
        }
        .into());
    }
    Ok(crate::shape::erase(ffi::topo_ds::cast_face_to_shape(make.Face())))
}

/// `BSplineSurface.value(u, v)`.
pub fn bspline_surface_value(
    surface: &BsplineSurface,
    u: f64,
    v: f64,
) -> Result<Vector3, GeomError> {
    let bspline = surface.build()?;
    let point = ffi::geom::HandleGeomBSplineSurface_Value(&bspline, u, v);
    Ok(Vector3::new(point.X(), point.Y(), point.Z()))
}

/// `BSplineSurface.parameter(point)`.
pub fn bspline_surface_parameter(
    surface: &BsplineSurface,
    point: Vector3,
) -> Result<(f64, f64), GeomError> {
    let bspline = surface.build()?;
    let support = ffi::geom::bspline_to_surface(&bspline);
    let mut analysis = ffi::shape_analysis::ShapeAnalysis_Surface_new(&support).map_err(|e| {
        OcctError::Exception {
            operation: "BSplineSurface.parameter",
            message: e.what().to_string(),
        }
    })?;
    let pnt = ffi::gp::new_point(point.x, point.y, point.z);
    let uv = ffi::shape_analysis::ShapeAnalysis_Surface_ValueOfUV(
        analysis.pin_mut(),
        &pnt,
        ffi::precision::Precision::Confusion(),
    )
    .map_err(|e| OcctError::Exception {
        operation: "BSplineSurface.parameter",
        message: e.what().to_string(),
    })?;
    Ok((uv.X(), uv.Y()))
}

/// `Part.Face(bspline_surface, wires)`: trim the surface with boundary wires.
pub fn face_on_bspline_surface(
    surface: &BsplineSurface,
    wires: &[Shape],
) -> Result<Shape, GeomError> {
    let wires = shape_list(wires)
        .ok_or_else(|| GeomError::from(OcctError::InvalidInput { what: "boundary wire" }))?;
    let bspline = surface.build()?;
    let support = ffi::geom::bspline_to_surface(&bspline);
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(&wires);
    if !it.More() || it.Value().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
        return Err(OcctError::InvalidInput {
            what: "boundary wire",
        }
        .into());
    }
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_surface_wire(
        &support,
        TopoDS::Wire(it.Value()),
        true,
    )
    .map_err(|e| OcctError::Exception {
        operation: "makeFaceOnBSplineSurface",
        message: e.what().to_string(),
    })?;
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "makeFaceOnBSplineSurface",
        }
        .into());
    }
    it.pin_mut().Next();
    while it.More() {
        if it.Value().ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
            return Err(OcctError::InvalidInput {
                what: "boundary wire",
            }
            .into());
        }
        make.pin_mut().add_wire(TopoDS::Wire(it.Value()));
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "makeFaceOnBSplineSurface",
            }
            .into());
        }
        it.pin_mut().Next();
    }
    if ffi::topo_ds::cast_face_to_shape(make.Face()).IsNull() {
        return Err(OcctError::NotDone {
            operation: "makeFaceOnBSplineSurface",
        }
        .into());
    }
    Ok(crate::shape::erase(ffi::topo_ds::cast_face_to_shape(make.Face())))
}

/// `Part.makeFace(wires, "Part::FaceMakerBullseye")`.
pub fn bullseye_face(wires: &[Shape]) -> Result<Shape, GeomError> {
    let wires = shape_list(wires)
        .ok_or_else(|| GeomError::from(OcctError::InvalidInput { what: "boundary wire" }))?;
    crate::policy::bullseye_face(&wires)
        .map_err(GeomError::from)
        .and_then(|output| result(output, "makeFaceBullseye"))
}

// ---------------------------------------------------------------------------
// Shared backend helpers (used by `backend.rs`)
// ---------------------------------------------------------------------------

/// `Shape.makeShell`'s re-sew fallback (`ShapeUpgrade_ShellSewing::ApplySewing`).
pub(crate) fn sew_shell(shape: &TopoDS_Shape) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "sew_shell",
        });
    }
    let result = ffi::shape_upgrade::ShapeUpgrade_ShellSewing_ApplySewing(shape).map_err(|e| {
        OcctError::Exception {
            operation: "sew_shell",
            message: e.what().to_string(),
        }
    })?;
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "sew_shell",
        });
    }
    Ok(result)
}

/// `TopoShape::sewShape` (`BRepBuilderAPI_Sewing`).
pub(crate) fn sew_shape(
    shape: &TopoDS_Shape,
    tolerance: f64,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() || !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(OcctError::NotDone { operation: "sew" });
    }
    let mut sew = ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_new(tolerance);
    sew.pin_mut().Load(shape);
    ffi::b_rep_builder_api::BRepBuilderAPI_Sewing_Perform(sew.pin_mut()).map_err(|e| {
        OcctError::Exception {
            operation: "sew",
            message: e.what().to_string(),
        }
    })?;
    let sewn = ffi::topo_ds::TopoDS_Shape_to_owned(sew.SewedShape());
    if sewn.IsNull() {
        return Err(OcctError::NotDone { operation: "sew" });
    }
    Ok(sewn)
}

/// `TopoShape::toNurbs` (`BRepBuilderAPI_NurbsConvert`).
pub(crate) fn to_nurbs(shape: &TopoDS_Shape) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shape.IsNull() {
        return Err(OcctError::NotDone {
            operation: "to_nurbs",
        });
    }
    let mut convert = ffi::b_rep_builder_api::BRepBuilderAPI_NurbsConvert_new(shape);
    let result = ffi::topo_ds::TopoDS_Shape_to_owned(convert.pin_mut().Shape());
    if result.IsNull() {
        return Err(OcctError::NotDone {
            operation: "to_nurbs",
        });
    }
    Ok(result)
}

/// `Shell.getFreeEdges` (`ShapeAnalysis_Shell`).
pub(crate) fn shell_free_edges(
    shell: &TopoDS_Shape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if shell.IsNull() {
        return Err(OcctError::NotDone {
            operation: "shell_free_edges",
        });
    }
    let mut analysis = ffi::shape_analysis::ShapeAnalysis_Shell_new();
    ffi::shape_analysis::ShapeAnalysis_Shell_LoadShells(analysis.pin_mut(), shell);
    ffi::shape_analysis::ShapeAnalysis_Shell_CheckOrientedShells(analysis.pin_mut(), shell);
    let compound = ffi::shape_analysis::ShapeAnalysis_Shell_FreeEdges(analysis.pin_mut());
    if compound.is_null() {
        return Err(OcctError::NotDone {
            operation: "shell_free_edges",
        });
    }
    Ok(ffi::topo_ds::TopoDS_Compound_as_shape(compound))
}

/// `Edge.getParameterByLength` (`GCPnts_AbscissaPoint`).
pub(crate) fn edge_parameter_by_length(
    edge: &TopoDS_Shape,
    length: f64,
    tolerance: f64,
) -> f64 {
    if edge.IsNull() || edge.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE || !length.is_finite() {
        return f64::NAN;
    }
    let adapt = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(TopoDS::Edge(edge));
    let first = adapt.FirstParameter();
    let last = adapt.LastParameter();
    if ffi::precision::Precision::IsInfinite(first) || ffi::precision::Precision::IsInfinite(last) {
        return length;
    }
    let tol = if tolerance > 0.0 {
        tolerance
    } else {
        ffi::precision::Precision::Confusion()
    };
    let total = ffi::gc_pnts::GCPnts_AbscissaPoint_Length(&adapt, tol).unwrap_or(f64::NAN);
    if !total.is_finite() {
        return f64::NAN;
    }
    let mut length = length;
    if length < -total || length > total {
        return f64::NAN;
    }
    if length < 0.0 {
        length = total + length;
    }
    let Ok(point) = ffi::gc_pnts::GCPnts_AbscissaPoint_new(tol, &adapt, length, first) else {
        return f64::NAN;
    };
    ffi::gc_pnts::GCPnts_AbscissaPoint_Parameter(&point).unwrap_or(f64::NAN)
}

/// `Shape.project(tools)` along the target's normals.
pub(crate) fn project_normal(
    target: &TopoDS_Shape,
    tools: &TopTools_ListOfShape,
) -> Result<UniquePtr<TopoDS_Shape>, OcctError> {
    if target.IsNull() || tools.IsEmpty() {
        return Err(OcctError::NotDone {
            operation: "project",
        });
    }
    let mut projection = ffi::b_rep_algo::BRepAlgo_NormalProjection_new(target).map_err(|e| {
        OcctError::Exception {
            operation: "project",
            message: e.what().to_string(),
        }
    })?;
    let mut it = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(tools);
    while it.More() {
        projection.pin_mut().Add(it.Value());
        it.pin_mut().Next();
    }
    ffi::b_rep_algo::BRepAlgo_NormalProjection_Compute3d(projection.pin_mut(), true).map_err(|e| {
        OcctError::Exception {
            operation: "project",
            message: e.what().to_string(),
        }
    })?;
    projection.pin_mut().SetLimit(true);
    projection.pin_mut().SetParams(
        1.0e-6,
        1.0e-6,
        ffi::geom_api::GeomAbs_Shape::GeomAbs_C1,
        14,
        10000,
    );
    ffi::b_rep_algo::BRepAlgo_NormalProjection_Build(projection.pin_mut()).map_err(|e| {
        OcctError::Exception {
            operation: "project",
            message: e.what().to_string(),
        }
    })?;
    if !projection.IsDone() {
        return Err(OcctError::NotDone {
            operation: "project",
        });
    }
    let projected = projection.Projection();
    if projected.IsNull() {
        return Err(OcctError::NotDone {
            operation: "project",
        });
    }
    Ok(ffi::topo_ds::TopoDS_Shape_to_owned(projected))
}

/// A copy of `shape` with the numeric `TopAbs_Orientation` applied.
pub(crate) fn with_orientation(
    shape: &TopoDS_Shape,
    orientation: i32,
) -> UniquePtr<TopoDS_Shape> {
    let orientation = match orientation {
        0 => TopAbs_Orientation::TopAbs_FORWARD,
        1 => TopAbs_Orientation::TopAbs_REVERSED,
        2 => TopAbs_Orientation::TopAbs_INTERNAL,
        3 => TopAbs_Orientation::TopAbs_EXTERNAL,
        _ => return UniquePtr::null(),
    };
    if shape.IsNull() {
        return UniquePtr::null();
    }
    let mut copy = ffi::topo_ds::TopoDS_Shape_to_owned(shape);
    copy.pin_mut().set_orientation(orientation);
    copy
}

/// The sub-shapes of a numeric `TopAbs_ShapeEnum` as a compound.
pub(crate) fn shape_sub_shapes_of_type(
    shape: &TopoDS_Shape,
    kind: TopAbs_ShapeEnum,
) -> UniquePtr<TopoDS_Shape> {
    crate::policy::shape_sub_shapes_of_type(shape, kind)
}

// ---------------------------------------------------------------------------
// Discretisation
// ---------------------------------------------------------------------------

enum Adaptor {
    Curve(UniquePtr<ffi::b_rep_adaptor::BRepAdaptor_Curve>),
    CompCurve(UniquePtr<ffi::b_rep_adaptor::BRepAdaptor_CompCurve>),
}

impl Adaptor {
    fn value(&self, u: f64) -> UniquePtr<ffi::gp::gp_Pnt> {
        match self {
            Adaptor::Curve(curve) => ffi::b_rep_adaptor::BRepAdaptor_Curve_value(curve, u),
            Adaptor::CompCurve(curve) => ffi::b_rep_adaptor::BRepAdaptor_CompCurve_value(curve, u),
        }
    }
}

fn point_vec(point: &ffi::gp::gp_Pnt) -> Vector3 {
    Vector3::new(point.X(), point.Y(), point.Z())
}

#[allow(clippy::too_many_arguments)]
fn discretize(
    adapt: &Adaptor,
    mode: i32,
    value: f64,
    curvature: f64,
    minimum: i32,
    first: f64,
    last: f64,
) -> Vec<Vector3> {
    let mut out = Vec::new();
    match mode {
        0 | 1 => {
            if (mode == 0 && value < 2.0) || (mode == 1 && !(value > 0.0)) {
                return out;
            }
            let mut discretizer = ffi::gc_pnts::GCPnts_UniformAbscissa_new();
            let result = if mode == 0 {
                match adapt {
                    Adaptor::Curve(curve) => ffi::gc_pnts::GCPnts_UniformAbscissa_Initialize_number(
                        discretizer.pin_mut(),
                        curve,
                        value as i32,
                        first,
                        last,
                    ),
                    Adaptor::CompCurve(curve) => {
                        ffi::gc_pnts::GCPnts_UniformAbscissa_Initialize_number_comp(
                            discretizer.pin_mut(),
                            curve,
                            value as i32,
                            first,
                            last,
                        )
                    }
                }
            } else {
                match adapt {
                    Adaptor::Curve(curve) => {
                        ffi::gc_pnts::GCPnts_UniformAbscissa_Initialize_abscissa_range(
                            discretizer.pin_mut(),
                            curve,
                            value,
                            first,
                            last,
                        )
                    }
                    Adaptor::CompCurve(curve) => {
                        ffi::gc_pnts::GCPnts_UniformAbscissa_Initialize_abscissa_range_comp(
                            discretizer.pin_mut(),
                            curve,
                            value,
                            first,
                            last,
                        )
                    }
                }
            };
            if result.is_err() || !discretizer.IsDone() {
                return out;
            }
            let Ok(count) = ffi::gc_pnts::GCPnts_UniformAbscissa_NbPoints(&discretizer) else {
                return out;
            };
            if count <= 0 {
                return out;
            }
            for index in 1..=count {
                let Ok(parameter) = ffi::gc_pnts::GCPnts_UniformAbscissa_Parameter(&discretizer, index)
                else {
                    return out;
                };
                out.push(point_vec(&adapt.value(parameter)));
            }
        }
        2 => {
            if !(value > 0.0) {
                return out;
            }
            let discretizer = match adapt {
                Adaptor::Curve(curve) => {
                    ffi::gc_pnts::GCPnts_UniformDeflection_new(curve, value, first, last)
                }
                Adaptor::CompCurve(curve) => {
                    ffi::gc_pnts::GCPnts_UniformDeflection_new_comp(curve, value, first, last)
                }
            };
            let Ok(discretizer) = discretizer else {
                return out;
            };
            if !discretizer.IsDone() {
                return out;
            }
            let Ok(count) = ffi::gc_pnts::GCPnts_UniformDeflection_NbPoints(&discretizer) else {
                return out;
            };
            if count <= 0 {
                return out;
            }
            for index in 1..=count {
                let Ok(point) = ffi::gc_pnts::GCPnts_UniformDeflection_Value(&discretizer, index)
                else {
                    return out;
                };
                out.push(point_vec(&point));
            }
        }
        3 => {
            if !(value > 0.0) || !(curvature >= 0.0) {
                return out;
            }
            let min_points = if minimum > 0 { minimum } else { 2 };
            let discretizer = match adapt {
                Adaptor::Curve(curve) => ffi::gc_pnts::GCPnts_TangentialDeflection_new_range(
                    curve,
                    first,
                    last,
                    value,
                    curvature,
                    min_points,
                ),
                Adaptor::CompCurve(curve) => {
                    ffi::gc_pnts::GCPnts_TangentialDeflection_new_range_comp(
                        curve,
                        first,
                        last,
                        value,
                        curvature,
                        min_points,
                    )
                }
            };
            let Ok(discretizer) = discretizer else {
                return out;
            };
            let count = discretizer.NbPoints();
            if count <= 0 {
                return out;
            }
            for index in 1..=count {
                let point = ffi::gc_pnts::GCPnts_TangentialDeflection_Value(&discretizer, index);
                out.push(point_vec(&point));
            }
        }
        _ => {}
    }
    out
}

/// `Edge.discretize` as typed points.
pub(crate) fn edge_discretize(
    edge: &TopoDS_Shape,
    mode: i32,
    value: f64,
    curvature: f64,
    minimum: i32,
    first: f64,
    last: f64,
) -> Vec<Vector3> {
    if edge.IsNull() || edge.ShapeType() != TopAbs_ShapeEnum::TopAbs_EDGE {
        return Vec::new();
    }
    let adapt = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(TopoDS::Edge(edge));
    let first_parameter = if first.is_finite() {
        first
    } else {
        adapt.FirstParameter()
    };
    let last_parameter = if last.is_finite() {
        last
    } else {
        adapt.LastParameter()
    };
    if !(first_parameter < last_parameter) {
        return Vec::new();
    }
    discretize(
        &Adaptor::Curve(adapt),
        mode,
        value,
        curvature,
        minimum,
        first_parameter,
        last_parameter,
    )
}

/// `Wire.discretize` as typed points.
pub(crate) fn wire_discretize(
    wire: &TopoDS_Shape,
    mode: i32,
    value: f64,
    curvature: f64,
    minimum: i32,
    first: f64,
    last: f64,
) -> Vec<Vector3> {
    if wire.IsNull() || wire.ShapeType() != TopAbs_ShapeEnum::TopAbs_WIRE {
        return Vec::new();
    }
    let adapt = ffi::b_rep_adaptor::BRepAdaptor_CompCurve_new(TopoDS::Wire(wire));
    let first_parameter = if first.is_finite() {
        first
    } else {
        adapt.FirstParameter()
    };
    let last_parameter = if last.is_finite() {
        last
    } else {
        adapt.LastParameter()
    };
    if !(first_parameter < last_parameter) {
        return Vec::new();
    }
    discretize(
        &Adaptor::CompCurve(adapt),
        mode,
        value,
        curvature,
        minimum,
        first_parameter,
        last_parameter,
    )
}

// ---------------------------------------------------------------------------
// B-spline surface interpolation (shared with `backend.rs`)
// ---------------------------------------------------------------------------

fn grid_points(rows: &[Vec<Vector3>]) -> Result<UniquePtr<ffi::t_col_gp::TColgp_Array2OfPnt>, OcctError> {
    let nx = rows.len();
    let ny = rows.first().map_or(0, Vec::len);
    if nx < 2 || ny < 2 || rows.iter().any(|row| row.len() != ny) {
        return Err(OcctError::InvalidInput {
            what: "a B-spline surface needs a rectangular grid of at least two by two points",
        });
    }
    let mut points = ffi::t_col_gp::TColgp_Array2OfPnt_new(1, nx as i32, 1, ny as i32);
    for (u, row) in rows.iter().enumerate() {
        for (v, point) in row.iter().enumerate() {
            let pnt = ffi::gp::new_point(point.x, point.y, point.z);
            points
                .pin_mut()
                .SetValue((u + 1) as i32, (v + 1) as i32, &pnt);
        }
    }
    Ok(points)
}

fn interpolate_bspline(
    rows: &[Vec<Vector3>],
) -> Result<UniquePtr<ffi::geom::Handle_Geom_BSplineSurface>, OcctError> {
    let points = grid_points(rows)?;
    let mut interpolation = ffi::geom_api::GeomAPI_PointsToBSplineSurface_new();
    ffi::geom_api::GeomAPI_PointsToBSplineSurface_Interpolate(
        interpolation.pin_mut(),
        &points,
        false,
    )
    .map_err(|e| OcctError::Exception {
        operation: "bspline_surface_interpolate",
        message: e.what().to_string(),
    })?;
    let surface = ffi::geom_api::GeomAPI_PointsToBSplineSurface_Surface(&interpolation);
    let bspline = ffi::geom::downcast_to_bspline_surface(&surface);
    if bspline.IsNull() {
        return Err(OcctError::NotDone {
            operation: "bspline_surface_interpolate",
        });
    }
    Ok(bspline)
}

fn expanded_knots(sequence: &ffi::t_col_std::TColStd_Array1OfReal) -> Vec<f64> {
    (1..=sequence.Length())
        .map(|i| ffi::t_col_std::TColStd_Array1OfReal_Value(sequence, i))
        .collect()
}

/// `BSplineSurface.interpolate`: bounds and expanded knot sequences.
pub(crate) fn bspline_surface_interpolate(
    rows: &[Vec<Vector3>],
) -> Result<BsplineSurfaceData, GeomError> {
    let bspline = interpolate_bspline(rows)?;
    let u_knots = expanded_knots(ffi::geom::HandleGeomBSplineSurface_UKnotSequence(&bspline));
    let v_knots = expanded_knots(ffi::geom::HandleGeomBSplineSurface_VKnotSequence(&bspline));
    Ok(BsplineSurfaceData {
        u_bounds: (
            ffi::geom::HandleGeomBSplineSurface_UKnot(&bspline, 1),
            ffi::geom::HandleGeomBSplineSurface_UKnot(
                &bspline,
                ffi::geom::HandleGeomBSplineSurface_NbUKnots(&bspline),
            ),
        ),
        v_bounds: (
            ffi::geom::HandleGeomBSplineSurface_VKnot(&bspline, 1),
            ffi::geom::HandleGeomBSplineSurface_VKnot(
                &bspline,
                ffi::geom::HandleGeomBSplineSurface_NbVKnots(&bspline),
            ),
        ),
        u_knots,
        v_knots,
    })
}

/// The face of the interpolating B-spline surface through `rows`.
pub(crate) fn bspline_surface_face_from_points(
    rows: &[Vec<Vector3>],
) -> Result<UniquePtr<TopoDS_Shape>, GeomError> {
    let bspline = interpolate_bspline(rows)?;
    let support = ffi::geom::bspline_to_surface(&bspline);
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_surface(
        &support,
        ffi::precision::Precision::Confusion(),
    );
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "bspline_surface_face",
        }
        .into());
    }
    Ok(ffi::topo_ds::TopoDS_Shape_to_owned(make.pin_mut().Shape()))
}
