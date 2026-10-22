//! The OCCT implementation of [`ferrocad_geom::GeometryBackend`].
//!
//! First operations: [`make_box`](OcctBackend::make_box), fuse, cut, fillet and
//! place. Fuse/cut also build a face-level [`History`] from the sibling bridge's
//! `Modified`/`Generated`/`IsDeleted`, using the result face map to turn output
//! sub-shapes into indices (the element-level lineage the spike verified).
//!
//! Unchanged sub-shapes are deliberately **absent** from the history: an input that
//! is neither modified, generated, nor deleted simply survives as itself, and the
//! mapper treats an unmapped, non-deleted input that way. (Verified in
//! `spikes/occt-history-rs`.)

use cxx::UniquePtr;
use ferrocad_geom::{
    Capabilities, Continuity, ElementKind, ElementRef, GeomError, GeometryBackend, History,
    MassProperties, OpResult, Shape, Tessellation, TessellationOptions, unsupported,
};
use ferrocad_curves::{Arc, Circle, EdgeCurve, Ellipse, Line, Vector3};
use ferrocad_types::{Matrix4, Placement};
use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::top_tools::TopTools_IndexedMapOfShape;
use ffi::topo_ds::TopoDS_Shape;

use crate::bridge;
use crate::error::OcctError;
use crate::shape::{downcast, erase, new_map, sub_shape_map};
use crate::topology;

type Map = TopTools_IndexedMapOfShape;

/// A geometry kernel backed by OCCT.
#[derive(Debug, Default, Clone, Copy)]
pub struct OcctBackend;

impl OcctBackend {
    pub fn new() -> Self {
        OcctBackend
    }
}

