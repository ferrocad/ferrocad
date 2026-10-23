//! The OCCT implementation of [`ferrocad_geom::GeometryBackend`].
//!
//! First operations: [`make_box`](OcctBackend::make_box), fuse, cut, fillet and
//! place. Fuse/cut/common build a [`History`] from the sibling bridge's
//! `Modified`/`Generated`/`IsDeleted` for **both** operands and all three element
//! kinds the kernel reports (faces, edges, vertices), using the result's map of
//! the same kind to turn output sub-shapes into positional names. Each relation
//! also carries an [`ElementId`] (operand, kind, 1-based ordinal), so two operands
//! that both report a `Face1` remain distinguishable.
//!
//! Unchanged sub-shapes are deliberately **absent** from the history: an input that
//! is neither modified, generated, nor deleted simply survives as itself, and the
//! mapper treats an unmapped, non-deleted input that way. (Verified in
//! `spikes/occt-history-rs`.)

use cxx::UniquePtr;
use ferrocad_geom::{
    Capabilities, Continuity, ElementId, ElementKind, ElementRef, GeomError, GeometryBackend,
    History, MassProperties, OpResult, OperandId, PrincipalProperties, Shape, Tessellation,
    TessellationOptions, unsupported,
};
use ferrocad_curves::{Arc, Circle, EdgeCurve, Ellipse, Line, Vector3};
use ferrocad_types::{Matrix4, Placement};
use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::top_abs::TopAbs_Orientation;
use ffi::top_tools::TopTools_IndexedMapOfShape;
use ffi::top_tools::TopTools_ListOfShape;
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

    /// Drop the dangling internal wires a fused region may carry. A wire other
    /// than the outer boundary that encloses no area is the trace of a cutter
    /// that terminated inside the face, not a hole; removing it leaves the
    /// region with a single clean boundary.
    fn without_dangling_wires(&self, shape: &Shape) -> Shape {
        let Ok(occt) = downcast(shape) else {
            return shape.clone();
        };
        let guard = occt.borrow();
        let Some(inner) = guard.as_ref() else {
            return shape.clone();
        };
        if !bridge::fc_is_face(inner) {
            return shape.clone();
        }
        let wires = sub_shape_map(inner, TopAbs_ShapeEnum::TopAbs_WIRE);
        if wires.Extent() <= 1 {
            return shape.clone();
        }
        let Some(outer) = self.outer_wire(shape) else {
            return shape.clone();
        };
        let Ok(outer_occt) = downcast(&outer) else {
            return shape.clone();
        };
        let outer_guard = outer_occt.borrow();
        let Some(outer_inner) = outer_guard.as_ref() else {
            return shape.clone();
        };
        let outer_index = bridge::fc_indexed_map_find_index_same(&wires, outer_inner);

        // A wire bounds an area only when it encloses a region on its own; probe
        // each one and count the ones that do not.
        let area = |shape: &Shape| {
            self.mass_properties(shape)
                .map(|mass| mass.area)
                .unwrap_or(0.0)
        };
        let mut dangling = 0;
        for index in 1..=wires.Extent() {
            if index == outer_index {
                continue;
            }
            let wire = erase(wires.FindKey(index));
            let enclosed = {
                let Ok(wire_occt) = downcast(&wire) else {
                    return shape.clone();
                };
                let wire_guard = wire_occt.borrow();
                let Some(wire_inner) = wire_guard.as_ref() else {
                    return shape.clone();
                };
                match bridge::fc_split_wire_into_faces(wire_inner).as_ref() {
                    Some(enclosed) => {
                        let faces = sub_shape_map(enclosed, TopAbs_ShapeEnum::TopAbs_FACE);
                        (1..=faces.Extent())
                            .map(|face| area(&erase(faces.FindKey(face))))
                            .sum::<f64>()
                    }
                    None => 0.0,
                }
            };
            if enclosed <= 1e-9 {
                dangling += 1;
            }
        }
        // Real holes must be kept; only a face whose extra wires are all
        // dangling can be rebuilt from its outer boundary alone.
        if dangling == 0 || dangling != wires.Extent() - 1 {
            return shape.clone();
        }
        let Ok(cleaned) = self.make_face(&[outer]) else {
            return shape.clone();
        };
        let cleaned_area = area(&cleaned);
        let original_area = area(shape);
        if cleaned_area > 0.0 && (cleaned_area - original_area).abs() <= 1e-9 * original_area.max(1.0)
        {
            cleaned
        } else {
            shape.clone()
        }
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

    fn make_bspline_poles(
        &self,
        poles: &[Vector3],
        degree: i32,
        periodic: bool,
    ) -> Result<Shape, GeomError> {
        if poles.len() < 2 {
            return Err(OcctError::InvalidInput {
                what: "a B-spline needs at least two poles",
            }
            .into());
        }
        let mut coords = Vec::with_capacity(poles.len() * 3);
        for pole in poles {
            coords.extend_from_slice(&[pole.x, pole.y, pole.z]);
        }
        let curve = bridge::fc_bspline_from_poles(&coords, degree, periodic);
        let Some(curve) = curve.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_bspline_poles",
            }
            .into());
        };
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeEdge_HandleGeomCurve(curve);
        let shape = erase(ffi::topo_ds::cast_edge_to_shape(make.pin_mut().Edge()));
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_bspline_poles",
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
            // A hole must run opposite to the outer boundary; reverse it so the
            // builder subtracts its area instead of adding another outer boundary.
            let reversed = bridge::fc_reversed(inner);
            let reversed = reversed.as_ref().ok_or(OcctError::InvalidInput {
                what: "null wire",
            })?;
            make.pin_mut().add_wire(bridge::fc_shape_as_wire(reversed));
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
        let mut splitters = Vec::new();
        let mut all_planar = true;
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

            // A self-intersecting boundary splits into several loops (one face
            // per loop); a simple boundary stays on the ordinary path below.
            let split = bridge::fc_split_wire_into_faces(inner);
            if let Some(split) = split.as_ref() {
                let map = sub_shape_map(split, TopAbs_ShapeEnum::TopAbs_FACE);
                if map.Extent() > 1 {
                    for index in 1..=map.Extent() {
                        faces.push(erase(map.FindKey(index)));
                    }
                    continue;
                }
            }

            let make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(
                bridge::fc_shape_as_wire(inner),
                true,
            );
            if make.IsDone() {
                faces.push(erase(ffi::topo_ds::cast_face_to_shape(make.Face())));
                continue;
            }
            // A boundary lying on an existing surface (for example a cylinder's
            // lateral wire) can still build a face when the builder is allowed
            // to find that surface.
            let make_any = ffi::b_rep_builder_api::BRepBuilderAPI_MakeFace_wire(
                bridge::fc_shape_as_wire(inner),
                false,
            );
            if make_any.IsDone() {
                faces.push(erase(ffi::topo_ds::cast_face_to_shape(make_any.Face())));
                continue;
            }

            let mut list = ffi::top_tools::new_list_of_shape();
            let _ = list.pin_mut().Append(inner);
            let filled = bridge::fc_fill_wires(&list);
            let Some(filled) = filled.as_ref() else {
                // An open boundary cannot bound a face; it still subdivides the
                // faces around it (dangling ends are pruned by the boolean).
                splitters.push(wire.clone());
                continue;
            };
            let filled = erase(filled);
            // `BRepFill_Filling` can also "succeed" on an open boundary, but only
            // with a degenerate face. Such a wire is a cutter, not a boundary.
            let filled_area = self
                .mass_properties(&filled)
                .map(|mass| mass.area)
                .unwrap_or(0.0);
            if filled_area <= 1e-9 {
                splitters.push(wire.clone());
                continue;
            }
            all_planar = false;
            faces.push(filled);
        }

        if !splitters.is_empty() {
            if faces.is_empty() {
                return Err(OcctError::InvalidInput {
                    what: "an open wire cannot form a face",
                }
                .into());
            }
            let mut list = ffi::top_tools::new_list_of_shape();
            for shape in faces.iter().chain(splitters.iter()) {
                let occt = downcast(shape)?;
                let guard = occt.borrow();
                if let Some(inner) = guard.as_ref() {
                    let _ = list.pin_mut().Append(inner);
                }
            }
            let fused = bridge::fc_general_fuse(&list);
            if let Some(fused) = fused.as_ref() {
                let map = sub_shape_map(fused, TopAbs_ShapeEnum::TopAbs_FACE);
                let mut regions = Vec::with_capacity(map.Extent() as usize);
                for index in 1..=map.Extent() {
                    let region = erase(map.FindKey(index));
                    // General Fuse keeps the degenerate slivers and the dangling
                    // cutters as regions of their own; neither bounds an area.
                    let area = self
                        .mass_properties(&region)
                        .map(|mass| mass.area)
                        .unwrap_or(0.0);
                    if area <= 1e-9 {
                        continue;
                    }
                    regions.push(self.without_dangling_wires(&region));
                }
                if !regions.is_empty() {
                    return if regions.len() == 1 {
                        Ok(regions.remove(0))
                    } else {
                        self.make_compound(&regions)
                    };
                }
            }
            return Err(OcctError::NotDone {
                operation: "make_faces",
            }
            .into());
        }

        // Coplanar faces that overlap or cross must be split into their regions;
        // General Fuse returns all parts, unlike a boolean union. Nested boundaries
        // are different: upstream keeps them as one face with holes.
        if all_planar && faces.len() > 1 {
            let area = |shape: &Shape| {
                self.mass_properties(shape)
                    .map(|mass| mass.area)
                    .unwrap_or(0.0)
            };
            let contains = |outer: &Shape, inner: &Shape| {
                let inner_area = area(inner);
                if inner_area <= 0.0 {
                    return false;
                }
                match self.common(outer, inner) {
                    Ok(result) => {
                        let overlap = area(&result.shape);
                        (overlap - inner_area).abs() <= 1e-6 * inner_area.max(1.0)
                    }
                    Err(_) => false,
                }
            };
            // Nesting follows the even-odd rule: the outermost boundary is a face,
            // its direct children are holes, their children are islands, and so on.
            let count = faces.len();
            let mut parent: Vec<Option<usize>> = vec![None; count];
            for inner in 0..count {
                for outer in 0..count {
                    if inner == outer || !contains(&faces[outer], &faces[inner]) {
                        continue;
                    }
                    let smaller = parent[inner]
                        .map(|current| area(&faces[outer]) < area(&faces[current]))
                        .unwrap_or(true);
                    if smaller {
                        parent[inner] = Some(outer);
                    }
                }
            }
            if parent.iter().any(Option::is_some) {
                let depth = |mut index: usize| {
                    let mut depth = 0;
                    while let Some(up) = parent[index] {
                        depth += 1;
                        index = up;
                        if depth > count {
                            break;
                        }
                    }
                    depth
                };
                let even: Vec<usize> = (0..count).filter(|index| depth(*index) % 2 == 0).collect();
                let odd: Vec<usize> = (0..count).filter(|index| depth(*index) % 2 == 1).collect();
                let intersection_area = |a: &Shape, b: &Shape| match self.common(a, b) {
                    Ok(result) => area(&result.shape),
                    Err(_) => 0.0,
                };

                // Positive faces that partially overlap must be unioned first; the
                // hole regions are then removed from the split result.
                let mut partial_overlap = false;
                'pairs: for (position, &a) in even.iter().enumerate() {
                    for &b in &even[position + 1..] {
                        let overlap = intersection_area(&faces[a], &faces[b]);
                        let smaller = area(&faces[a]).min(area(&faces[b]));
                        if overlap > 1e-9 && smaller - overlap > 1e-9 {
                            partial_overlap = true;
                            break 'pairs;
                        }
                    }
                }
                if !odd.is_empty() && partial_overlap {
                    // Include the hole boundaries too: General Fuse must split on
                    // them so there is a region inside each hole to discard.
                    let mut list = ffi::top_tools::new_list_of_shape();
                    for face in &faces {
                        let occt = downcast(face)?;
                        let guard = occt.borrow();
                        if let Some(inner) = guard.as_ref() {
                            let _ = list.pin_mut().Append(inner);
                        }
                    }
                    let fused = bridge::fc_general_fuse(&list);
                    if let Some(fused) = fused.as_ref() {
                        let map = sub_shape_map(fused, TopAbs_ShapeEnum::TopAbs_FACE);
                        let mut kept = Vec::new();
                        for index in 1..=map.Extent() {
                            let region = erase(map.FindKey(index));
                            let inside_hole = odd
                                .iter()
                                .any(|&hole| contains(&faces[hole], &region));
                            if !inside_hole {
                                kept.push(region);
                            }
                        }
                        if !kept.is_empty() {
                            return if kept.len() == 1 {
                                Ok(kept.remove(0))
                            } else {
                                self.make_compound(&kept)
                            };
                        }
                    }
                }

                let mut built = Vec::new();
                for node in 0..count {
                    if depth(node) % 2 != 0 {
                        continue;
                    }
                    let mut boundaries = vec![wires[node].clone()];
                    let mut holes: Vec<(f64, Shape)> = (0..count)
                        .filter(|&child| parent[child] == Some(node))
                        .map(|child| (area(&faces[child]), wires[child].clone()))
                        .collect();
                    holes.sort_by(|a, b| {
                        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    boundaries.extend(holes.into_iter().map(|(_, wire)| wire));
                    if let Ok(face) = self.make_face(&boundaries) {
                        built.push(face);
                    }
                }
                if !built.is_empty() {
                    return if built.len() == 1 {
                        Ok(built.remove(0))
                    } else {
                        self.make_compound(&built)
                    };
                }
            }

            let mut list = ffi::top_tools::new_list_of_shape();
            for face in &faces {
                let occt = downcast(face)?;
                let guard = occt.borrow();
                if let Some(inner) = guard.as_ref() {
                    let _ = list.pin_mut().Append(inner);
                }
            }
            let fused = bridge::fc_general_fuse(&list);
            if let Some(fused) = fused.as_ref() {
                let map = sub_shape_map(fused, TopAbs_ShapeEnum::TopAbs_FACE);
                let mut regions = Vec::with_capacity(map.Extent() as usize);
                for index in 1..=map.Extent() {
                    regions.push(erase(map.FindKey(index)));
                }
                if regions.len() > 1 {
                    return self.make_compound(&regions);
                }
            }
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

    fn surface_type(&self, shape: &Shape) -> Option<String> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let surface = ffi::b_rep::BRep_Tool_Surface(bridge::fc_shape_as_face(inner));
        let name = ffi::standard::type_name(ffi::geom::DynamicType(surface.as_ref()?));
        Some(
            match name.as_str() {
                "Geom_Plane" => "Part::GeomPlane",
                "Geom_CylindricalSurface" => "Part::GeomCylinder",
                "Geom_ConicalSurface" => "Part::GeomCone",
                "Geom_SphericalSurface" => "Part::GeomSphere",
                "Geom_ToroidalSurface" => "Part::GeomToroid",
                "Geom_BSplineSurface" => "Part::GeomBSplineSurface",
                _ => "Part::GeomSurface",
            }
            .to_string(),
        )
    }

    fn outer_wire(&self, shape: &Shape) -> Option<Shape> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        if !bridge::fc_is_face(inner) {
            return None;
        }
        let wire = ffi::b_rep_tools::outer_wire(bridge::fc_shape_as_face(inner));
        Some(erase(ffi::topo_ds::cast_wire_to_shape(wire.as_ref()?)))
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
        let out_maps = sub_shape_maps(op.pin_mut().Shape());
        let history = build_history(
            [(OperandId::A, sa), (OperandId::B, sb)],
            &out_maps,
            |sub, mods, gens| {
                bridge::fc_brep_fuse_modified(op.pin_mut(), sub, mods.pin_mut());
                bridge::fc_brep_fuse_generated(op.pin_mut(), sub, gens.pin_mut());
                bridge::fc_brep_fuse_is_deleted(op.pin_mut(), sub)
            },
        );
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
        let out_maps = sub_shape_maps(op.pin_mut().Shape());
        let history = build_history(
            [(OperandId::A, sa), (OperandId::B, sb)],
            &out_maps,
            |sub, mods, gens| {
                bridge::fc_brep_cut_modified(op.pin_mut(), sub, mods.pin_mut());
                bridge::fc_brep_cut_generated(op.pin_mut(), sub, gens.pin_mut());
                bridge::fc_brep_cut_is_deleted(op.pin_mut(), sub)
            },
        );
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
        let out_maps = sub_shape_maps(op.pin_mut().Shape());
        let history = build_history(
            [(OperandId::A, sa), (OperandId::B, sb)],
            &out_maps,
            |sub, mods, gens| {
                bridge::fc_brep_common_modified(op.pin_mut(), sub, mods.pin_mut());
                bridge::fc_brep_common_generated(op.pin_mut(), sub, gens.pin_mut());
                bridge::fc_brep_common_is_deleted(op.pin_mut(), sub)
            },
        );
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

    fn make_comp_solid(&self, solids: &[Shape]) -> Result<Shape, GeomError> {
        if solids.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a comp-solid needs at least one solid",
            }
            .into());
        }
        let mut list = ffi::top_tools::new_list_of_shape();
        for solid in solids {
            let occt = downcast(solid)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null solid" })?;
            let _ = list.pin_mut().Append(inner);
        }
        let composed = bridge::fc_comp_solid(&list);
        let Some(composed) = composed.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_comp_solid",
            }
            .into());
        };
        Ok(erase(composed))
    }

    fn make_solid(&self, shell: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shell)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shell" })?;
        if !bridge::fc_is_shell(inner) {
            return Err(OcctError::InvalidInput {
                what: "make_solid expects a shell",
            }
            .into());
        }
        let mut make = ffi::b_rep_builder_api::BRepBuilderAPI_MakeSolid_new(
            bridge::fc_shape_as_shell(inner),
        );
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_solid",
            }
            .into());
        }
        Ok(shape)
    }

    fn make_shell(&self, faces: &[Shape]) -> Result<Shape, GeomError> {
        if faces.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a shell needs at least one face",
            }
            .into());
        }
        let mut shell = ffi::topo_ds::TopoDS_Shell_new();
        let builder = ffi::b_rep::BRep_Builder_new();
        let builder = ffi::b_rep::BRep_Builder_upcast_to_topods_builder(&builder);
        builder.MakeShell(shell.pin_mut());
        let mut shell = ffi::topo_ds::TopoDS_Shell_as_shape(shell);
        for face in faces {
            let occt = downcast(face)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            if !bridge::fc_is_face(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_shell expects faces",
                }
                .into());
            }
            builder.Add(shell.pin_mut(), inner);
        }
        Ok(erase(&shell))
    }

    fn make_shell_from_wires(&self, wires: &[Shape]) -> Result<Shape, GeomError> {
        if wires.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "a shell needs at least one wire",
            }
            .into());
        }
        let mut faces = Vec::with_capacity(wires.len());
        for wire in wires {
            // Each wire bounds one planar face; upstream's wires are the
            // boundaries of a box's faces.
            faces.push(self.make_face(std::slice::from_ref(wire))?);
        }
        self.make_shell(&faces)
    }

    fn make_loft(&self, wires: &[Shape], solid: bool) -> Result<Shape, GeomError> {
        if wires.len() < 2 {
            return Err(OcctError::InvalidInput {
                what: "a loft needs at least two sections",
            }
            .into());
        }
        let mut make = ffi::b_rep_offset_api::BRepOffsetAPI_ThruSections_new(solid);
        for wire in wires {
            let occt = downcast(wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            if !bridge::fc_is_wire(inner) {
                return Err(OcctError::InvalidInput {
                    what: "make_loft expects wires",
                }
                .into());
            }
            make.pin_mut().AddWire(bridge::fc_shape_as_wire(inner));
        }
        // Upstream keeps the wires in the order given rather than letting OCCT
        // reorder them to match a common origin.
        make.pin_mut().CheckCompatibility(false);
        let progress = ffi::message::Message_ProgressRange_new();
        make.pin_mut().Build(&progress);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "make_loft",
            }
            .into());
        }
        Ok(erase(make.pin_mut().Shape()))
    }

    fn make_ruled_surface(&self, first: &Shape, second: &Shape) -> Result<Shape, GeomError> {
        let first_occt = downcast(first)?;
        let second_occt = downcast(second)?;
        let first_guard = first_occt.borrow();
        let second_guard = second_occt.borrow();
        let a = first_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let b = second_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        let face = bridge::fc_ruled_surface(a, b);
        let Some(face) = face.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_ruled_surface",
            }
            .into());
        };
        Ok(erase(face))
    }

    fn make_pipe(
        &self,
        spine: &Shape,
        profile: &Shape,
        tolerance: f64,
    ) -> Result<Shape, GeomError> {
        let spine_occt = downcast(spine)?;
        let spine_guard = spine_occt.borrow();
        let spine_inner = spine_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null spine" })?;
        let profile_occt = downcast(profile)?;
        let profile_guard = profile_occt.borrow();
        let profile_inner = profile_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null profile" })?;
        let piped = bridge::fc_pipe(spine_inner, profile_inner, tolerance);
        let Some(piped) = piped.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_pipe",
            }
            .into());
        };
        Ok(erase(piped))
    }

    fn make_filled_face(&self, edges: &[Shape]) -> Result<Shape, GeomError> {
        let wire = self.make_wire(edges)?;
        let mut list = ffi::top_tools::new_list_of_shape();
        {
            let occt = downcast(&wire)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null wire" })?;
            list.pin_mut().Append(inner);
        }
        let face = bridge::fc_fill_wires(&list);
        let Some(face) = face.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_filled_face",
            }
            .into());
        };
        Ok(erase(face))
    }

    fn extrude(&self, shape: &Shape, vector: Vector3) -> Result<Shape, GeomError> {
        if vector.length() <= 0.0 || !vector.length().is_finite() {
            return Err(OcctError::InvalidInput {
                what: "extrusion needs a non-zero vector",
            }
            .into());
        }
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let direction = ffi::gp::new_vec(vector.x, vector.y, vector.z);
        let mut make =
            ffi::b_rep_prim_api::BRepPrimAPI_MakePrism_new(inner, &direction, true, true);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "extrude",
            }
            .into());
        }
        Ok(erase(make.pin_mut().Shape()))
    }

    fn revolve(
        &self,
        shape: &Shape,
        center: Vector3,
        axis: Vector3,
        angle: f64,
    ) -> Result<Shape, GeomError> {
        if axis.length() <= 0.0 || !axis.length().is_finite() {
            return Err(OcctError::InvalidInput {
                what: "revolving needs a non-zero axis",
            }
            .into());
        }
        if !angle.is_finite() {
            return Err(OcctError::InvalidInput {
                what: "revolving needs a finite angle",
            }
            .into());
        }
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let origin = point(center);
        let direction = ffi::gp::gp_Dir_new(axis.x, axis.y, axis.z);
        let axis = ffi::gp::gp_Ax1_new(&origin, &direction);
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeRevol_new(inner, &axis, angle, true);
        if !make.IsDone() {
            return Err(OcctError::NotDone {
                operation: "revolve",
            }
            .into());
        }
        Ok(erase(make.pin_mut().Shape()))
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

    fn is_valid(&self, shape: &Shape) -> bool {
        let Ok(occt) = downcast(shape) else {
            return false;
        };
        let guard = occt.borrow();
        match guard.as_ref() {
            Some(inner) => bridge::fc_is_valid(inner),
            None => false,
        }
    }

    fn inertia_matrix(&self, shape: &Shape) -> Option<Matrix4> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut values = Vec::with_capacity(9);
        bridge::fc_inertia_matrix(inner, &mut values);
        if values.len() < 9 {
            return None;
        }
        let mut matrix = Matrix4::identity();
        for row in 0..3 {
            for column in 0..3 {
                matrix.m[row * 4 + column] = values[row * 3 + column];
            }
        }
        Some(matrix)
    }

    fn principal_properties(&self, shape: &Shape) -> Option<PrincipalProperties> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut values = Vec::with_capacity(17);
        bridge::fc_principal_properties(inner, &mut values);
        if values.len() < 17 {
            return None;
        }
        let axis = |offset: usize| {
            Vector3::new(values[offset], values[offset + 1], values[offset + 2])
        };
        Some(PrincipalProperties {
            symmetry_axis: values[0] != 0.0,
            symmetry_point: values[1] != 0.0,
            moments: (values[2], values[3], values[4]),
            axes: (axis(5), axis(8), axis(11)),
            radius_of_gyration: (values[14], values[15], values[16]),
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

    fn project(&self, shape: &Shape, tools: &[Shape]) -> Result<Shape, GeomError> {
        // This slice supports a planar target only, where a tool that already
        // lies in the target's plane projects onto itself. Other surfaces need
        // `BRepAlgo_NormalProjection`, which the seam does not expose yet.
        if self.surface_type(shape).as_deref() != Some("Part::GeomPlane") {
            return Err(unsupported(
                "project: only a planar target is supported (projection is the identity on it)",
            ));
        }
        if tools.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "project needs at least one tool shape",
            }
            .into());
        }
        self.make_compound(tools)
    }

    fn make_offset(&self, shape: &Shape, distance: f64, join: i32) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let offset = bridge::fc_offset(inner, distance, join);
        let Some(offset) = offset.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_offset",
            }
            .into());
        };
        Ok(erase(offset))
    }

    fn make_thickness(
        &self,
        shape: &Shape,
        closing_faces: &[Shape],
        thickness: f64,
        tolerance: f64,
    ) -> Result<Shape, GeomError> {
        if !thickness.is_finite() || thickness == 0.0 {
            return Err(OcctError::InvalidInput {
                what: "make_thickness needs a non-zero thickness",
            }
            .into());
        }
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        for face in closing_faces {
            let face_occt = downcast(face)?;
            let face_guard = face_occt.borrow();
            let face_inner = face_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            let _ = list.pin_mut().Append(face_inner);
        }
        let thickened = bridge::fc_thick_solid(inner, &list, thickness, tolerance);
        let Some(thickened) = thickened.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "make_thickness",
            }
            .into());
        };
        Ok(erase(thickened))
    }

    fn section(&self, a: &Shape, b: &Shape) -> Result<Shape, GeomError> {
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
        let section = bridge::fc_section(sa, sb);
        let Some(section) = section.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "section",
            }
            .into());
        };
        Ok(erase(section))
    }

    fn slice(
        &self,
        shape: &Shape,
        direction: Vector3,
        distance: f64,
    ) -> Result<Vec<Shape>, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let sliced = bridge::fc_slice(inner, direction.x, direction.y, direction.z, distance);
        let Some(sliced) = sliced.as_ref() else {
            return Err(OcctError::NotDone { operation: "slice" }.into());
        };
        let wires = sub_shape_map(sliced, TopAbs_ShapeEnum::TopAbs_WIRE);
        Ok((1..=wires.Extent())
            .map(|index| erase(wires.FindKey(index)))
            .collect())
    }

    fn general_fuse(&self, shapes: &[Shape]) -> Result<(Shape, Vec<Vec<Shape>>), GeomError> {
        if shapes.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "general fuse needs at least one shape",
            }
            .into());
        }
        let mut list = ffi::top_tools::new_list_of_shape();
        for shape in shapes {
            let occt = downcast(shape)?;
            let guard = occt.borrow();
            let inner = guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let _ = list.pin_mut().Append(inner);
        }
        let fused = bridge::fc_general_fuse(&list);
        let Some(fused) = fused.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "general_fuse",
            }
            .into());
        };
        let mut split = Vec::with_capacity(shapes.len());
        for index in 1..=shapes.len() as i32 {
            let modified = bridge::fc_general_fuse_modified(&list, index, 0.0);
            split.push(
                modified
                    .as_ref()
                    .map(|shape| top_level_parts(shape))
                    .unwrap_or_default(),
            );
        }
        Ok((erase(fused), split))
    }

    fn split_shape(
        &self,
        shape: &Shape,
        tools: &[(Shape, Shape)],
        check_interior: bool,
    ) -> Result<(Vec<Shape>, Vec<Shape>), GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;

        let mut cutters = ffi::top_tools::new_list_of_shape();
        let mut faces = ffi::top_tools::new_list_of_shape();
        for (cutter, face) in tools {
            let cutter_occt = downcast(cutter)?;
            let cutter_guard = cutter_occt.borrow();
            let cutter_inner = cutter_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null tool" })?;
            let face_occt = downcast(face)?;
            let face_guard = face_occt.borrow();
            let face_inner = face_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null face" })?;
            let _ = cutters.pin_mut().Append(cutter_inner);
            let _ = faces.pin_mut().Append(face_inner);
        }

        let mut direct = ffi::top_tools::new_list_of_shape();
        let mut left = ffi::top_tools::new_list_of_shape();
        if !bridge::fc_split_shape(
            inner,
            &cutters,
            &faces,
            check_interior,
            direct.pin_mut(),
            left.pin_mut(),
        ) {
            return Err(OcctError::NotDone {
                operation: "split_shape",
            }
            .into());
        }
        Ok((collect_list(&direct), collect_list(&left)))
    }

    fn optimal_bounds(
        &self,
        shape: &Shape,
        use_triangulation: bool,
        use_tolerance: bool,
    ) -> Option<(Vector3, Vector3)> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut values = Vec::with_capacity(6);
        if !bridge::fc_optimal_box(inner, use_triangulation, use_tolerance, &mut values) {
            return None;
        }
        if values.len() < 6 {
            return None;
        }
        Some((
            Vector3::new(values[0], values[1], values[2]),
            Vector3::new(values[3], values[4], values[5]),
        ))
    }

    fn same_shape(&self, a: &Shape, b: &Shape) -> bool {
        let (Ok(a_occt), Ok(b_occt)) = (downcast(a), downcast(b)) else {
            return a.is_same(b);
        };
        let a_guard = a_occt.borrow();
        let b_guard = b_occt.borrow();
        match (a_guard.as_ref(), b_guard.as_ref()) {
            (Some(a_inner), Some(b_inner)) => bridge::fc_is_same(a_inner, b_inner),
            _ => a.is_same(b),
        }
    }

    fn orientation(&self, shape: &Shape) -> Option<&'static str> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        Some(match inner.Orientation() {
            TopAbs_Orientation::TopAbs_REVERSED => "Reversed",
            TopAbs_Orientation::TopAbs_INTERNAL => "Internal",
            TopAbs_Orientation::TopAbs_EXTERNAL => "External",
            TopAbs_Orientation::TopAbs_FORWARD => "Forward",
            _ => "Forward",
        })
    }

    fn reversed(&self, shape: &Shape) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let reversed = bridge::fc_reversed(inner);
        let Some(reversed) = reversed.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "reversed",
            }
            .into());
        };
        Ok(erase(reversed))
    }

    fn remove_shapes(&self, shape: &Shape, shapes: &[Shape]) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut list = ffi::top_tools::new_list_of_shape();
        for removal in shapes {
            let removal_occt = downcast(removal)?;
            let removal_guard = removal_occt.borrow();
            let removal_inner = removal_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let _ = list.pin_mut().Append(removal_inner);
        }
        let result = bridge::fc_reshape_remove(inner, &list);
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "remove_shapes",
            }
            .into());
        };
        Ok(erase(result))
    }

    fn split_edge(&self, edge: &Shape, parameters: &[f64]) -> Result<Shape, GeomError> {
        let occt = downcast(edge)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null edge" })?;
        if !bridge::fc_is_edge(inner) {
            return Err(OcctError::InvalidInput {
                what: "split_edge needs an edge",
            }
            .into());
        }
        let split = bridge::fc_split_edge(inner, parameters);
        let Some(split) = split.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "split_edge",
            }
            .into());
        };
        Ok(erase(split))
    }

    fn replace_shapes(
        &self,
        shape: &Shape,
        replacements: &[(Shape, Shape)],
    ) -> Result<Shape, GeomError> {
        let occt = downcast(shape)?;
        let guard = occt.borrow();
        let inner = guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut from = ffi::top_tools::new_list_of_shape();
        let mut to = ffi::top_tools::new_list_of_shape();
        for (old, new) in replacements {
            let old_occt = downcast(old)?;
            let old_guard = old_occt.borrow();
            let old_inner = old_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let new_occt = downcast(new)?;
            let new_guard = new_occt.borrow();
            let new_inner = new_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null shape" })?;
            let _ = from.pin_mut().Append(old_inner);
            let _ = to.pin_mut().Append(new_inner);
        }
        let result = bridge::fc_reshape_replace(inner, &from, &to);
        let Some(result) = result.as_ref() else {
            return Err(OcctError::NotDone {
                operation: "replace_shapes",
            }
            .into());
        };
        Ok(erase(result))
    }

    fn static_moments(&self, shape: &Shape) -> Option<[f64; 3]> {
        let occt = downcast(shape).ok()?;
        let guard = occt.borrow();
        let inner = guard.as_ref()?;
        let mut props = ffi::g_prop::GProps_new();
        ffi::b_rep_g_prop::BRepGProp::VolumeProperties(
            inner,
            props.pin_mut(),
            false,
            false,
            false,
        );
        let (mut lx, mut ly, mut lz) = (0.0, 0.0, 0.0);
        props.StaticMoments(&mut lx, &mut ly, &mut lz);
        Some([lx, ly, lz])
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

/// The sub-shape kinds a boolean reports lineage for, coarsest first.
///
/// The order is fixed so a history is deterministic; it is not the order of the
/// relation lists (those stay grouped by input), only of enumeration.
const HISTORY_KINDS: [(ElementKind, TopAbs_ShapeEnum); 3] = [
    (ElementKind::Face, TopAbs_ShapeEnum::TopAbs_FACE),
    (ElementKind::Edge, TopAbs_ShapeEnum::TopAbs_EDGE),
    (ElementKind::Vertex, TopAbs_ShapeEnum::TopAbs_VERTEX),
];

/// The result's sub-shape maps, one per kind in [`HISTORY_KINDS`] order.
///
/// Built before the operation object is probed: the result shape borrows the
/// operation immutably, and `Modified`/`Generated` need it mutably, so the two
/// borrows cannot overlap.
fn sub_shape_maps(shape: &TopoDS_Shape) -> [(ElementKind, UniquePtr<Map>); 3] {
    HISTORY_KINDS.map(|(kind, occt)| (kind, sub_shape_map(shape, occt)))
}

/// Build the [`History`] of a boolean from **both** of its operands.
///
/// Every sub-shape of both operands is enumerated, for each of the three kinds the
/// kernel reports (`Face`, `Edge`, `Vertex`); `probe` fills `mods`/`gens` for one of
/// them and reports whether OCCT considers it deleted. An output is kept only if it
/// appears in the result's map of the same kind, so an element that does not survive
/// there is dropped, as before.
///
/// Each kept relation also records the input's [`ElementId`]: which operand it came
/// from, its kind, and its 1-based ordinal in that operand's map. Names stay
/// positional (`Face1`, `Edge3`, `Vertex8`), so one operand's `Face1` and another's
/// are told apart by identity rather than by spelling. An input with no surviving
/// output is reported deleted only when the kernel says so: unchanged elements
/// remain absent from all three lists.
fn build_history<F>(
    operands: [(OperandId, &TopoDS_Shape); 2],
    out_maps: &[(ElementKind, UniquePtr<Map>); 3],
    mut probe: F,
) -> History
where
    F: FnMut(&TopoDS_Shape, &mut UniquePtr<Map>, &mut UniquePtr<Map>) -> bool,
{
    let mut history = History::default();

    for (operand, in_shape) in operands {
        for (kind_index, (kind, occt_kind)) in HISTORY_KINDS.into_iter().enumerate() {
            let in_map = sub_shape_map(in_shape, occt_kind);
            let out_map = &out_maps[kind_index].1;

            for ordinal in 1..=in_map.Extent() {
                let element = in_map.FindKey(ordinal);
                let mut mods = new_map();
                let mut gens = new_map();
                let deleted = probe(element, &mut mods, &mut gens);
                let input_id = ElementId::new(operand, kind.prefix(), ordinal as usize);

                let mut mapped = false;
                for entry in 1..=mods.Extent() {
                    let out = mods.FindKey(entry);
                    let index = bridge::fc_indexed_map_find_index(out_map, out);
                    if index > 0 {
                        history.push_modified(
                            element_ref(kind, ordinal, element),
                            input_id.clone(),
                            element_ref(kind, index, out),
                        );
                        mapped = true;
                    }
                }
                for entry in 1..=gens.Extent() {
                    let out = gens.FindKey(entry);
                    let index = bridge::fc_indexed_map_find_index(out_map, out);
                    if index > 0 {
                        history.push_generated(
                            element_ref(kind, ordinal, element),
                            input_id.clone(),
                            element_ref(kind, index, out),
                        );
                        mapped = true;
                    }
                }
                if !mapped && deleted {
                    history.push_deleted(element_ref(kind, ordinal, element), input_id);
                }
            }
        }
    }

    // The identity side table is canonical: sorted by operand, kind, ordinal,
    // relation and output, so the order OCCT reported relations in cannot change
    // how a history compares. The relation lists above keep reporting order.
    history.provenance.sort();
    history
}

/// A positional element reference into an operand (`FaceN`, `EdgeN`, `VertexN`).
fn element_ref(kind: ElementKind, index: i32, shape: &TopoDS_Shape) -> ElementRef {
    ElementRef {
        shape: erase(shape),
        name: format!("{}{index}", kind.prefix()),
    }
}

/// The top-level parts of a compound: its solids when it has any, else its
/// faces, then edges, then wires (the split pieces General Fuse returns).
fn top_level_parts(shape: &TopoDS_Shape) -> Vec<Shape> {
    for kind in [
        TopAbs_ShapeEnum::TopAbs_SOLID,
        TopAbs_ShapeEnum::TopAbs_SHELL,
        TopAbs_ShapeEnum::TopAbs_FACE,
        TopAbs_ShapeEnum::TopAbs_EDGE,
        TopAbs_ShapeEnum::TopAbs_WIRE,
    ] {
        let map = sub_shape_map(shape, kind);
        if map.Extent() > 0 {
            return (1..=map.Extent())
                .map(|index| erase(map.FindKey(index)))
                .collect();
        }
    }
    Vec::new()
}

/// The shapes in an OCCT shape list, in the list's order.
fn collect_list(list: &UniquePtr<TopTools_ListOfShape>) -> Vec<Shape> {
    let Some(list) = list.as_ref() else {
        return Vec::new();
    };
    let count = bridge::fc_list_size(list);
    (1..=count)
        .map(|index| erase(bridge::fc_list_value(list, index)))
        .collect()
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

#[cfg(test)]
mod provenance_tests {
    //! Golden histories for the two-input booleans: both operands, all three
    //! element kinds, and the identity behind every reported relation.
    //!
    //! The counts are the OCCT 7.8 answer for two coincident-sized boxes, one offset
    //! by `(5, 5, 5)`. They are exact on purpose: this is the baseline S4 hands to
    //! the lineage layer, so a kernel or adapter change should be visible here first.

    use std::collections::{BTreeMap, BTreeSet};

    use ferrocad_geom::{ElementKind, GeometryBackend, History, OperandId, Relation};
    use ferrocad_types::Rotation;

    use super::*;

    /// Overlapping 10³ boxes: `a` at the origin, `b` shifted by `(5, 5, 5)`.
    fn overlapping_boxes(backend: &OcctBackend) -> (Shape, Shape) {
        let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let b = backend
            .place(
                &backend.make_box(10.0, 10.0, 10.0).unwrap(),
                &Placement {
                    base: Vector3::new(5.0, 5.0, 5.0),
                    rotation: Rotation::identity(),
                },
            )
            .unwrap();
        (a, b)
    }

    /// `(operand, kind, relation) -> count` over the provenance side table.
    fn counts(history: &History) -> BTreeMap<(OperandId, String, Relation), usize> {
        let mut counts = BTreeMap::new();
        for entry in &history.provenance {
            *counts
                .entry((
                    entry.input.operand(),
                    entry.input.kind().to_string(),
                    entry.relation,
                ))
                .or_insert(0) += 1;
        }
        counts
    }

    fn expected(entries: &[(OperandId, &str, Relation, usize)]) -> BTreeMap<(OperandId, String, Relation), usize> {
        entries
            .iter()
            .map(|(operand, kind, relation, count)| ((*operand, kind.to_string(), *relation), *count))
            .collect()
    }

    /// Split a positional name into its kind prefix and 1-based ordinal.
    fn split_name(name: &str) -> (String, usize) {
        for prefix in ["Face", "Edge", "Vertex"] {
            if let Some(rest) = name.strip_prefix(prefix) {
                let ordinal = rest.parse().expect("a positional ordinal");
                return (prefix.to_string(), ordinal);
            }
        }
        panic!("not a positional sub-element name: {name}");
    }

    /// The relation lists projected onto `(kind, ordinal, relation, output)`.
    ///
    /// The relation lists have no operand field, so this is compared with the
    /// provenance projection to check that every reported relation has exactly one
    /// identity entry; the per-operand counts above then check the operand split.
    fn relation_projection(history: &History) -> Vec<(String, usize, Relation, Option<String>)> {
        let mut entries = Vec::new();
        for (input, output) in &history.modified {
            let (kind, ordinal) = split_name(&input.name);
            entries.push((kind, ordinal, Relation::Modified, Some(output.name.clone())));
        }
        for (input, output) in &history.generated {
            let (kind, ordinal) = split_name(&input.name);
            entries.push((kind, ordinal, Relation::Generated, Some(output.name.clone())));
        }
        for input in &history.deleted {
            let (kind, ordinal) = split_name(&input.name);
            entries.push((kind, ordinal, Relation::Deleted, None));
        }
        entries.sort();
        entries
    }

    fn provenance_projection(history: &History) -> Vec<(String, usize, Relation, Option<String>)> {
        let mut entries: Vec<_> = history
            .provenance
            .iter()
            .map(|entry| {
                (
                    entry.input.kind().to_string(),
                    entry.input.ordinal(),
                    entry.relation,
                    entry.output.clone(),
                )
            })
            .collect();
        entries.sort();
        entries
    }

    /// The ordered names in each relation list, for run-to-run comparison.
    fn relation_names(
        history: &History,
    ) -> (Vec<(String, String)>, Vec<(String, String)>, Vec<String>) {
        (
            history
                .modified
                .iter()
                .map(|(input, output)| (input.name.clone(), output.name.clone()))
                .collect(),
            history
                .generated
                .iter()
                .map(|(input, output)| (input.name.clone(), output.name.clone()))
                .collect(),
            history.deleted.iter().map(|input| input.name.clone()).collect(),
        )
    }

    /// Every identity is in range for its operand and every output resolves in the
    /// result, under the same kind. Names stay positional, so the two must agree.
    fn assert_histories_are_coherent(
        backend: &OcctBackend,
        operands: [(&Shape, OperandId); 2],
        result: &Shape,
        history: &History,
    ) {
        let kinds = [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex];
        let total = |shape: &Shape, kind: ElementKind| backend.elements(shape, kind).len();

        let mut outputs = BTreeMap::new();
        for kind in kinds {
            outputs.insert(kind.prefix(), total(result, kind));
        }

        for entry in &history.provenance {
            let operand = operands
                .iter()
                .find(|(_, id)| *id == entry.input.operand())
                .map(|(shape, _)| *shape)
                .expect("a known operand");
            let kind = kinds
                .iter()
                .find(|kind| kind.prefix() == entry.input.kind())
                .copied()
                .expect("Face, Edge or Vertex");

            let input_total = total(operand, kind);
            assert!(
                (1..=input_total).contains(&entry.input.ordinal()),
                "{} {}#{} is outside the operand's {input_total} elements",
                entry.input.kind(),
                entry.input.operand().position(),
                entry.input.ordinal(),
            );

            if let Some(output) = &entry.output {
                let (output_kind, ordinal) = split_name(output);
                assert_eq!(
                    output_kind,
                    entry.input.kind(),
                    "an input keeps its kind in the result"
                );
                let output_total = outputs[output_kind.as_str()];
                assert!(
                    ordinal >= 1 && ordinal <= output_total,
                    "output {output} is outside the result's {output_total} {output_kind}s",
                );
            }
        }

        assert_eq!(
            provenance_projection(history),
            relation_projection(history),
            "every relation has exactly one identity entry",
        );

        let operand_ids: BTreeSet<_> = history
            .provenance
            .iter()
            .map(|entry| entry.input.operand())
            .collect();
        assert_eq!(
            operand_ids,
            BTreeSet::from([OperandId::A, OperandId::B]),
            "both operands are reported",
        );
    }

    #[test]
    fn fuse_reports_both_operands_and_all_three_kinds() {
        let backend = OcctBackend::new();
        let (a, b) = overlapping_boxes(&backend);
        let out = backend.fuse(&a, &b).unwrap();

        assert_eq!(
            counts(&out.history),
            expected(&[
                (OperandId::A, "Face", Relation::Modified, 3),
                (OperandId::A, "Edge", Relation::Modified, 3),
                (OperandId::A, "Vertex", Relation::Deleted, 1),
                (OperandId::B, "Face", Relation::Modified, 3),
                (OperandId::B, "Edge", Relation::Modified, 3),
                (OperandId::B, "Vertex", Relation::Deleted, 1),
            ]),
        );
        assert!(out.history.generated.is_empty(), "fuse generates no new elements");

        assert_histories_are_coherent(&backend, [(&a, OperandId::A), (&b, OperandId::B)], &out.shape, &out.history);

        // Identity is canonical: the side table is sorted even though the relation
        // lists stay in reporting order.
        let mut sorted = out.history.provenance.clone();
        sorted.sort();
        assert_eq!(sorted, out.history.provenance);
    }

    #[test]
    fn cut_reports_both_operands_and_all_three_kinds() {
        let backend = OcctBackend::new();
        let (a, b) = overlapping_boxes(&backend);
        let out = backend.cut(&a, &b).unwrap();

        assert_eq!(
            counts(&out.history),
            expected(&[
                (OperandId::A, "Face", Relation::Modified, 3),
                (OperandId::A, "Edge", Relation::Modified, 3),
                (OperandId::A, "Vertex", Relation::Deleted, 1),
                // The tool contributes the new walls; the rest of it is consumed.
                (OperandId::B, "Face", Relation::Modified, 3),
                (OperandId::B, "Face", Relation::Deleted, 3),
                (OperandId::B, "Edge", Relation::Modified, 3),
                (OperandId::B, "Edge", Relation::Deleted, 9),
                (OperandId::B, "Vertex", Relation::Deleted, 7),
            ]),
        );
        assert!(out.history.generated.is_empty(), "cut generates no new elements");

        assert_histories_are_coherent(&backend, [(&a, OperandId::A), (&b, OperandId::B)], &out.shape, &out.history);
    }

    #[test]
    fn the_same_boolean_twice_yields_the_same_history() {
        let backend = OcctBackend::new();
        let (a, b) = overlapping_boxes(&backend);

        for (first, second) in [
            (backend.fuse(&a, &b).unwrap(), backend.fuse(&a, &b).unwrap()),
            (backend.cut(&a, &b).unwrap(), backend.cut(&a, &b).unwrap()),
        ] {
            assert_eq!(
                first.history.provenance, second.history.provenance,
                "identity is deterministic",
            );
            assert_eq!(
                relation_names(&first.history),
                relation_names(&second.history),
                "the relation lists are deterministic",
            );
        }
    }
}
