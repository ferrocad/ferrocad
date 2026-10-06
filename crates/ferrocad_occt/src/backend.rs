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
use ferrocad_geom::{ElementRef, GeometryBackend, History, OpResult, Shape};
use ferrocad_types::Placement;
use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::top_tools::TopTools_IndexedMapOfShape;
use ffi::topo_ds::{TopoDS, TopoDS_Shape};

use crate::bridge;
use crate::error::OcctError;
use crate::shape::{downcast, erase, new_map, sub_shape_map};

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
    type Error = OcctError;

    fn make_box(&self, length: f64, width: f64, height: f64) -> Result<Shape, Self::Error> {
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let mut make =
            ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&origin, length, width, height);
        let shape = erase(make.pin_mut().Shape());
        if !make.IsDone() {
            return Err(OcctError::NotDone { operation: "make_box" });
        }
        Ok(shape)
    }

    fn fuse(&self, a: &Shape, b: &Shape) -> Result<OpResult, Self::Error> {
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
            return Err(OcctError::NotDone { operation: "fuse" });
        }
        let out_faces = sub_shape_map(op.pin_mut().Shape(), TopAbs_ShapeEnum::TopAbs_FACE);
        let history = build_history(sa, &out_faces, |face, mods, gens| {
            bridge::fc_brep_fuse_modified(op.pin_mut(), face, mods.pin_mut());
            bridge::fc_brep_fuse_generated(op.pin_mut(), face, gens.pin_mut());
            bridge::fc_brep_fuse_is_deleted(op.pin_mut(), face)
        });
        Ok(OpResult::new(erase(op.pin_mut().Shape()), history))
    }

    fn cut(&self, a: &Shape, b: &Shape) -> Result<OpResult, Self::Error> {
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
            return Err(OcctError::NotDone { operation: "cut" });
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
    ) -> Result<OpResult, Self::Error> {
        if edges.is_empty() {
            return Err(OcctError::InvalidInput {
                what: "fillet needs at least one edge",
            });
        }
        let shape_occt = downcast(shape)?;
        let shape_guard = shape_occt.borrow();
        let s = shape_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let mut op = ffi::b_rep_fillet_api::BRepFilletAPI_MakeFillet_new(s);
        for edge_ref in edges {
            let edge_occt = downcast(&edge_ref.shape)?;
            let edge_guard = edge_occt.borrow();
            let es = edge_guard
                .as_ref()
                .ok_or(OcctError::InvalidInput { what: "null edge" })?;
            op.pin_mut().add_edge(radius, TopoDS::Edge(es));
        }
        // `Shape()` triggers the build; `IsDone()` is meaningful afterwards.
        let out = erase(op.pin_mut().Shape());
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "fillet" });
        }
        // Fillet history is not built yet (the bridge would need the fillet class).
        Ok(OpResult::new(out, History::default()))
    }

    fn place(&self, shape: &Shape, placement: &Placement) -> Result<Shape, Self::Error> {
        let shape_occt = downcast(shape)?;
        let shape_guard = shape_occt.borrow();
        let s = shape_guard
            .as_ref()
            .ok_or(OcctError::InvalidInput { what: "null shape" })?;
        let trsf = transform_from(placement);
        let mut op = ffi::b_rep_builder_api::BRepBuilderAPI_Transform_new(s, &trsf, true);
        let out = erase(op.pin_mut().Shape());
        if !op.IsDone() {
            return Err(OcctError::NotDone { operation: "place" });
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
        ("Face", TopAbs_ShapeEnum::TopAbs_FACE),
        ("Edge", TopAbs_ShapeEnum::TopAbs_EDGE),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if let Ok(index) = rest.parse::<i32>() {
                return Some((kind, index));
            }
        }
    }
    None
}