impl GeometryBackend for OcctBackend {
    fn make_box(&self, length: f64, width: f64, height: f64) -> Result<Shape, GeomError> {
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let mut make =
            ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&origin, length, width, height);
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone { operation: "make_box" }.into());
        }
        Ok(shape)
    }

    fn make_cylinder(
        &self,
        radius: f64,
        height: f64,
        base: Vector3,
        direction: Vector3,
    ) -> Result<Shape, GeomError> {
        let axis = axis_from(base, direction)?;
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeCylinder_new(&axis, radius, height);
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_cylinder",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_sphere(&self, radius: f64, center: Vector3) -> Result<Shape, GeomError> {
        let origin = point(center);
        let direction = ffi::gp::gp_Dir_new(0.0, 0.0, 1.0);
        let axis = ffi::gp::gp_Ax2_new(&origin, &direction);
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeSphere_new(
            &axis,
            radius,
            std::f64::consts::TAU,
        );
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_sphere",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_cone(
        &self,
        radius1: f64,
        radius2: f64,
        height: f64,
        base: Vector3,
        direction: Vector3,
    ) -> Result<Shape, GeomError> {
        let axis = axis_from(base, direction)?;
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeCone_new(
            &axis,
            radius1,
            radius2,
            height,
            std::f64::consts::TAU,
        );
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_cone",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_edge(&self, curve: &EdgeCurve) -> Result<Shape, GeomError> {
        match curve {
            EdgeCurve::LineSegment(segment) => {
                let start = point(segment.start());
                let end = point(segment.end());
                let mut make =
                    ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_gp_Pnt_gp_Pnt(&start, &end);
                if !make.IsDone() {
                    return Err(OcctError::NotDone { operation: "make_edge" }.into());
                }
                Ok(erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge())))
            }
            EdgeCurve::Circle(circle) => edge_from_circle(circle),
            EdgeCurve::Arc(arc) => {
                if arc.is_closed() {
                    // A full turn is a circle; `GC_MakeArcOfCircle` needs three
                    // distinct points, and the mid point would coincide.
                    return edge_from_circle(&arc.circle());
                }
                let start = point(arc.start());
                let middle = point(arc.point_at(arc.first() + arc.sweep() / 2.0));
                let end = point(arc.end());
                let arc_maker = ffi::gc::GC_MakeArcOfCircle_point_point_point(
                    &start, &middle, &end,
                );
                let trimmed = ffi::gc::GC_MakeArcOfCircle_Value(&*arc_maker);
                if trimmed.is_null() {
                    return Err(OcctError::NotDone {
                        operation: "make_edge (arc)",
                    }
                    .into());
                }
                let curve = ffi::geom::new_HandleGeomCurve_from_HandleGeom_TrimmedCurve(&*trimmed);
                let mut make =
                    ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&*curve);
                if !make.IsDone() {
                    return Err(OcctError::NotDone {
                        operation: "make_edge (arc)",
                    }
                    .into());
                }
                Ok(erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge())))
            }
            EdgeCurve::Line(_) => Err(unsupported("make_edge for an infinite line")),
            EdgeCurve::Ellipse(_) => Err(unsupported(
                "make_edge for an ellipse (the OCCT bindings expose no gp_Elips)",
            )),
        }
    }

    fn make_wire(&self, edges: &[Shape]) -> Result<Shape, GeomError> {
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeWire_new();
        for edge in edges {
            let occt = downcast(edge)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null edge" })?;
            if !bridge::fc_is_edge(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_wire expects edges",
                }
                .into());
            }
            make.pin_mut().add_edge(bridge::fc_shape_as_edge(inner));
        }
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_wire",
            }
            .into());
        }
        Ok(erase(ffi::topo_ds::cast_wire_to_shape(make.pin_mut().Wire())))
    }

    fn make_vertex(&self, at: Vector3) -> Result<Shape, GeomError> {
        let pnt = point(at);
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeVertex_gp_Pnt(&pnt);
        Ok(erase(ffi::topo_ds::cast_vertex_to_shape(
            make.pin_mut().Vertex(),
        )))
    }

    fn vertex_point(&self, shape: &Shape) -> Option<Vector3> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_vertex(inner) {
            return None;
        }
        let pnt = ffi::b_rep::BRep_Tool_Pnt(bridge::fc_shape_as_vertex(inner));
        Some(Vector3::new(pnt.X(), pnt.Y(), pnt.Z()))
    }

    fn make_bspline_edge(
        &self,
        points: &[Vector3],
        periodic: bool,
    ) -> Result<Shape, GeomError> {
        if points.len() < 2 {
            return Err(OcctError::InvalidInput {
                what: "a B-spline needs at least two points",
            }
            .into());
        }
        let mut array = ffi::t_col_gp::TColgp_HArray1OfPnt_new(1, points.len() as i32);
        for (index, value) in points.iter().enumerate() {
            let value = point(*value);
            array.pin_mut().SetValue(index as i32 + 1, &value);
        }
        let array =
            ffi::t_col_gp::new_HandleTColgpHArray1OfPnt_from_TColgpHArray1OfPnt(array);
        let mut interpolate = ffi::geom_api::GeomAPI_Interpolate_new(&array, periodic, 1e-7);
        interpolate.pin_mut().Perform();
        let curve = ffi::geom_api::GeomAPI_Interpolate_Curve(&interpolate);
        let curve = ffi::geom::new_HandleGeomCurve_from_HandleGeom_BSplineCurve(&curve);
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(&curve);
        let shape = erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()));
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_bspline_edge",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_face(&self, wires: &[Shape]) -> Result<Shape, GeomError> {
        let (first, rest) = wires.split_first().ok_or(OcctError::InvalidInput {
            what: "a face needs a wire",
        })?;
        let first_occt = downcast(first)?;
        let first_guard = first_occt.borrow();
        let first_inner = first_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null wire" })?;
        if !bridge::fc_is_wire(first_inner) {
            return Err(OcctError::InvalidInput {
                what: "make_face expects wires",
            }
            .into());
        }
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(
            bridge::fc_shape_as_wire(first_inner),
            true,
        );
        // Adding a hole to an already-failed builder can crash OCCT, so stop as
        // soon as the outer wire does not make a face.
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_face",
            }
            .into());
        }
        for wire in rest {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_face expects wires",
                }
                .into());
            }
            make.pin_mut().add_wire(bridge::fc_shape_as_wire(inner));
            if !make.IsDone() {
                return Err(OcctError::NotDone {
                    operation: "make_face",
                }
                .into());
            }
        }
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_face",
            }
            .into());
        }
        Ok(erase(ffi::topo_ds::cast_face_to_shape(make.Face())))
    }

    fn make_faces(&self, wires: &[Shape]) -> Result<Shape, GeomError> {
        let mut faces = Vec::with_capacity(wires.len());
        for wire in wires {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_faces expects wires",
                }
                .into());
            }

            // Planar boundaries use the ordinary face builder; anything else is
            // filled through `BRepFill_Filling`.
            let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(
                bridge::fc_shape_as_wire(inner),
                true,
            );
            if make.IsDone() {
                faces.push(erase(ffi::topo_ds::cast_face_to_shape(make.Face())));
                continue;
            }

            let mut list = ffi::top_tools::new_list_of_shape();
            let _ = list.pin_mut().Append(inner);
            let filled = bridge::fc_fill_wires(&list);
            let Some(filled) = filled.as_ref() else {
                return Err(OcctError::NotDone {
                    operation: "make_faces",
                }
                .into());
            };
            faces.push(erase(filled));
        }
        match faces.len() {
            0 => Err(OcctError::InvalidInput {
                what: "a face needs a wire",
            }
            .into()),
            1 => Ok(faces.remove(0)),
            _ => self.make_compound(&faces),
        }
    }

    fn curve_of(&self, shape: &Shape) -> Result<EdgeCurve, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let data = bridge::fc_edge_curve(inner);
        if data.len() < 14 {
            return Err(unsupported("curve_of: the bridge returned no curve data"));
        }
        let origin = Vector3::new(data[1], data[2], data[3]);
        let direction = Vector3::new(data[4], data[5], data[6]);
        let x_axis = Vector3::new(data[7], data[8], data[9]);
        let (first, last) = (data[10], data[11]);
        let (major, minor) = (data[12], data[13]);
        match data[0] as i32 {
            1 => Ok(EdgeCurve::Line(Line::new(origin, direction)?)),
            2 => {
                let circle = Circle::with_axis(origin, direction, x_axis, major)?;
                if (last - first).abs() >= std::f64::consts::TAU - 1e-9 {
                    Ok(EdgeCurve::Circle(circle))
                } else {
                    Ok(EdgeCurve::Arc(Arc::new(circle, first, last)?))
                }
            }
            3 => Ok(EdgeCurve::Ellipse(Ellipse::with_axis(
                origin,
                direction,
                x_axis,
                major,
                minor,
            )?)),
            _ => Err(unsupported("curve_of: unsupported curve type")),
        }
    }

    fn fuse(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Fuse_new(sa, sb);
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "fuse" }.into());
        }
        let out_faces = sub_shape_map(op.pin_mut().Shape(), TopAbs_ShapeEnum::TopAbs_FACE);
        let history = build_history(sa, &out_faces, |face, mods, gens| {
            bridge::fc_brep_fuse_modified(op.pin_mut(), face, mods.pin_mut());
            bridge::fc_brep_fuse_generated(op.pin_mut(), face, gens.pin_mut());
            bridge::fc_brep_fuse_is_deleted(op.pin_mut(), face)
        });
        Ok(OpResult::new(erase(op.pin_mut().Shape()), history))
    }

    fn cut(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Cut_new(sa, sb);
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "cut" }.into());
        }
        let out_faces = sub_shape_map(op.pin_mut().Shape(), TopAbs_ShapeEnum::TopAbs_FACE);
        let history = build_history(sa, &out_faces, |face, mods, gens| {
            bridge::fc_brep_cut_modified(op.pin_mut(), face, mods.pin_mut());
            bridge::fc_brep_cut_generated(op.pin_mut(), face, gens.pin_mut());
            bridge::fc_brep_cut_is_deleted(op.pin_mut(), face)
        });
        Ok(OpResult::new(erase(op.pin_mut().Shape()), history))
    }

    fn fillet(
        &self,
        shape: &Shape,
        edges: &[ElementRef],
        radius: f64,
    ) -> Result<OpResult, GeomError> {
        topology::fillet(shape, edges, radius)
    }

    fn common(&self, a: &Shape, b: &Shape) -> Result<OpResult, GeomError> {
        let a_occt = downcast(a)?;
        let b_occt = downcast(b)?;
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        let sa = a_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sb = b_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut op = ffi::b_rep_algo_api::BRepAlgoAPI_Common_new(sa, sb);
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "common" }.into());
        }
        let out_faces = sub_shape_map(op.pin_mut().Shape(), TopAbs_ShapeEnum::TopAbs_FACE);
        let history = build_history(sa, &out_faces, |face, mods, gens| {
            bridge::fc_brep_common_modified(op.pin_mut(), face, mods.pin_mut());
            bridge::fc_brep_common_generated(op.pin_mut(), face, gens.pin_mut());
            bridge::fc_brep_common_is_deleted(op.pin_mut(), face)
        });
        Ok(OpResult::new(erase(op.pin_mut().Shape()), history))
    }

    fn make_compound(&self, shapes: &[Shape]) -> Result<Shape, GeomError> {
        if shapes.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a compound needs at least one shape",
            }
            .into());
        }
        let mut compound = ffi::topo_ds::TopoDS_Compound_new();
        let builder = ffi::b_rep::BRep_Builder_new();
        let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
        builder.MakeCompound(compound.pin_mut());

        // `TopoDS_Builder::Add(S, C)` takes the container first and the child
        // second; the cxx binding keeps that order (`shape` is the compound).
        let mut compound = ffi::topo_ds::TopoDS_Compound_as_shape(compound);
        for shape in shapes {
            let occt = downcast(shape)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            builder.Add(compound.pin_mut(), inner);
        }
        Ok(erase(&compound))
    }

    fn bounds(&self, shape: &Shape) -> Option<(Vector3, Vector3)> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut bnd = ffi::bnd::Bnd_Box_new();
        ffi::b_rep_bnd_lib::BRepBndLib::Add(inner, bnd.pin_mut(), false);
        if bnd.IsVoid() {
            return None;
        }
        // Drop the protective gap so the box is the tight one upstream reports.
        bnd.pin_mut().SetGap(0.0);
        let (mut xmin, mut ymin, mut zmin) = (0.0, 0.0, 0.0);
        let (mut xmax, mut ymax, mut zmax) = (0.0, 0.0, 0.0);
        bnd.Get(
            &mut xmin, &mut ymin, &mut zmin, &mut xmax, &mut ymax, &mut zmax,
        );
        Some((
            Vector3::new(xmin, ymin, zmin),
            Vector3::new(xmax, ymax, zmax),
        ))
    }

    fn mass_properties(&self, shape: &Shape) -> Option<MassProperties> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;

        let mut linear = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::LinearProperties(inner, linear.pin_mut(), false, false);
        let length = linear.Mass();

        let mut surface = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::SurfaceProperties(inner, surface.pin_mut(), false, false);
        let area = surface.Mass();

        let mut volume = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::VolumeProperties(
            inner,
            volume.pin_mut(),
            false,
            false,
            false,
        );
        let centre = ffi::g_prop::GProp_GProps_CentreOfMass(&volume);
        Some(MassProperties {
            length,
            area,
            volume: volume.Mass(),
            center_of_mass: Vector3::new(centre.X(), centre.Y(), centre.Z()),
        })
    }

    fn clean(&self, shape: &Shape) -> Option<Shape> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut op = ffi::shape_upgrade::UnifySameDomain_new(inner, true, true, false);
        op.pin_mut().build();
        Some(erase(op.shape()))
    }

    fn place(&self, shape: &Shape, placement: &Placement) -> Result<Shape, GeomError> {
        let shape_occt = downcast(shape)?;
        let shape_guard = shape_occt.borrow();
        let s = shape_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let trsf = transform_from(placement);
        let mut op = ffi::b_rep_builder_api::BRepBuilderAPI_Transform_new(s, &trsf, true);
        let out = erase(op.pin_mut().Shape());
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "place" }.into());
        }
        Ok(out)
    }

    fn transform(&self, shape: &Shape, matrix: &Matrix4) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut trsf = ffi::gp::new_transform();
        if !bridge::fc_trsf_from_matrix(&matrix.m, trsf.pin_mut()) {
            return Err(OcctError::InvalidInput {
                what: "matrix is not an affine transform",
            }
            .into());
        }
        let mut op = ffi::b_rep_builder_api::BRepBuilderAPI_Transform_new(inner, &trsf, true);
        let out = erase(op.pin_mut().Shape());
        if !op.IsDone() {
            return Err(OcctError::NotDone {
                operation: "transform",
            }
            .into());
        }
        Ok(out)
    }

    fn resolve(&self, shape: &Shape, name: &str) -> Option<Shape> {
        // Positional fallback only: `FaceN`/`EdgeN` -> the N-th sub-shape. Stable
        // *named* resolution needs the lineage mapper (a later slice); this mirrors
        // FreeCAD's behaviour when no element map is available.
        let shape_occt = downcast(shape).ok()?;
        let shape_guard = shape_occt.borrow();
        let s = shape_guard.as_ref()?;
        let (kind, index) = parse_sub_name(name)?;
        let map = sub_shape_map(s, kind);
        if index >= 1 && index <= map.Extent() {
            Some(erase(map.FindKey(index)))
        } else {
            None
        }
    }

    fn save_shape(&self, shape: &Shape) -> Vec<u8> {
        crate::shape::write_brep(shape).unwrap_or_default()
    }

    fn load_shape(&self, bytes: &[u8]) -> Option<Shape> {
        crate::shape::read_brep(bytes)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::BOOLEANS
            | Capabilities::FILLET
            | Capabilities::CHAMFER
            | Capabilities::TESSELLATION
            | Capabilities::TOPOLOGY
    }

    fn tessellate(
        &self,
        shape: &Shape,
        options: &TessellationOptions,
    ) -> Result<Tessellation, GeomError> {
        topology::tessellate(shape, options)
    }

    fn elements(&self, shape: &Shape, kind: ElementKind) -> Vec<ElementRef> {
        topology::elements(shape, kind)
    }

    fn ancestors(&self, shape: &Shape, element: &ElementRef, kind: ElementKind) -> Vec<ElementRef> {
        topology::ancestors(shape, element, kind)
    }

    fn continuity(&self, shape: &Shape, a: &ElementRef, b: &ElementRef) -> Option<Continuity> {
        topology::continuity(shape, a, b)
    }

    fn chamfer(
        &self,
        shape: &Shape,
        edges: &[ElementRef],
        d1: f64,
        d2: Option<f64>,
    ) -> Result<OpResult, GeomError> {
        topology::chamfer(shape, edges, d1, d2)
    }
}

