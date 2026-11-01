//! FreeCAD's `TopoShape` operation policy, on top of the generic OCCT bindings.
//!
//! These rules come from FreeCAD, not from OCCT: `Mod/Part/App/TopoShape.cpp`
//! for `isClosed` and the shape tolerance, `FCBRepAlgoAPI_BooleanOperation` for
//! how a boolean operation is configured. They used to be C++ helpers
//! (`include/fc_policy.hxx`) only because the OCCT entry points they need were
//! not bridged; now they are plain Rust over `ferrocad_part_cxx`.

use std::pin::Pin;

use cxx::UniquePtr;

use ferrocad_part_cxx::{bnd, history, topology};
use opencascade_sys as ffi;

use ffi::b_rep_algo_api::BRepAlgoAPI_Cut;
use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::top_tools::{TopTools_IndexedMapOfShape, TopTools_ListOfShape};
use ffi::topo_ds::{TopoDS, TopoDS_Shape};

use crate::bridge;
use crate::shape::new_map;

/// Upstream `FCBRepAlgoAPI_BooleanOperation`'s constructor settings: run in
/// parallel and non-destructively, so the operands are not rewritten in place.
pub(crate) fn cut_prepare(mut op: Pin<&mut BRepAlgoAPI_Cut>) {
    history::fc_cut_set_run_parallel(op.as_mut(), true);
    history::fc_cut_set_non_destructive(op.as_mut(), true);
}

/// Upstream's automatic fuzzy value: the diagonal of the combined operand bounds
/// times `Precision::Confusion()` (`FuzzyHelper::getBooleanFuzzy` defaults its
/// factor to 1.0).
///
/// Reads the operands already installed on `op`, so it must run after
/// `fc_cut_configure`.
pub(crate) fn cut_set_auto_fuzzy(mut op: Pin<&mut BRepAlgoAPI_Cut>) {
    let mut bounds = ffi::bnd::Bnd_Box_new();
    add_bounds(history::fc_cut_arguments(op.as_ref().get_ref()), bounds.pin_mut());
    add_bounds(history::fc_cut_tools(op.as_ref().get_ref()), bounds.pin_mut());

    let diagonal = bnd::fc_bnd_box_square_extent(bounds.as_ref().unwrap()).sqrt();
    history::fc_cut_set_fuzzy_value(op.as_mut(), diagonal * bnd::fc_precision_confusion());
}

/// Extend `bounds` with every shape in `shapes`, using the triangulation when the
/// shape has one (`BRepBndLib::Add`'s default, which upstream also relies on).
fn add_bounds(shapes: &TopTools_ListOfShape, mut bounds: Pin<&mut ffi::bnd::Bnd_Box>) {
    for index in 1..=bridge::fc_list_size(shapes) {
        let shape = bridge::fc_list_value(shapes, index);
        ffi::b_rep_bnd_lib::BRepBndLib::Add(shape, bounds.as_mut(), true);
    }
}

/// Upstream `TopoShape::isClosed`: a shell, wire, edge, face or vertex asks
/// `BRep_Tool::IsClosed`; a solid or comp-solid requires every shell to be
/// closed; a compound also requires every face, wire, edge and vertex that is not
/// contained in a higher element to be closed.
pub(crate) fn is_closed(shape: &TopoDS_Shape) -> bool {
    if shape.IsNull() {
        return false;
    }
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_SHELL
        | TopAbs_ShapeEnum::TopAbs_WIRE
        | TopAbs_ShapeEnum::TopAbs_EDGE
        | TopAbs_ShapeEnum::TopAbs_FACE
        | TopAbs_ShapeEnum::TopAbs_VERTEX
        | TopAbs_ShapeEnum::TopAbs_SHAPE => topology::fc_shape_is_closed(shape),
        TopAbs_ShapeEnum::TopAbs_COMPSOLID | TopAbs_ShapeEnum::TopAbs_SOLID => {
            every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_SHELL,
                TopAbs_ShapeEnum::TopAbs_SHAPE,
            )
        }
        TopAbs_ShapeEnum::TopAbs_COMPOUND => {
            // Each of these asks only about sub-shapes that are not contained in
            // a higher element, which is what `to_avoid` selects.
            let mut closed = every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_SHELL,
                TopAbs_ShapeEnum::TopAbs_SHAPE,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_FACE,
                TopAbs_ShapeEnum::TopAbs_SHELL,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_WIRE,
                TopAbs_ShapeEnum::TopAbs_FACE,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_EDGE,
                TopAbs_ShapeEnum::TopAbs_WIRE,
            );
            closed &= every_sub_shape_closed(
                shape,
                TopAbs_ShapeEnum::TopAbs_VERTEX,
                TopAbs_ShapeEnum::TopAbs_EDGE,
            );
            closed
        }
        // `TopAbs_ShapeEnum` is an extern enum, so the compiler must allow an
        // unknown discriminant; a kind OCCT does not define is not closed.
        _ => false,
    }
}

