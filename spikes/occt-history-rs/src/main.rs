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

/// One `fc_brep_fuse_history` call, returned as six numbers:
/// `(Modified, Generated, Deleted, BRepTools_History::Modified, ..Generated, ..Removed)`.
#[cfg(feature = "patched")]
fn fuse_history_for(
    fuse: &mut cxx::UniquePtr<ffi::b_rep_algo_api::BRepAlgoAPI_Fuse>,
    sub_shape: &TopoDS_Shape,
) -> (i32, i32, i32, i32, i32, i32) {
    let (mut m, mut g, mut d) = (0, 0, 0);
    let (mut hm, mut hg, mut hr) = (0, 0, 0);
    ffi::b_rep_algo_api::fc_brep_fuse_history(
        fuse.pin_mut(),
        sub_shape,
        &mut m,
        &mut g,
        &mut d,
        &mut hm,
        &mut hg,
        &mut hr,
    );
    (m, g, d, hm, hg, hr)
}

/// Report the fuse history for one input shape.
///
/// Two queries, because they answer different questions: the whole solid is
/// consumed by the fuse (all its faces are replaced), so it reports
/// `Modified=0 Generated=0 Deleted=1`. The per-face query is the one that
/// carries the lineage an element map needs.
#[cfg(feature = "patched")]
fn report_fuse_history(
    fuse: &mut cxx::UniquePtr<ffi::b_rep_algo_api::BRepAlgoAPI_Fuse>,
    input: &TopoDS_Shape,
    who: &str,
) {
    let (m, g, d, hm, hg, hr) = fuse_history_for(fuse, input);
    println!(
        "fuse history[{who} solid]: Modified={m} Generated={g} Deleted={d} \
         | BRepTools_History: Modified={hm} Generated={hg} Removed={hr}"
    );

    let mut faces = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(input, TopAbs_ShapeEnum::TopAbs_FACE, faces.pin_mut());
    let n = faces.Extent();
    let (mut sm, mut sg, mut sd) = (0, 0, 0);
    let (mut shm, mut shg, mut shr) = (0, 0, 0);
    for i in 1..=n {
        let face = faces.FindKey(i);
        let (fm, fg, fd, fhm, fhg, fhr) = fuse_history_for(fuse, face);
        sm += fm;
        sg += fg;
        sd += fd;
        shm += fhm;
        shg += fhg;
        shr += fhr;
    }
    println!(
        "fuse history[{who} faces ({n})]: Modified={sm} Generated={sg} Deleted={sd} \
         | BRepTools_History: Modified={shm} Generated={shg} Removed={shr}"
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
    let edges = ffi::top_exp::TopExp_Explorer_new(fused, TopAbs_ShapeEnum::TopAbs_EDGE);
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