/// Build a face-level [`History`] for a boolean op.
///
/// `probe` fills `mods`/`gens` for one input face and reports whether OCCT considers
/// it deleted; it captures the operation object (so the caller passes the op-specific
/// bridge functions).
fn build_history<F>(in_shape: &TopoDS_Shape, out_faces: &Map, mut probe: F) -> History
where
    F: FnMut(&TopoDS_Shape, &mut UniquePtr<Map>, &mut UniquePtr<Map>) -> bool,
{
    let in_faces = sub_shape_map(in_shape, TopAbs_ShapeEnum::TopAbs_FACE);
    let count = in_faces.Extent();
    let mut history = History::default();

    for i in 1..=count {
        let face = in_faces.FindKey(i);
        let mut mods = new_map();
        let mut gens = new_map();
        let deleted = probe(face, &mut mods, &mut gens);

        let mut mapped = false;
        for k in 1..=mods.Extent() {
            let out = mods.FindKey(k);
            let index = bridge::fc_indexed_map_find_index(out_faces, out);
            if index > 0 {
                history
                    .modified
                    .push((face_ref(i, face), face_ref(index, out)));
                mapped = true;
            }
        }
        for k in 1..=gens.Extent() {
            let out = gens.FindKey(k);
            let index = bridge::fc_indexed_map_find_index(out_faces, out);
            if index > 0 {
                history
                    .generated
                    .push((face_ref(i, face), face_ref(index, out)));
                mapped = true;
            }
        }
        if !mapped && deleted {
            history.deleted.push(face_ref(i, face));
        }
    }

    history
}