/// Every sub-shape of type `to_find` that is not inside a `to_avoid` sub-shape
/// reports closed.
fn every_sub_shape_closed(
    shape: &TopoDS_Shape,
    to_find: TopAbs_ShapeEnum,
    to_avoid: TopAbs_ShapeEnum,
) -> bool {
    let mut found: UniquePtr<TopTools_IndexedMapOfShape> = new_map();
    topology::fc_explore(shape, to_find, to_avoid, found.pin_mut());
    (1..=found.Extent()).all(|index| topology::fc_shape_is_closed(found.FindKey(index)))
}

/// The shape's geometric tolerance. A vertex, edge or face reports its own; any
/// higher shape reports the maximum over its sub-shapes, matching
/// `ShapeAnalysis_ShapeTolerance`'s default.
pub(crate) fn shape_tolerance(shape: &TopoDS_Shape) -> f64 {
    if shape.IsNull() {
        return 0.0;
    }
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_VERTEX
        | TopAbs_ShapeEnum::TopAbs_EDGE
        | TopAbs_ShapeEnum::TopAbs_FACE => leaf_tolerance(shape),
        _ => max_tolerance(shape, TopAbs_ShapeEnum::TopAbs_VERTEX)
            .max(max_tolerance(shape, TopAbs_ShapeEnum::TopAbs_EDGE))
            .max(max_tolerance(shape, TopAbs_ShapeEnum::TopAbs_FACE)),
    }
}

/// The largest tolerance among `shape`'s sub-shapes of `kind`, or 0.0 when it has
/// none.
fn max_tolerance(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> f64 {
    let mut found: UniquePtr<TopTools_IndexedMapOfShape> = new_map();
    topology::fc_explore(shape, kind, TopAbs_ShapeEnum::TopAbs_SHAPE, found.pin_mut());
    (1..=found.Extent())
        .map(|index| leaf_tolerance(found.FindKey(index)))
        .fold(0.0_f64, f64::max)
}

/// `BRep_Tool::Tolerance` of a vertex, edge or face. Those are the only shapes
/// that carry one, so anything else reports 0.0 - which is also what upstream
/// accumulates before walking the sub-shapes.
fn leaf_tolerance(shape: &TopoDS_Shape) -> f64 {
    match shape.ShapeType() {
        TopAbs_ShapeEnum::TopAbs_VERTEX => topology::fc_vertex_tolerance(TopoDS::Vertex(shape)),
        TopAbs_ShapeEnum::TopAbs_EDGE => topology::fc_edge_tolerance(TopoDS::Edge(shape)),
        TopAbs_ShapeEnum::TopAbs_FACE => topology::fc_face_tolerance(TopoDS::Face(shape)),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_closed, shape_tolerance};
    use cxx::UniquePtr;
    use ferrocad_part_cxx::topology::fc_copy_shape;
    use opencascade_sys as ffi;
    use ffi::topo_ds::TopoDS_Shape;

    fn make_box(size: f64) -> UniquePtr<TopoDS_Shape> {
        let origin = ffi::gp::new_point(0.0, 0.0, 0.0);
        let mut make = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&origin, size, size, size);
        let shape = fc_copy_shape(make.pin_mut().Shape());
        assert!(!shape.is_null(), "a box copy");
        shape
    }

    #[test]
    fn a_box_solid_is_closed_and_carries_a_tolerance() {
        let cube = make_box(10.0);
        assert!(is_closed(&cube), "a box is watertight");

        let tolerance = shape_tolerance(&cube);
        assert!(tolerance > 0.0, "tolerance was {tolerance}");
        assert!(tolerance < 1e-3, "tolerance was {tolerance}");
    }

    #[test]
    fn an_open_face_is_not_closed() {
        let cube = make_box(10.0);
        let mut faces = ffi::top_tools::new_indexed_map_of_shape();
        ferrocad_part_cxx::topology::fc_explore(
            cube.as_ref().unwrap(),
            ffi::top_abs::TopAbs_ShapeEnum::TopAbs_FACE,
            ffi::top_abs::TopAbs_ShapeEnum::TopAbs_SHAPE,
            faces.pin_mut(),
        );
        let face = faces.FindKey(1);
        assert!(!is_closed(face), "a box face is an open surface");
        assert!(shape_tolerance(face) > 0.0);
    }
}
