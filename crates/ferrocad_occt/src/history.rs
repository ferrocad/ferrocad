//! Rust-side composition over the generic OCCT bindings.
//!
//! These are the pieces that used to be C++ helpers in
//! `ferrocad_part_cxx::history`: iterating a boolean's `Modified`/`Generated`
//! list into an element map, walking a shape's direct children, and driving a
//! reusable `BRepAlgoAPI_Cut`. The sys crate now exposes the OCCT calls, so the
//! loops and the operation policy live here in Rust.

use std::pin::Pin;

use cxx::UniquePtr;
use opencascade_sys as ffi;

use ffi::b_rep_algo_api::BRepAlgoAPI_Cut;
use ffi::bop_algo::BOPAlgo_Operation;
use ffi::top_tools::{TopTools_IndexedMapOfShape, TopTools_ListOfShape};
use ffi::topo_ds::TopoDS_Shape;

type Map = TopTools_IndexedMapOfShape;

/// Add every shape in `list` to `out`, de-duplicating it (`Add` keeps the first
/// occurrence). This is the body of the old `fc_fill_modified`/`fc_fill_generated`.
///
/// `out` is expected to be empty: the call sites allocate a fresh map per probe.
pub(crate) fn collect(list: &TopTools_ListOfShape, out: &mut UniquePtr<Map>) {
    let mut iterator = ffi::top_tools::TopTools_ListIteratorOfListOfShape_new(list);
    while iterator.More() {
        out.pin_mut().Add(iterator.Value());
        iterator.pin_mut().Next();
    }
}

/// The *direct* children of `shape`, in `TopoDS_Iterator` order.
///
/// Unlike `TopExp::MapShapes`, this does not recurse.
pub(crate) fn children(shape: &TopoDS_Shape) -> Vec<UniquePtr<TopoDS_Shape>> {
    let mut out = Vec::new();
    let mut iterator = ffi::topo_ds::TopoDS_Iterator_new(shape);
    while iterator.More() {
        out.push(ffi::topo_ds::TopoDS_Shape_to_owned(iterator.Value()));
        iterator.pin_mut().Next();
    }
    out
}

/// `BRepAlgoAPI_Cut::SetOperation` from a fuse/cut flag.
pub(crate) fn set_operation(op: Pin<&mut BRepAlgoAPI_Cut>, fuse: bool) {
    let operation = if fuse {
        BOPAlgo_Operation::BOPAlgo_FUSE
    } else {
        BOPAlgo_Operation::BOPAlgo_CUT
    };
    op.SetOperation(operation);
}

/// `SetArguments` + `SetTools`.
pub(crate) fn configure_cut(
    mut op: Pin<&mut BRepAlgoAPI_Cut>,
    arguments: &TopTools_ListOfShape,
    tools: &TopTools_ListOfShape,
) {
    op.as_mut().SetArguments(arguments);
    op.as_mut().SetTools(tools);
}

/// `Build()` with a fresh default `Message_ProgressRange`.
pub(crate) fn build_cut(op: Pin<&mut BRepAlgoAPI_Cut>) {
    let range = ffi::message::Message_ProgressRange_new();
    op.Build(range.as_ref().expect("a fresh progress range"));
}