/// A `FaceN` element reference, owning a copy of the sub-shape.
fn face_ref(index: i32, shape: &TopoDS_Shape) -> ElementRef {
    ElementRef {
        shape: erase(shape),
        name: format!("Face{index}"),
    }
}

/// A `gp_Trsf` equivalent to a FerroCAD [`Placement`] (rotation about the origin,
/// then translation).
fn transform_from(placement: &Placement) -> UniquePtr<ffi::gp::gp_Trsf> {
    let mut trsf = ffi::gp::new_transform();
    let angle = placement.rotation.angle();
    if angle.abs() > 1e-12 {
        let axis = placement.rotation.axis();
        let dir = ffi::gp::gp_Dir_new(axis.x, axis.y, axis.z);
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let ax1 = ffi::gp::gp_Ax1_new(&origin, &dir);
        trsf.pin_mut().SetRotation(&ax1, angle);
    }
    let translation = ffi::gp::new_vec(placement.base.x, placement.base.y, placement.base.z);
    trsf.pin_mut().set_translation_vec(&translation);
    trsf
}

/// Parse a positional sub-element name (`Face6`, `Edge12`).
fn parse_sub_name(name: &str) -> Option<(TopAbs_ShapeEnum, i32)> {
    for (prefix, kind) in [
        ("Solid", TopAbs_ShapeEnum::TopAbs_SOLID),
        ("Shell", TopAbs_ShapeEnum::TopAbs_SHELL),
        ("Face", TopAbs_ShapeEnum::TopAbs_FACE),
        ("Wire", TopAbs_ShapeEnum::TopAbs_WIRE),
        ("Edge", TopAbs_ShapeEnum::TopAbs_EDGE),
        ("Vertex", TopAbs_ShapeEnum::TopAbs_VERTEX),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if let Ok(index) = rest.parse::<i32>() {
                return Some((kind, index));
            }
        }
    }
    None
}

