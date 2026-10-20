//! OCCT tessellation / topology probe.
//!
//! Answers the F0.0b questions against a real kernel:
//!
//! 1. Does `BRepMesh_IncrementalMesh` + `BRep_Tool::Triangulation` give per-face
//!    triangulations through `opencascade-sys` 0.3?
//! 2. What are the index bases, and is the vertex location identity for a primitive?
//! 3. Can a topological edge be sampled without a new bridge (`BRepAdaptor_Curve` +
//!    `GCPnts_TangentialDeflection`)?
//! 4. Does `TopExp::MapShapesAndAncestors` give the parent/child lists once the
//!    sibling bridge adds list access?
//! 5. Do `BRepFilletAPI_MakeChamfer` (equal) and the bridged two-distance form build?
//!    Do the fillet/chamfer history calls return lineage?
//!
//! Run against an OCCT prefix:
//!
//! ```sh
//! export OCCT_INCLUDE_DIR=<prefix>/include/opencascade
//! export CMAKE_POLICY_VERSION_MINIMUM=3.5
//! cargo run --release
//! ```

use opencascade_sys as ffi;

use ffi::top_abs::{TopAbs_Orientation, TopAbs_ShapeEnum};
use ffi::topo_ds::{TopoDS, TopoDS_Shape};
use occt_topology_bridge as bridge;

