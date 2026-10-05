//! Rust OCCT spike using the real binding crate (`opencascade-sys` 0.3,
//! bschwind/opencascade-rs).
//!
//! It mirrors the C++ probe: build two boxes, fuse and cut them, count
//! sub-shapes, fillet an edge, and report what **history** the binding exposes.
//! The finding is the point, not the geometry.
//!
//! What the binding covers (verified against the 0.3.0 sources):
//!   * primitives, booleans, fillet/chamfer, `TopExp::MapShapes`, `TopoDS`
//!     downcasts, `TopTools_IndexedMapOfShape::{Extent,FindKey}`.
//! What it does NOT cover:
//!   * `BRepBuilderAPI_MakeShape::{Modified,IsDeleted}` and `BRepTools_History`.
//!     Only `BRepAlgoAPI_Cut::Generated` is bridged. So the binding can build
//!     geometry but cannot hand us the lineage an element map needs.
//!
//! See ../../docs/occt-history-spike.md §7 for the implication and the minimal
//! bridge addition this suggests.

use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::topo_ds::TopoDS_Shape;

/// Count the sub-shapes of `kind` using the binding's `TopExp::MapShapes`.
fn count(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> i32 {
    let mut map = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(shape, kind, map.pin_mut());
    map.Extent()
}

fn print_counts(label: &str, shape: &TopoDS_Shape) {
    println!(
        "{label}: {} solids, {} faces, {} edges, {} vertices",
        count(shape, TopAbs_ShapeEnum::TopAbs_SOLID),
        count(shape, TopAbs_ShapeEnum::TopAbs_FACE),
        count(shape, TopAbs_ShapeEnum::TopAbs_EDGE),
        count(shape, TopAbs_ShapeEnum::TopAbs_VERTEX),
    );
}

fn main() {
    println!("OCCT history probe (Rust, opencascade-sys)\n==========================================");

    // --- primitives -----------------------------------------------------------
    let mut a = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&ffi::gp::new_point(0.0, 0.0, 0.0), 10.0, 10.0, 10.0);
    let mut b = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(&ffi::gp::new_point(5.0, 0.0, 0.0), 10.0, 10.0, 10.0);
    let a_shape = a.pin_mut().Shape();
    let b_shape = b.pin_mut().Shape();

    println!("\n== inputs ==");
    print_counts("box a", a_shape);
    print_counts("box b", b_shape);

    // --- fuse -----------------------------------------------------------------
    let mut fuse = ffi::b_rep_algo_api::BRepAlgoAPI_Fuse_new(a_shape, b_shape);
    let fused = fuse.pin_mut().Shape();
    println!("\n== fuse output ==");
    print_counts("fused", fused);

    // --- fillet one edge ------------------------------------------------------
    let mut edges = ffi::top_exp::TopExp_Explorer_new(fused, TopAbs_ShapeEnum::TopAbs_EDGE);
    let mut fillet = ffi::b_rep_fillet_api::BRepFilletAPI_MakeFillet_new(fused);
    if edges.More() {
        let edge = ffi::topo_ds::TopoDS::Edge(edges.Current());
        fillet.pin_mut().add_edge(1.0, edge);
    }
    let rounded = fillet.pin_mut().Shape();
    println!("\n== fillet output ==");
    print_counts("filleted", rounded);

    // --- history: the one call the binding exposes ----------------------------
    // `Generated` is bridged only on `BRepAlgoAPI_Cut`, not on `Fuse`, and
    // `Modified`/`IsDeleted`/`BRepTools_History` are not bridged at all.
    let mut cut = ffi::b_rep_algo_api::BRepAlgoAPI_Cut_new(a_shape, b_shape);
    let generated = cut.pin_mut().Generated(a_shape);
    println!("\n== history exposed by opencascade-sys 0.3.0 ==");
    println!("BRepAlgoAPI_Cut::Generated(a) -> {} shape(s)", generated.Size());
    println!("BRepAlgoAPI_Fuse::Modified/Generated/IsDeleted -> not bridged");
    println!("BRepTools_History, BRepAlgoAPI_BuilderAlgo::History   -> not bridged");

    println!("\nConclusion: the binding builds geometry, but a stable element map");
    println!("needs ~a dozen extra bridge functions over BRepBuilderAPI_MakeShape");
    println!("(Modified/Generated/IsDeleted) and TopExp traversal. See the report.");
}