/// A `gp_Pnt` from a [`Vector3`].
fn axis_from(base: Vector3, direction: Vector3) -> Result<UniquePtr<ffi::gp::gp_Ax2>, GeomError> {
    if direction.x == 0.0 && direction.y == 0.0 && direction.z == 0.0 {
        return Err(OcctError::InvalidInput {
            what: "a primitive needs a non-zero direction",
        }
        .into());
    }
    let origin = point(base);
    let direction = ffi::gp::gp_Dir_new(direction.x, direction.y, direction.z);
    Ok(ffi::gp::gp_Ax2_new(&origin, &direction))
}

fn point(vector: Vector3) -> UniquePtr<ffi::gp::gp_Pnt> {
    ffi::gp::new_point(vector.x, vector.y, vector.z)
}

/// A circular edge from the analytic circle.
///
/// The edge's parameterisation origin is OCCT's choice: `gp_Ax2` is built from a
/// point and a normal only, so the curve comes back with its own `x_axis` after
/// [`curve_of`](GeometryBackend::curve_of). The geometry (centre, normal, radius)
/// is what round-trips; an arc's angles are carried by its three points instead.
fn edge_from_circle(circle: &Circle) -> Result<Shape, GeomError> {
    let center = point(circle.center());
    let normal = ffi::gp::gp_Dir_new(circle.normal().x, circle.normal().y, circle.normal().z);
    let axis = ffi::gp::gp_Ax2_new(&center, &normal);
    let circ = ffi::gp::gp_Circ_new(&axis, circle.radius());
    let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_circle(&circ);
    if !make.IsDone() {
        return Err(OcctError::NotDone {
            operation: "make_edge (circle)",
        }
        .into());
    }
    Ok(erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge())))
}
