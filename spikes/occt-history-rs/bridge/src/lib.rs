//! Sibling cxx bridge over OCCT, reusing `opencascade-sys`'s types.
//!
//! `opencascade-sys` 0.3 builds geometry but does not bridge shape *history*
//! (`Modified`/`Generated`/`IsDeleted`, and the element maps an element map needs).
//! Rather than patching or forking the crate, this adds a second
//! `#[cxx::bridge]` that declares the same OCCT types and links the same OCCT
//! libraries, so the two bridges share types at the ABI level.
//!
//! See `patch/0001-history-bridge.patch` for the equivalent change applied *inside*
//! the crate (the version to send upstream). This crate is how we consume it.

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("occt-history-bridge/include/fc_history.hxx");

        // Reuse the crate's opaque types. cxx shares these across bridges by path.
        type BRepAlgoAPI_Fuse = opencascade_sys::b_rep_algo_api::BRepAlgoAPI_Fuse;
        type BRepAlgoAPI_Cut = opencascade_sys::b_rep_algo_api::BRepAlgoAPI_Cut;
        type BRepAlgoAPI_Common = opencascade_sys::b_rep_algo_api::BRepAlgoAPI_Common;
        type TopoDS_Shape = opencascade_sys::topo_ds::TopoDS_Shape;
        type TopTools_IndexedMapOfShape = opencascade_sys::top_tools::TopTools_IndexedMapOfShape;

        /// `TopTools_IndexedMapOfShape::FindIndex`, which the crate does not bridge.
        fn fc_indexed_map_find_index(
            map: &TopTools_IndexedMapOfShape,
            shape: &TopoDS_Shape,
        ) -> i32;

        fn fc_brep_fuse_modified(
            op: Pin<&mut BRepAlgoAPI_Fuse>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_brep_fuse_generated(
            op: Pin<&mut BRepAlgoAPI_Fuse>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_brep_fuse_is_deleted(op: Pin<&mut BRepAlgoAPI_Fuse>, shape: &TopoDS_Shape) -> bool;

        fn fc_brep_cut_modified(
            op: Pin<&mut BRepAlgoAPI_Cut>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_brep_cut_generated(
            op: Pin<&mut BRepAlgoAPI_Cut>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_brep_cut_is_deleted(op: Pin<&mut BRepAlgoAPI_Cut>, shape: &TopoDS_Shape) -> bool;

        fn fc_brep_common_modified(
            op: Pin<&mut BRepAlgoAPI_Common>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_brep_common_generated(
            op: Pin<&mut BRepAlgoAPI_Common>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_brep_common_is_deleted(
            op: Pin<&mut BRepAlgoAPI_Common>,
            shape: &TopoDS_Shape,
        ) -> bool;
    }
}

pub use ffi::*;