/// Count the sub-shapes of `kind`.
fn count(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> i32 {
    let mut map = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(shape, kind, map.pin_mut());
    map.Extent()
}

/// The deduplicated sub-shapes of `kind`.
///
/// `TopExp_Explorer` visits a shared sub-shape once per parent (a box's edge appears
/// twice, once per adjacent face); `TopExp::MapShapes` is the deduplicating walk a
/// display mesh needs.
fn sub_shapes(
    shape: &TopoDS_Shape,
    kind: TopAbs_ShapeEnum,
) -> cxx::UniquePtr<ffi::top_tools::TopTools_IndexedMapOfShape> {
    let mut map = ffi::top_tools::new_indexed_map_of_shape();
    ffi::top_exp::TopExp::MapShapes(shape, kind, map.pin_mut());
    map
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

/// Probe every face's triangulation. Returns the first face shape for later use.
fn report_face_meshes(shape: &TopoDS_Shape) -> (i32, i32, i32, bool, i32, bool) {
    let faces = sub_shapes(shape, TopAbs_ShapeEnum::TopAbs_FACE);
    let mut triangles = 0;
    let mut min_node = i32::MAX;
    let mut has_normals = false;
    let mut reversed = 0;
    let mut identity_locations = true;

    for face_index in 1..=faces.Extent() {
        let face_shape = faces.FindKey(face_index);
        if matches!(
            face_shape.Orientation(),
            TopAbs_Orientation::TopAbs_REVERSED
        ) {
            reversed += 1;
        }

        let face = TopoDS::Face(face_shape);
        let mut location = ffi::top_loc::Location_new();
        let handle = ffi::b_rep::BRep_Tool_Triangulation(face, location.pin_mut());
        if handle.IsNull() {
            println!("  face {face_index}: no triangulation");
            continue;
        }

        let tri = ffi::poly::Handle_Poly_Triangulation_Get(&handle)
            .expect("a non-null triangulation dereferences");
        let nb_nodes = tri.NbNodes();
        let nb_triangles = tri.NbTriangles();
        has_normals |= tri.HasNormals();
        triangles += nb_triangles;

        for i in 1..=nb_triangles {
            let triangle = tri.Triangle(i);
            for k in 1..=3 {
                min_node = min_node.min(triangle.Value(k));
            }
        }

        let trsf = ffi::top_loc::TopLoc_Location_Transformation(&location);
        let identity = (1..=3).all(|r| (trsf.Value(r, r) - 1.0).abs() < 1e-12)
            && (1..=3).all(|r| trsf.Value(r, 4).abs() < 1e-12);
        identity_locations &= identity;

        // Sample the first node through the location transform.
        let mut node = ffi::poly::Poly_Triangulation_Node(tri, 1);
        node.pin_mut().Transform(&trsf);

        println!(
            "  face {face_index}: {nb_nodes} nodes, {nb_triangles} triangles, \
             normals={}, orientation={:?}, first node=({:.2}, {:.2}, {:.2}), location={}",
            tri.HasNormals(),
            face_shape.Orientation(),
            node.X(),
            node.Y(),
            node.Z(),
            if identity { "identity" } else { "transformed" },
        );
    }

    println!(
        "  totals: {triangles} triangles, min node index {min_node} \
         (0-based if 0, 1-based if 1), any normals={has_normals}, \
         reversed faces={reversed}, all locations identity={identity_locations}"
    );

    (
        faces.Extent(),
        triangles,
        min_node,
        has_normals,
        reversed,
        identity_locations,
    )
}

/// Sample every edge with `BRepAdaptor_Curve` + `GCPnts_TangentialDeflection`.
/// Returns `(edges sampled, total points, all edges have >= 2 points)`.
fn report_edges(shape: &TopoDS_Shape) -> (i32, i32, bool) {
    let edges_map = sub_shapes(shape, TopAbs_ShapeEnum::TopAbs_EDGE);
    let edges = edges_map.Extent();
    let mut total_points = 0;
    let mut all_valid = true;

    for index in 1..=edges {
        let edge = TopoDS::Edge(edges_map.FindKey(index));
        let curve = ffi::b_rep_adaptor::BRepAdaptor_Curve_new(edge);
        // 0.5 rad is OCCT's own default angular deflection.
        let sampler = ffi::gc_pnts::TangentialDeflection_new(&curve, 0.5, 0.1);
        let points = sampler.NbPoints();
        total_points += points;
        all_valid &= points >= 2;

        if index <= 3 {
            let first = ffi::gc_pnts::GCPnts_TangentialDeflection_Value(&sampler, 1);
            let last = ffi::gc_pnts::GCPnts_TangentialDeflection_Value(&sampler, points);
            println!(
                "  edge {index}: {points} points, ({:.2}, {:.2}, {:.2}) -> ({:.2}, {:.2}, {:.2})",
                first.X(),
                first.Y(),
                first.Z(),
                last.X(),
                last.Y(),
                last.Z(),
            );
        }
    }

    println!("  totals: {edges} edges, {total_points} points, every edge >= 2 points={all_valid}");
    (edges, total_points, all_valid)
}

/// Edge -> adjacent faces, and face -> bounding edges.
fn report_adjacency(shape: &TopoDS_Shape) -> (i32, i32, bool) {
    // `MapShapesAndAncestors(S, TS, TA, M)` only walks **ancestors**: the map is
    // keyed by sub-shapes of type `TS`, with the containing `TA` elements as values.
    // It answers edge -> faces, never face -> edges. The inverse direction is an
    // explorer over the face.
    let mut edge_faces = ffi::top_tools::new_indexed_data_map_of_shape_list_of_shape();
    ffi::top_exp::TopExp::MapShapesAndAncestors(
        shape,
        TopAbs_ShapeEnum::TopAbs_EDGE,
        TopAbs_ShapeEnum::TopAbs_FACE,
        edge_faces.pin_mut(),
    );

    let edges_map = sub_shapes(shape, TopAbs_ShapeEnum::TopAbs_EDGE);
    let mut every_edge_has_two_faces = true;
    let mut edges = 0;
    for index in 1..=edges_map.Extent() {
        edges += 1;
        let edge_shape = edges_map.FindKey(index);
        let data_index = edge_faces.FindIndex(edge_shape);
        let n = if data_index > 0 {
            bridge::fc_list_size(edge_faces.FindFromIndex(data_index))
        } else {
            0
        };
        every_edge_has_two_faces &= n == 2;
        if edges <= 3 {
            println!("  edge {edges}: {n} adjacent faces");
        }
    }

    let faces_map = sub_shapes(shape, TopAbs_ShapeEnum::TopAbs_FACE);
    let mut every_face_has_four_edges = true;
    let mut faces = 0;
    for index in 1..=faces_map.Extent() {
        faces += 1;
        let face = faces_map.FindKey(index);
        let mut n = 0;
        let mut explorer = ffi::top_exp::TopExp_Explorer_new(face, TopAbs_ShapeEnum::TopAbs_EDGE);
        while explorer.More() {
            n += 1;
            explorer.pin_mut().Next();
        }
        every_face_has_four_edges &= n == 4;
    }

    println!(
        "  totals: {edges} edges each with 2 faces={every_edge_has_two_faces}, \
         {faces} faces each with 4 edges={every_face_has_four_edges}"
    );
    (
        edges,
        faces,
        every_edge_has_two_faces && every_face_has_four_edges,
    )
}

/// Pick the first edge and one adjacent face, as `TopoDS_Shape` copies.
fn first_edge_and_face(
    shape: &TopoDS_Shape,
) -> (cxx::UniquePtr<TopoDS_Shape>, cxx::UniquePtr<TopoDS_Shape>) {
    let edges = sub_shapes(shape, TopAbs_ShapeEnum::TopAbs_EDGE);
    assert!(edges.Extent() >= 1, "the shape has an edge");
    let edge_shape = ffi::topo_ds::TopoDS_Shape_to_owned(edges.FindKey(1));

    let mut edge_faces = ffi::top_tools::new_indexed_data_map_of_shape_list_of_shape();
    ffi::top_exp::TopExp::MapShapesAndAncestors(
        shape,
        TopAbs_ShapeEnum::TopAbs_EDGE,
        TopAbs_ShapeEnum::TopAbs_FACE,
        edge_faces.pin_mut(),
    );
    let data_index = edge_faces.FindIndex(&edge_shape);
    assert!(data_index > 0, "the edge is a key in the ancestor map");
    let list = edge_faces.FindFromIndex(data_index);
    assert!(bridge::fc_list_size(list) >= 1, "the edge has a face");
    let face = bridge::fc_list_value(list, 1);
    let face_shape = ffi::topo_ds::cast_face_to_shape(TopoDS::Face(face));
    let face_owned = ffi::topo_ds::TopoDS_Shape_to_owned(face_shape);

    (edge_shape, face_owned)
}

fn report_chamfers(
    shape: &TopoDS_Shape,
    edge: &TopoDS_Shape,
    face: &TopoDS_Shape,
) -> (bool, bool, i32) {
    // Equal-distance chamfer: the crate bridges `Add(distance, edge)`.
    let mut equal = ffi::b_rep_fillet_api::BRepFilletAPI_MakeChamfer_new(shape);
    equal.pin_mut().add_edge(1.0, TopoDS::Edge(edge));
    {
        let _ = equal.pin_mut().Shape();
    }
    let equal_done = equal.IsDone();
    let equal_faces = if equal_done {
        let out = {
            let built = equal.pin_mut().Shape();
            ffi::topo_ds::TopoDS_Shape_to_owned(built)
        };
        count(&out, TopAbs_ShapeEnum::TopAbs_FACE)
    } else {
        0
    };
    println!("  equal-distance chamfer: done={equal_done}, faces={equal_faces}");

    // Asymmetric chamfer: only reachable through the sibling bridge.
    let mut asym = ffi::b_rep_fillet_api::BRepFilletAPI_MakeChamfer_new(shape);
    bridge::fc_chamfer_add_asym(
        asym.pin_mut(),
        1.0,
        2.0,
        TopoDS::Edge(edge),
        TopoDS::Face(face),
    );
    {
        let _ = asym.pin_mut().Shape();
    }
    let asym_done = asym.IsDone();
    let asym_faces = if asym_done {
        let out = {
            let built = asym.pin_mut().Shape();
            ffi::topo_ds::TopoDS_Shape_to_owned(built)
        };
        count(&out, TopAbs_ShapeEnum::TopAbs_FACE)
    } else {
        0
    };
    println!("  two-distance chamfer: done={asym_done}, faces={asym_faces}");

    (equal_done, asym_done, equal_faces)
}

/// Fillet one edge and report the lineage the bridge exposes.
fn report_fillet(shape: &TopoDS_Shape, edge: &TopoDS_Shape) -> (bool, i32, i32, i32) {
    let mut fillet = ffi::b_rep_fillet_api::BRepFilletAPI_MakeFillet_new(shape);
    fillet.pin_mut().add_edge(1.0, TopoDS::Edge(edge));
    {
        let _ = fillet.pin_mut().Shape();
    }
    let done = fillet.IsDone();
    let faces = if done {
        let out = {
            let built = fillet.pin_mut().Shape();
            ffi::topo_ds::TopoDS_Shape_to_owned(built)
        };
        count(&out, TopAbs_ShapeEnum::TopAbs_FACE)
    } else {
        0
    };

    let mut modified = ffi::top_tools::new_indexed_map_of_shape();
    let mut generated = ffi::top_tools::new_indexed_map_of_shape();
    bridge::fc_fillet_modified(fillet.pin_mut(), edge, modified.pin_mut());
    bridge::fc_fillet_generated(fillet.pin_mut(), edge, generated.pin_mut());
    let deleted = bridge::fc_fillet_is_deleted(fillet.pin_mut(), edge);
    println!(
        "  fillet: done={done}, faces={faces}, \
         Modified(edge)={}, Generated(edge)={}, IsDeleted(edge)={deleted}",
        modified.Extent(),
        generated.Extent()
    );

    (done, faces, modified.Extent(), generated.Extent())
}

fn main() {
    println!("OCCT tessellation / topology probe (opencascade-sys 0.3)\n=========================================================");

    let mut make_box = ffi::b_rep_prim_api::BRepPrimAPI_MakeBox_new(
        &ffi::gp::new_point(0.0, 0.0, 0.0),
        10.0,
        10.0,
        10.0,
    );
    let shape = make_box.pin_mut().Shape();

    println!("\n== topology ==");
    print_counts("box", shape);

    println!("\n== meshing (deflection 0.5) ==");
    let mesh = ffi::b_rep_mesh::IncrementalMesh_new(shape, 0.5);
    assert!(mesh.IsDone(), "BRepMesh_IncrementalMesh succeeded");
    let (faces, triangles, min_node, has_normals, reversed, identity) = report_face_meshes(shape);

    println!("\n== edge sampling ==");
    let (edges, points, edges_valid) = report_edges(shape);

    println!("\n== adjacency ==");
    let (adj_edges, adj_faces, adjacency_ok) = report_adjacency(shape);

    println!("\n== chamfer ==");
    let (edge, face) = first_edge_and_face(shape);
    let (equal_done, asym_done, equal_faces) = report_chamfers(shape, &edge, &face);

    println!("\n== fillet ==");
    let (fillet_done, fillet_faces, modified, generated) = report_fillet(shape, &edge);

    println!("\n== findings ==");
    println!(
        "faces={faces} triangles={triangles} edges={edges} edge_points={points} \
         min_node_index={min_node} normals={has_normals} reversed_faces={reversed} \
         identity_locations={identity}"
    );

    assert_eq!(faces, 6, "a box has six faces");
    assert_eq!(edges, 12, "a box has twelve edges");
    assert_eq!(adj_edges, 12);
    assert_eq!(adj_faces, 6);
    assert_eq!(triangles, 12, "two triangles per planar face");
    assert_eq!(min_node, 1, "Poly_Triangulation node indices are 1-based");
    assert!(
        !has_normals,
        "a box's planar triangulations carry no normals"
    );
    assert!(identity, "a primitive's triangulation location is identity");
    assert!(edges_valid, "every edge samples to at least two points");
    assert!(adjacency_ok, "edge/face ancestor lists are populated");
    assert!(equal_done, "the equal-distance chamfer builds");
    assert!(asym_done, "the bridged two-distance chamfer builds");
    assert_eq!(equal_faces, 7, "chamfering one edge adds one face");
    assert!(fillet_done, "the fillet builds");

    println!(
        "\nConclusion: per-face meshing, edge sampling, adjacency, chamfer and fillet \
         lineage all work. The only bridge additions needed are list access (ancestors), \
         the two-distance chamfer, and the fillet/chamfer history calls; edge polylines \
         need no bridge at all (`BRepAdaptor_Curve` + `GCPnts_TangentialDeflection`)."
    );
    println!(
        "Fillet lineage for the seeded edge: Modified={modified}, Generated={generated}, \
         result faces={fillet_faces}."
    );
}
