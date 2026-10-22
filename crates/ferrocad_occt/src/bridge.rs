//! Sibling cxx bridge over OCCT, reusing `opencascade-sys`'s types.
//!
//! `opencascade-sys` 0.3 builds geometry but does not bridge shape *history*
//! (`Modified`/`Generated`/`IsDeleted`, and the element maps lineage needs). Rather
//! than patching or forking the crate, this declares a second `#[cxx::bridge]` over
//! the same OCCT types and links the same libraries. The C++ side is
//! `include/fc_history.hxx`; the design is written up in `docs/architecture/occt-integration.md`
//! and verified in `spikes/occt-history-rs`.

// `common` is bridged for completeness and used when the trait grows an
// intersection operation; until then it is dead code.
#[allow(dead_code)]
#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("ferrocad_occt/include/fc_history.hxx");
        include!("ferrocad_occt/include/fc_topology.hxx");
        include!("ferrocad_occt/include/fc_curve.hxx");

        // Reuse the crate's opaque types. cxx shares these across bridges by path.
        type BRepAlgoAPI_Fuse = opencascade_sys::b_rep_algo_api::BRepAlgoAPI_Fuse;
        type BRepAlgoAPI_Cut = opencascade_sys::b_rep_algo_api::BRepAlgoAPI_Cut;
        type BRepAlgoAPI_Common = opencascade_sys::b_rep_algo_api::BRepAlgoAPI_Common;
        type BRepFilletAPI_MakeFillet = opencascade_sys::b_rep_fillet_api::BRepFilletAPI_MakeFillet;
        type BRepFilletAPI_MakeChamfer =
            opencascade_sys::b_rep_fillet_api::BRepFilletAPI_MakeChamfer;
        type TopoDS_Shape = opencascade_sys::topo_ds::TopoDS_Shape;
        type TopoDS_Edge = opencascade_sys::topo_ds::TopoDS_Edge;
        type TopoDS_Vertex = opencascade_sys::topo_ds::TopoDS_Vertex;
        type TopoDS_Wire = opencascade_sys::topo_ds::TopoDS_Wire;
        type TopoDS_Face = opencascade_sys::topo_ds::TopoDS_Face;
        type TopoDS_Shell = opencascade_sys::topo_ds::TopoDS_Shell;
        type gp_Trsf = opencascade_sys::gp::gp_Trsf;
        type Handle_Geom_Curve = opencascade_sys::geom::Handle_Geom_Curve;
        type TopTools_IndexedMapOfShape = opencascade_sys::top_tools::TopTools_IndexedMapOfShape;
        type TopTools_IndexedDataMapOfShapeListOfShape =
            opencascade_sys::top_tools::TopTools_IndexedDataMapOfShapeListOfShape;
        type TopTools_ListOfShape = opencascade_sys::top_tools::TopTools_ListOfShape;

        /// The analytic curve behind an edge, flattened (see `fc_curve.hxx` for
        /// the layout: `[kind, origin xyz, direction xyz, x-axis xyz, first, last,
        /// major, minor]`).
        fn fc_edge_curve(shape: &TopoDS_Shape) -> Vec<f64>;

        /// The edge view of a shape (`TopoDS::Edge`).
        fn fc_shape_as_edge(shape: &TopoDS_Shape) -> &TopoDS_Edge;

        /// The vertex view of a shape (`TopoDS::Vertex`).
        fn fc_shape_as_vertex(shape: &TopoDS_Shape) -> &TopoDS_Vertex;

        /// The wire view of a shape (`TopoDS::Wire`).
        fn fc_shape_as_wire(shape: &TopoDS_Shape) -> &TopoDS_Wire;

        /// The face view of a shape (`TopoDS::Face`).
        fn fc_shape_as_face(shape: &TopoDS_Shape) -> &TopoDS_Face;

        /// The shell view of a shape (`TopoDS::Shell`).
        fn fc_shape_as_shell(shape: &TopoDS_Shape) -> &TopoDS_Shell;

        /// Shape-type guards (the cast would otherwise throw).
        fn fc_is_edge(shape: &TopoDS_Shape) -> bool;
        fn fc_is_wire(shape: &TopoDS_Shape) -> bool;
        fn fc_is_vertex(shape: &TopoDS_Shape) -> bool;
        fn fc_is_face(shape: &TopoDS_Shape) -> bool;
        fn fc_is_shell(shape: &TopoDS_Shape) -> bool;

        /// A ruled surface between two edges (`BRepFill::Face`), or null.
        fn fc_ruled_surface(
            first: &TopoDS_Shape,
            second: &TopoDS_Shape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A filled face through the boundary wires, or null on failure.
        fn fc_fill_wires(wires: &TopTools_ListOfShape) -> UniquePtr<TopoDS_Shape>;

        /// All split parts of the arguments (General Fuse), or null on failure.
        fn fc_general_fuse(shapes: &TopTools_ListOfShape) -> UniquePtr<TopoDS_Shape>;

        /// A comp-solid of the given solids, or null on failure.
        fn fc_comp_solid(solids: &TopTools_ListOfShape) -> UniquePtr<TopoDS_Shape>;

        /// Faces from a (possibly self-intersecting) wire, or null on failure.
        fn fc_split_wire_into_faces(wire: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// A copy of the shape with reversed orientation.
        fn fc_reversed(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// Install a row-major 4x4 matrix into an OCCT transform.
        fn fc_trsf_from_matrix(m: &[f64], out: Pin<&mut gp_Trsf>) -> bool;

        /// A B-spline from control points (flat xyz), or null on failure.
        fn fc_bspline_from_poles(
            coords: &[f64],
            degree: i32,
            periodic: bool,
        ) -> UniquePtr<Handle_Geom_Curve>;

        /// `TopTools_IndexedMapOfShape::FindIndex`, which the crate does not bridge.
        fn fc_indexed_map_find_index(
            map: &TopTools_IndexedMapOfShape,
            shape: &TopoDS_Shape,
        ) -> i32;

        /// Like `fc_indexed_map_find_index`, but ignores orientation (`IsSame`).
        fn fc_indexed_map_find_index_same(
            map: &TopTools_IndexedMapOfShape,
            shape: &TopoDS_Shape,
        ) -> i32;

        /// Like `fc_indexed_map_find_index_same`, for the ancestor data map.
        fn fc_data_map_find_index_same(
            map: &TopTools_IndexedDataMapOfShapeListOfShape,
            shape: &TopoDS_Shape,
        ) -> i32;

        /// `TopTools_ListOfShape::Size`.
        fn fc_list_size(list: &TopTools_ListOfShape) -> i32;

        /// The `index`-th shape in the list (1-based).
        fn fc_list_value<'a>(list: &'a TopTools_ListOfShape, index: i32) -> &'a TopoDS_Shape;

        /// The two-distance chamfer form.
        fn fc_chamfer_add_asym(
            op: Pin<&mut BRepFilletAPI_MakeChamfer>,
            distance1: f64,
            distance2: f64,
            edge: &TopoDS_Edge,
            face: &TopoDS_Face,
        );

        fn fc_fillet_modified(
            op: Pin<&mut BRepFilletAPI_MakeFillet>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_fillet_generated(
            op: Pin<&mut BRepFilletAPI_MakeFillet>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_fillet_is_deleted(
            op: Pin<&mut BRepFilletAPI_MakeFillet>,
            shape: &TopoDS_Shape,
        ) -> bool;

        fn fc_chamfer_modified(
            op: Pin<&mut BRepFilletAPI_MakeChamfer>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_chamfer_generated(
            op: Pin<&mut BRepFilletAPI_MakeChamfer>,
            shape: &TopoDS_Shape,
            out: Pin<&mut TopTools_IndexedMapOfShape>,
        );
        fn fc_chamfer_is_deleted(
            op: Pin<&mut BRepFilletAPI_MakeChamfer>,
            shape: &TopoDS_Shape,
        ) -> bool;

        /// Two unit tangents at the edges' shared vertex (six numbers), or empty.
        fn fc_edge_tangents(edge1: &TopoDS_Edge, edge2: &TopoDS_Edge, out: &mut Vec<f64>);

        /// Serialise a shape to BREP bytes (in memory; the crate only does files).
        fn fc_brep_write(shape: &TopoDS_Shape, out: &mut Vec<u8>);
        /// Read a shape from BREP bytes; null on failure.
        fn fc_brep_read(data: &[u8]) -> UniquePtr<TopoDS_Shape>;

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

pub(crate) use ffi::*;
