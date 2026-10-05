//! Rust OCCT spike using the real binding crate (`opencascade-sys` 0.3,
//! bschwind/opencascade-rs).
//!
//! It mirrors the C++ probe: build two boxes, fuse and cut them, count
//! sub-shapes, fillet an edge, and report what **history** the binding exposes.
//!
//! Three modes:
//!
//!   * default: stock geometry plus the **element map** from the sibling bridge
//!     crate `occt-history-bridge`, which adds the missing history calls without
//!     patching `opencascade-sys`;
//!   * `--features patched`: additionally reads `Modified`/`Generated`/`Deleted` and
//!     `BRepTools_History` through `patch/0001-history-bridge.patch` applied inside the
//!     crate (the version to send upstream).
//!
//! See `../../docs/occt-history-spike.md` §7 and `README.md`.

use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::topo_ds::TopoDS_Shape;
// Sibling bridge: the history/element-map calls missing from `opencascade-sys`.
use occt_history_bridge as bridge;

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

/// Element-level lineage, via the sibling bridge crate.
///
/// For each face of `input`, the 1-based indices (in `result_faces`) of the result
/// faces it maps to. This is the granularity an element map needs; the whole-solid
/// query above gives nothing. Works against the **stock** crate.
fn report_element_map(
    fuse: &mut cxx::UniquePtr<ffi::b_rep_algo_api::BRepAlgoAPI_Fuse>,
    input: &TopoDS_Shape,
    result_faces: &ffi::top_tools::TopTools_IndexedMapOfShape,
    who: &str,
) {
    let mut input_faces = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(input, TopAbs_ShapeEnum::TopAbs_FACE, input_faces.pin_mut());
    let n = input_faces.Extent();
    println!("\n== fuse element map [{who}: {n} input faces] ==");

    for i in 1..=n {
        let face = input_faces.FindKey(i);

        let mut modified = ffi::top_tools::new_indexed_map_of_shape();
        bridge::fc_brep_fuse_modified(fuse.pin_mut(), face, modified.pin_mut());
        let mut generated = ffi::top_tools::new_indexed_map_of_shape();
        bridge::fc_brep_fuse_generated(fuse.pin_mut(), face, generated.pin_mut());

        let mut idx = Vec::new();
        for k in 1..=modified.Extent() {
            idx.push(bridge::fc_indexed_map_find_index(
                result_faces,
                modified.FindKey(k),
            ));
        }
        for k in 1..=generated.Extent() {
            idx.push(bridge::fc_indexed_map_find_index(
                result_faces,
                generated.FindKey(k),
            ));
        }
        idx.sort_unstable();
        idx.dedup();

        if idx.is_empty() {
            let deleted = bridge::fc_brep_fuse_is_deleted(fuse.pin_mut(), face);
            if deleted {
                println!("  face #{i} -> deleted");
            } else {
                // OCCT convention: an unchanged sub-shape is absent from
                // Modified()/Generated() and is expected to survive as-is.
                let same = bridge::fc_indexed_map_find_index(result_faces, face);
                if same > 0 {
                    println!("  face #{i} -> result face [{same}] (unchanged)");
                } else {
                    println!("  face #{i} -> no mapping and not deleted (absorbed)");
                }
            }
        } else {
            println!("  face #{i} -> result faces {idx:?}");
        }
    }
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

    // Element-level lineage. Build the result face index map first; the borrow of
    // `fuse` ends with this block, so the loop can borrow it mutably again.
    let result_faces = {
        let fused = fuse.pin_mut().Shape();
        let mut m = ffi::top_tools::new_indexed_map_of_shape();
        ffi::top_exp::TopExp::MapShapes(fused, TopAbs_ShapeEnum::TopAbs_FACE, m.pin_mut());
        m
    };
    report_element_map(&mut fuse, a_shape, &result_faces, "box a");
    report_element_map(&mut fuse, b_shape, &result_faces, "box b");

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
    println!("history; the sibling bridge crate adds the calls needed to read per-sub-shape");
    println!("Modified/Generated lineage against the unmodified crate, and");
    println!("patch/0001-history-bridge.patch is a minimal in-crate variant of the same idea.");
}
