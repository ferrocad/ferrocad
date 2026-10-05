//! Rust OCCT spike using the real binding crate (`opencascade-sys` 0.3,
//! bschwind/opencascade-rs).
//!
//! It mirrors the C++ probe: build two boxes, fuse and cut them, count
//! sub-shapes, fillet an edge, and report what **history** the binding exposes.
//!
//! Stock `opencascade-sys` bridges no useful history, so this runs two modes:
//!
//!   * default: prints what the stock crate exposes (geometry + `Cut::Generated`);
//!   * `--features patched`: after applying `patch/0001-history-bridge.patch`, it
//!     also reads `Modified`/`Generated`/`Deleted` and `BRepTools_History`.
//!
//! See `../../docs/occt-history-spike.md` §7 and `README.md`.

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

/// Report the fuse history for one input sub-shape.
///
/// With the bridge patch, all six numbers are real. Without it, there is nothing
/// to call, so it prints why.
#[cfg(feature = "patched")]
fn report_fuse_history(
    fuse: &mut cxx::UniquePtr<ffi::b_rep_algo_api::BRepAlgoAPI_Fuse>,
    input: &TopoDS_Shape,
    who: &str,
) {
    let (mut modified, mut generated, mut deleted) = (0, 0, 0);
    let (mut h_modified, mut h_generated, mut h_removed) = (0, 0, 0);
    ffi::b_rep_algo_api::fc_brep_fuse_history(
        fuse.pin_mut(),
        input,
        &mut modified,
        &mut generated,
        &mut deleted,
        &mut h_modified,
        &mut h_generated,
        &mut h_removed,
    );
    println!(
        "fuse history[{who}]: Modified={modified} Generated={generated} Deleted={deleted} \
         | BRepTools_History: Modified={h_modified} Generated={h_generated} Removed={h_removed}"
    );
}

#[cfg(not(feature = "patched"))]
fn report_fuse_history(
    _fuse: &mut cxx::UniquePtr<ffi::b_rep_algo_api::BRepAlgoAPI_Fuse>,
    _input: &TopoDS_Shape,
    who: &str,
) {
    println!(
        "fuse history[{who}]: not exposed by stock opencascade-sys \
         (apply patch/0001-history-bridge.patch and run with --features patched)"
    );
}

fn main() {
    println!("OCCT history probe (Rust, opencascade-sys)\n==========================================");

    // --- primitives -----------------------------------------------------------
    let mut a = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(
        &ffi::gp::new_point(0.0, 0.0, 0.0),
        10.0,
        10.0,
        10.0,
    );
    let mut b = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(
        &ffi::gp::new_point(5.0, 0.0, 0.0),
        10.0,
        10.0,
        10.0,
    );
    let a_shape = a.pin_mut().Shape();
    let b_shape = b.pin_mut().Shape();

    println!("\n== inputs ==");
    print_counts("box a", a_shape);
    print_counts("box b", b_shape);

    // --- fuse -----------------------------------------------------------------
    let mut fuse = ffi::b_rep_algo_api::BRepAlgoAPI_Fuse_new(a_shape, b_shape);

    // History first, while the op is mutably borrowable (the `Shape()` borrow
    // below keeps `fuse` borrowed for the rest of the scope).
    println!("\n== fuse history ==");
    report_fuse_history(&mut fuse, a_shape, "box a");
    report_fuse_history(&mut fuse, b_shape, "box b");

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

    // --- the one history call the stock crate exposes -------------------------
    let mut cut = ffi::b_rep_algo_api::BRepAlgoAPI_Cut_new(a_shape, b_shape);
    let generated = cut.pin_mut().Generated(a_shape);
    println!("\n== stock history call ==");
    println!("BRepAlgoAPI_Cut::Generated(a) -> {} shape(s)", generated.Size());

    println!("\nConclusion: stock opencascade-sys builds geometry but exposes almost no");
    println!("history; patch/0001-history-bridge.patch adds the ~3 calls needed so the");
    println!("spike can read Modified/Generated/Deleted and BRepTools_History.");
}
