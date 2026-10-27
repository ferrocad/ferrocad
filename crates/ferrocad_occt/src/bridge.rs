//! Sibling cxx bridge over OCCT, reusing `opencascade-sys`'s types.
//!
//! `opencascade-sys` 0.3 builds geometry but does not bridge shape *history*
//! (`Modified`/`Generated`/`IsDeleted`, and the element maps lineage needs). Rather
//! than patching or forking the crate, this declares a second `#[cxx::bridge]` over
//! the same OCCT types and links the same libraries. The C++ side is
//! `include/fc_history.hxx`; the design is written up in `docs/developer/architecture/occt-integration.md`
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
        include!("ferrocad_occt/include/fc_geom2d.hxx");
        include!("ferrocad_occt/include/fc_transform.hxx");
        include!("ferrocad_occt/include/fc_shape_ops.hxx");
        include!("ferrocad_occt/src/fc_part_ops.hxx");

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
        fn fc_ruled_surface(first: &TopoDS_Shape, second: &TopoDS_Shape)
        -> UniquePtr<TopoDS_Shape>;

        /// Planar offset of a face or wire (`BRepOffsetAPI_MakeOffset`), or null.
        fn fc_offset(shape: &TopoDS_Shape, distance: f64, join: i32) -> UniquePtr<TopoDS_Shape>;

        /// Hollow a solid by `offset`, leaving `closing_faces` open, or null.
        fn fc_thick_solid(
            shape: &TopoDS_Shape,
            closing_faces: &TopTools_ListOfShape,
            offset: f64,
            tolerance: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The intersection of two shapes (`BRepAlgoAPI_Section`), or null.
        fn fc_section(first: &TopoDS_Shape, second: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// The wires where `shape` meets the plane `d·x = distance`, or null.
        fn fc_slice(
            shape: &TopoDS_Shape,
            dx: f64,
            dy: f64,
            dz: f64,
            distance: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The split parts of input `index` from a General Fuse, or null.
        fn fc_general_fuse_modified(
            shapes: &TopTools_ListOfShape,
            index: i32,
            tolerance: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// `BRepBndLib::AddOptimal` bounds, six values, or false.
        fn fc_optimal_box(
            shape: &TopoDS_Shape,
            use_triangulation: bool,
            use_tolerance: bool,
            out: &mut Vec<f64>,
        ) -> bool;

        /// `TopoDS_Shape::IsSame` (ignores orientation).
        fn fc_is_same(first: &TopoDS_Shape, second: &TopoDS_Shape) -> bool;

        /// A deep copy with a new TShape (`BRepBuilderAPI_Copy`), or null.
        fn fc_copy_shape(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// A hash consistent with `IsSame` (TShape + location, orientation ignored).
        fn fc_shape_hash(shape: &TopoDS_Shape, upper: u64) -> u64;

        /// `Shape.isInside`: 1 inside, 0 outside, -1 when the kernel fails.
        fn fc_is_inside(
            shape: &TopoDS_Shape,
            x: f64,
            y: f64,
            z: f64,
            tolerance: f64,
            check_face: bool,
        ) -> i32;

        /// `TopoDS_Shape::IsPartner` (same TShape, any location/orientation).
        fn fc_is_partner(first: &TopoDS_Shape, second: &TopoDS_Shape) -> bool;

        /// A copy with the complement of the orientation, or null.
        fn fc_complement(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// `Shape.transformGeometry`: copy, then `BRepBuilderAPI_GTransform`.
        fn fc_transform_geometry(
            shape: &TopoDS_Shape,
            matrix: &[f64],
            copy: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// `ShapeAnalysis_ShapeTolerance` average/maximum/minimum (NaN on failure).
        fn fc_global_tolerance(shape: &TopoDS_Shape, mode: i32) -> f64;

        /// `ShapeFix_ShapeTolerance::SetTolerance` (mutates the TShape in place).
        fn fc_fix_tolerance(shape: &TopoDS_Shape, value: f64, shape_type: i32) -> bool;

        /// `ShapeFix_ShapeTolerance::LimitTolerance`.
        fn fc_limit_tolerance(
            shape: &TopoDS_Shape,
            minimum: f64,
            maximum: f64,
            shape_type: i32,
        ) -> bool;

        /// `ShapeAnalysis_ShapeTolerance::InTolerance`, appended to `out`.
        fn fc_tolerance_in(
            shape: &TopoDS_Shape,
            minimum: f64,
            maximum: f64,
            shape_type: i32,
            out: Pin<&mut TopTools_ListOfShape>,
        );

        /// `ShapeAnalysis_ShapeTolerance::OverTolerance`, appended to `out`.
        fn fc_tolerance_over(
            shape: &TopoDS_Shape,
            value: f64,
            shape_type: i32,
            out: Pin<&mut TopTools_ListOfShape>,
        );

        /// `Shape.makeWires`: a compound of the wires found, or null.
        fn fc_make_wires(shape: &TopoDS_Shape, tolerance: f64)
        -> UniquePtr<TopoDS_Shape>;

        /// A `ShapeUpgrade_RemoveInternalWires` run (see `fc_shape_ops.hxx`).
        type FcRemoveInternalWires;
        fn fc_remove_internal_wires_new() -> UniquePtr<FcRemoveInternalWires>;
        fn perform(
            self: Pin<&mut FcRemoveInternalWires>,
            shape: &TopoDS_Shape,
            minimum_area: f64,
        ) -> bool;
        fn result(self: &FcRemoveInternalWires) -> UniquePtr<TopoDS_Shape>;

        /// Binary STL bytes for `Shape.exportStl`, or empty on failure.
        fn fc_stl_bytes(shape: &TopoDS_Shape, deflection: f64) -> Vec<u8>;

        /// Remove sub-shapes (`BRepTools_ReShape`), or null.
        fn fc_reshape_remove(
            shape: &TopoDS_Shape,
            removals: &TopTools_ListOfShape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// Replace sub-shapes (`BRepTools_ReShape`), or null.
        fn fc_reshape_replace(
            shape: &TopoDS_Shape,
            from: &TopTools_ListOfShape,
            to: &TopTools_ListOfShape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// `BRepCheck_Analyzer`, the bool behind `Shape.isValid`.
        fn fc_is_valid(shape: &TopoDS_Shape) -> bool;

        /// Split an edge at curve parameters into a wire of pieces, or null.
        fn fc_split_edge(shape: &TopoDS_Shape, parameters: &[f64]) -> UniquePtr<TopoDS_Shape>;

        /// Split `shape` with the parallel `tools`/`faces` lists
        /// (`BRepFeat_SplitShape`), filling the two result groups.
        fn fc_split_shape(
            shape: &TopoDS_Shape,
            tools: &TopTools_ListOfShape,
            faces: &TopTools_ListOfShape,
            check_interior: bool,
            direct: Pin<&mut TopTools_ListOfShape>,
            left: Pin<&mut TopTools_ListOfShape>,
        ) -> bool;

        /// Sweep `profile` along `spine` (`BRepOffsetAPI_MakePipe`), or null.
        fn fc_pipe(
            spine: &TopoDS_Shape,
            profile: &TopoDS_Shape,
            tolerance: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The 3x3 inertia matrix, row-major (nine values), or empty.
        fn fc_inertia_matrix(shape: &TopoDS_Shape, out: &mut Vec<f64>);

        /// `Shape.CenterOfMass` under upstream's `getShapeProperties` rule: the
        /// measurement follows what the shape contains. False when it has nothing
        /// to measure.
        fn fc_center_of_mass(shape: &TopoDS_Shape, x: &mut f64, y: &mut f64, z: &mut f64) -> bool;

        /// `GProp_PrincipalProps`, flattened, or empty.
        fn fc_principal_properties(shape: &TopoDS_Shape, out: &mut Vec<f64>);

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

        /// Apply a general affine transform (`BRepBuilderAPI_GTransform`), or
        /// null on failure.
        fn fc_gtransform(shape: &TopoDS_Shape, matrix: &[f64]) -> UniquePtr<TopoDS_Shape>;

        /// A B-spline from control points (flat xyz), or null on failure.
        fn fc_bspline_from_poles(
            coords: &[f64],
            degree: i32,
            periodic: bool,
        ) -> UniquePtr<Handle_Geom_Curve>;

        /// A full elliptical edge from its centre, normal, major-axis direction
        /// and the two radii (`Part.Ellipse.toShape`), or null on failure.
        fn fc_ellipse_edge(
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
            xx: f64,
            xy: f64,
            xz: f64,
            major: f64,
            minor: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A trimmed conic edge over `[first, last]`: 3 = ellipse, 4 = hyperbola,
        /// 5 = parabola. Null on failure.
        #[allow(clippy::too_many_arguments)]
        fn fc_conic_edge(
            kind: i32,
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
            xx: f64,
            xy: f64,
            xz: f64,
            major: f64,
            minor: f64,
            first: f64,
            last: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A Bezier edge from explicit poles and weights, or null on failure.
        fn fc_bezier_edge(coords: &[f64], weights: &[f64]) -> UniquePtr<TopoDS_Shape>;

        /// A B-spline edge from explicit poles/weights/knots/mults over the range,
        /// or null on failure. `periodic` keeps a periodic knot vector periodic.
        fn fc_bspline_edge_range(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            first: f64,
            last: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// An edge of the offset curve of `basis` over `[first, last]`, or null on
        /// failure.
        fn fc_offset_curve_edge(
            basis: &TopoDS_Shape,
            offset: f64,
            dx: f64,
            dy: f64,
            dz: f64,
            first: f64,
            last: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A B-spline edge from explicit poles, weights, knots and multiplicities
        /// (upstream's `buildFromPolesMultsKnots`), or null on failure.
        fn fc_bspline_edge(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The flattened state of the B-spline built from the given arrays, or
        /// empty when the arguments do not describe a valid curve. See
        /// `fc_bspline_push_state` in `fc_curve.hxx` for the layout.
        fn fc_bspline_probe(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
        ) -> Vec<f64>;

        /// The flattened state of the B-spline interpolating `coords`, or empty.
        fn fc_bspline_probe_interpolated(coords: &[f64], periodic: bool) -> Vec<f64>;

        /// `BSplineCurve.segment(u1, u2)`: the state of the restricted curve.
        #[allow(clippy::too_many_arguments)]
        fn fc_bspline_segment(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            u1: f64,
            u2: f64,
        ) -> Vec<f64>;

        /// Rebuild the curve in (or out of) the periodic form and return its new
        /// flattened state, or empty.
        fn fc_bspline_set_periodic(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            target_periodic: bool,
        ) -> Vec<f64>;

        /// `Geom_BSplineCurve::Resolution` for a 3D tolerance (NaN on failure).
        fn fc_bspline_resolution(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            tolerance: f64,
        ) -> f64;

        /// The point at parameter `u` on the B-spline with the given state, or
        /// empty on failure. See `fc_curve.hxx`.
        fn fc_bspline_point(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            u: f64,
        ) -> Vec<f64>;

        /// A conic (2 = circle, 3 = ellipse) over `[first, last]` as a B-spline,
        /// flattened like `fc_bspline_probe`. `nurbs` selects the exact rational
        /// conversion (`GeomConic::toNurbs`) rather than the approximating
        /// `GeomCurve::toBSpline`. Empty on failure. See `fc_curve.hxx`.
        fn fc_conic_to_spline(
            kind: i32,
            ox: f64,
            oy: f64,
            oz: f64,
            nx: f64,
            ny: f64,
            nz: f64,
            xx: f64,
            xy: f64,
            xz: f64,
            major: f64,
            minor: f64,
            first: f64,
            last: f64,
            nurbs: bool,
        ) -> Vec<f64>;

        /// `TopTools_IndexedMapOfShape::FindIndex`, which the crate does not bridge.
        fn fc_indexed_map_find_index(map: &TopTools_IndexedMapOfShape, shape: &TopoDS_Shape)
        -> i32;

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

        /// `BRep_Tool::Degenerated`.
        fn fc_edge_degenerated(shape: &TopoDS_Shape) -> bool;

        /// `BRep_Tool::IsClosed(edge, face)`: the edge is a seam of the face.
        fn fc_edge_is_seam(edge: &TopoDS_Shape, face: &TopoDS_Shape) -> bool;

        /// The edge's curve parameter range (`BRep_Tool::Range`).
        fn fc_edge_range(shape: &TopoDS_Shape, first: &mut f64, last: &mut f64);

        /// The face's UV bounds (`BRepTools::UVBounds`).
        fn fc_face_uv_bounds(
            shape: &TopoDS_Shape,
            u0: &mut f64,
            u1: &mut f64,
            v0: &mut f64,
            v1: &mut f64,
        );

        /// The number of direct children (`TopoDS_Iterator`).
        fn fc_direct_child_count(shape: &TopoDS_Shape) -> i32;

        /// The minimum distance between two shapes and the closest point on each
        /// (`BRepExtrema_DistShapeShape`). False when there is no solution.
        fn fc_dist_to_shape(
            first: &TopoDS_Shape,
            second: &TopoDS_Shape,
            distance: &mut f64,
            first_x: &mut f64,
            first_y: &mut f64,
            first_z: &mut f64,
            second_x: &mut f64,
            second_y: &mut f64,
            second_z: &mut f64,
        ) -> bool;

        /// Upstream `TopoShape::isClosed`.
        fn fc_is_closed(shape: &TopoDS_Shape) -> bool;

        /// The shape's geometric tolerance (`BRep_Tool::Tolerance`).
        fn fc_shape_tolerance(shape: &TopoDS_Shape) -> f64;

        /// The point of an edge at curve parameter `parameter`
        /// (`BRepAdaptor_Curve::Value`).
        fn fc_edge_value(
            shape: &TopoDS_Shape,
            parameter: f64,
            x: &mut f64,
            y: &mut f64,
            z: &mut f64,
        ) -> bool;

        /// The point of a face at surface parameters `(u, v)`
        /// (`BRepAdaptor_Surface::Value`).
        fn fc_face_value(
            shape: &TopoDS_Shape,
            u: f64,
            v: f64,
            x: &mut f64,
            y: &mut f64,
            z: &mut f64,
        ) -> bool;

        /// The face's backing surface frame, tagged by kind (`BRepAdaptor_Surface`
        /// for a plane, cylinder, cone, sphere or torus); empty for any other
        /// surface.
        fn fc_face_surface_data(shape: &TopoDS_Shape, out: &mut Vec<f64>);

        /// `fc_face_surface_data` with the cone (4) and torus (5) cases added.
        fn fc_face_surface_frame(shape: &TopoDS_Shape, out: &mut Vec<f64>);

        /// The surface normal at `(u, v)` (`BRepLProp_SLProps`, through upstream's
        /// `Tools::getNormal`): three components, or empty when the normal is not
        /// defined there. The face orientation is already applied.
        fn fc_face_normal(shape: &TopoDS_Shape, u: f64, v: f64) -> Vec<f64>;

        /// The two unit surface tangents at `(u, v)` (`TangentU`, `TangentV`), or
        /// empty when either is undefined.
        fn fc_face_tangents(shape: &TopoDS_Shape, u: f64, v: f64) -> Vec<f64>;

        /// The two partial derivatives of order `order` (1 or 2) at `(u, v)`.
        fn fc_face_derivatives(shape: &TopoDS_Shape, u: f64, v: f64, order: i32)
            -> Vec<f64>;

        /// The `(min, max)` principal curvatures at `(u, v)`, or empty.
        fn fc_face_curvature(shape: &TopoDS_Shape, u: f64, v: f64) -> Vec<f64>;

        /// `Geom_Surface::Bounds` of the surface behind a face (untrimmed), or
        /// empty.
        fn fc_face_surface_bounds(shape: &TopoDS_Shape) -> Vec<f64>;

        /// The principal normal of a curve at `parameter` (`BRepLProp_CLProps`),
        /// or empty when it is undefined.
        fn fc_edge_normal(shape: &TopoDS_Shape, parameter: f64) -> Vec<f64>;

        /// The curve derivative of order `order` (1, 2 or 3) at `parameter`.
        fn fc_edge_derivatives(shape: &TopoDS_Shape, parameter: f64, order: i32)
            -> Vec<f64>;

        /// The curvature at `parameter`; `NaN` when it is undefined.
        fn fc_edge_curvature(shape: &TopoDS_Shape, parameter: f64) -> f64;

        /// `Part.Geom2d.*.toShape(surface)`: the pcurve as an edge on the given
        /// surface, with the 3-D curve built from it (`BRepBuilderAPI_MakeEdge`
        /// and `BRepLib::BuildCurves3d`). An empty `surface_spec` uses
        /// `surface_face`'s own surface, and with neither the world XY plane.
        fn fc_bspline2d_edge(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            surface_spec: &[f64],
            surface_face: &TopoDS_Shape,
            first: f64,
            last: f64,
            has_range: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// `Part.Face(wire)` for one boundary wire: `BRepBuilderAPI_MakeFace`
        /// with `OnlyPlane = false` (a plane, cylinder or sphere found from the
        /// wire itself). Null when no surface supports the wire.
        fn fc_face_from_wire(wire: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// Attach a further boundary wire to an existing face
        /// (`BRepBuilderAPI_MakeFace::Add`). Null on failure.
        fn fc_face_add_wire(face: &TopoDS_Shape, wire: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// `Part.makeFace(wires, "Part::FaceMakerBullseye")`: planar faces with
        /// holes, where a hole can hold further faces. One face, or a compound
        /// when the wires describe several regions. Null on failure.
        fn fc_make_face_bullseye(wires: &TopTools_ListOfShape) -> UniquePtr<TopoDS_Shape>;

        /// `Part.Geom2d.*.toShape(surface)` for the analytic 2-D forms: the
        /// planar (`z = 0`) edge's exact curve becomes the pcurve, and the
        /// returned edge is the surface image of that pcurve
        /// (`BRepBuilderAPI_MakeEdge` + `BRepLib::BuildCurves3d`). An empty
        /// `surface_spec` uses `surface_face`'s own surface, and with neither the
        /// world XY plane. Null on failure.
        fn fc_curve2d_on_surface(
            planar_edge: &TopoDS_Shape,
            surface_spec: &[f64],
            surface_face: &TopoDS_Shape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The pcurve an edge has on a face (`BRep_Tool::CurveOnSurface`), in the
        /// B-spline state layout; empty when the edge has none there.
        fn fc_edge_pcurve(edge: &TopoDS_Shape, face: &TopoDS_Shape) -> Vec<f64>;

        /// The vertex an edge starts at, in the edge's own orientation
        /// (`TopExp::FirstVertex`), or null.
        fn fc_edge_first_vertex(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// The vertex an edge ends at, in the edge's own orientation
        /// (`TopExp::LastVertex`), or null.
        fn fc_edge_last_vertex(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// A wire's stored first/last vertex (`TopExp::Vertices`), or null.
        fn fc_wire_first_vertex(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;
        fn fc_wire_last_vertex(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// `GeomAPI_PointsToBSpline` (`BSplineCurve.approximate`), as a state.
        fn fc_bspline_approximated(
            coords: &[f64],
            degree_min: i32,
            degree_max: i32,
            tolerance: f64,
        ) -> Vec<f64>;

        /// `GeomAPI_ProjectPointOnCurve` (`GeometryCurve.parameter`); `NaN` when
        /// the projection fails.
        #[allow(clippy::too_many_arguments)]
        fn fc_bspline_parameter(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            x: f64,
            y: f64,
            z: f64,
        ) -> f64;

        /// `Geom_BSplineCurve::InsertKnot`, reported as the new state.
        fn fc_bspline_insert_knot(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            u: f64,
            mult: i32,
            tolerance: f64,
        ) -> Vec<f64>;

        /// `Geom_BSplineCurve::RemoveKnot`; empty when the removal is rejected.
        #[allow(clippy::too_many_arguments)]
        fn fc_bspline_remove_knot(
            coords: &[f64],
            weights: &[f64],
            knots: &[f64],
            multiplicities: &[i32],
            degree: i32,
            periodic: bool,
            check_rational: bool,
            index: i32,
            mult: i32,
            tolerance: f64,
        ) -> Vec<f64>;

        /// Sew a shape's faces into shells (`BRepBuilderAPI_Sewing`), or null.
        fn fc_sew_shape(shape: &TopoDS_Shape, tolerance: f64) -> UniquePtr<TopoDS_Shape>;

        /// Re-sew a shell whose naive assembly is not valid
        /// (`ShapeUpgrade_ShellSewing::ApplySewing`), or null. This is the
        /// fallback upstream's `Part.makeShell` applies.
        fn fc_sew_shell(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// Convert every curve and surface to a B-spline
        /// (`BRepBuilderAPI_NurbsConvert`), or null.
        fn fc_to_nurbs(shape: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// `Edge.discretize` points as flat xyz; empty on failure. `mode` is
        /// 0 = Number, 1 = Distance, 2 = Deflection, 3 = Angular/Curvature.
        fn fc_edge_discretize(
            shape: &TopoDS_Shape,
            mode: i32,
            value: f64,
            curvature: f64,
            minimum: i32,
            first: f64,
            last: f64,
        ) -> Vec<f64>;

        /// `Wire.discretize` over `BRepAdaptor_CompCurve`; same modes and layout as
        /// `fc_edge_discretize`, with the wire's accumulated parameterisation.
        fn fc_wire_discretize(
            shape: &TopoDS_Shape,
            mode: i32,
            value: f64,
            curvature: f64,
            minimum: i32,
            first: f64,
            last: f64,
        ) -> Vec<f64>;

        /// The curve parameter at arc length `length` (`GCPnts_AbscissaPoint`),
        /// or `NaN` when the length is out of range.
        fn fc_edge_parameter_by_length(
            shape: &TopoDS_Shape,
            length: f64,
            tolerance: f64,
        ) -> f64;

        /// The free (single-face) edges of a shell as a compound
        /// (`ShapeAnalysis_Shell`), or null.
        fn fc_shell_free_edges(shell: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// A face on an elementary surface (flat spec, see
        /// `ferrocad_occt/src/fc_part_ops.hxx`) bounded by `wires`, or null.
        fn fc_make_face_on_surface(
            spec: &[f64],
            wires: &TopTools_ListOfShape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// `GeomAPI_PointsToBSplineSurface` through a row-major `nx` by `ny`
        /// grid of points; the bounds and expanded knot sequences, or empty.
        fn fc_bspline_surface_interpolate(coords: &[f64], nx: i32, ny: i32) -> Vec<f64>;

        /// The face of the interpolating B-spline surface through the grid, or null
        /// (`BSplineSurface.toShape`).
        fn fc_bspline_surface_face(coords: &[f64], nx: i32, ny: i32)
            -> UniquePtr<TopoDS_Shape>;

        /// A copy of the shape with a numeric `TopAbs_Orientation`, or null.
        fn fc_with_orientation(
            shape: &TopoDS_Shape,
            orientation: i32,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The sub-shapes of a numeric `TopAbs_ShapeEnum` as a compound
        /// (`TopExp::MapShapes`), or null.
        fn fc_shape_sub_shapes_of_type(shape: &TopoDS_Shape, kind: i32) -> UniquePtr<TopoDS_Shape>;

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

        /// A default-constructed cut operation that can be reconfigured and
        /// rebuilt several times (upstream's recursive compound policy).
        fn fc_cut_new_op() -> UniquePtr<BRepAlgoAPI_Cut>;

        /// Switch a cut operation between `BOPAlgo_CUT` and `BOPAlgo_FUSE`.
        fn fc_cut_set_operation(op: Pin<&mut BRepAlgoAPI_Cut>, fuse: bool);

        /// Install the argument/tool lists (`SetArguments`/`SetTools`).
        fn fc_cut_configure(
            op: Pin<&mut BRepAlgoAPI_Cut>,
            arguments: &TopTools_ListOfShape,
            tools: &TopTools_ListOfShape,
        );

        /// `Build()` with a default progress range.
        fn fc_cut_build(op: Pin<&mut BRepAlgoAPI_Cut>);

        /// Upstream's constructor settings: run parallel, non-destructive.
        fn fc_cut_prepare(op: Pin<&mut BRepAlgoAPI_Cut>);

        /// Upstream's automatic fuzzy value from the current argument/tool bounds.
        fn fc_cut_set_auto_fuzzy(op: Pin<&mut BRepAlgoAPI_Cut>);

        /// `Face.isPartOfDomain(u, v)` (`BRepTopAdaptor_FClass2d`).
        fn fc_face_is_part_of_domain(face: &TopoDS_Shape, u: f64, v: f64) -> bool;

        /// `Face.validate()`: repair an invalid face's wires, or the unchanged face.
        fn fc_validate_face(face: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// `Shape.fix(precision, mintol, maxtol)` (`ShapeFix_Shape`).
        fn fc_fix_shape(
            shape: &TopoDS_Shape,
            precision: f64,
            mintol: f64,
            maxtol: f64,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The `(u, v)` of a point on a face's surface
        /// (`ShapeAnalysis_Surface::ValueOfUV`).
        fn fc_face_parameter(
            face: &TopoDS_Shape,
            x: f64,
            y: f64,
            z: f64,
            u: &mut f64,
            v: &mut f64,
        ) -> bool;

        /// Project `tool` onto `target`: parallel with a direction, or
        /// perspective from a viewpoint (`BRepProj_Projection`).
        fn fc_project_shape(
            tool: &TopoDS_Shape,
            target: &TopoDS_Shape,
            x: f64,
            y: f64,
            z: f64,
            parallel: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// Remove faces and heal the gap (`BRepAlgoAPI_Defeaturing`), or null.
        fn fc_defeature(
            shape: &TopoDS_Shape,
            faces: &TopTools_ListOfShape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// A 3D offset (`BRepOffsetAPI_MakeOffsetShape::PerformByJoin`), or null.
        fn fc_offset_shape(
            shape: &TopoDS_Shape,
            offset: f64,
            tolerance: f64,
            inter: bool,
            self_inter: bool,
            offset_mode: i32,
            join: i32,
            fill: bool,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The wire's edges in connection order as a compound
        /// (`BRepTools_WireExplorer`), or null.
        fn fc_wire_ordered_edges(wire: &TopoDS_Shape) -> UniquePtr<TopoDS_Shape>;

        /// A B-spline face's surface in the flat layout documented in
        /// `fc_part_ops.hxx`; empty for any other surface.
        fn fc_bspline_surface_data(face: &TopoDS_Shape) -> Vec<f64>;

        /// The face of a B-spline surface described in that layout, or null.
        fn fc_bspline_surface_face_from_data(
            data: &[f64],
        ) -> UniquePtr<TopoDS_Shape>;

        /// `Geom_BSplineSurface::Value(u, v)`.
        fn fc_bspline_surface_value(
            data: &[f64],
            u: f64,
            v: f64,
            x: &mut f64,
            y: &mut f64,
            z: &mut f64,
        ) -> bool;

        /// The `(u, v)` of a point on a B-spline surface.
        fn fc_bspline_surface_parameter(
            data: &[f64],
            x: f64,
            y: f64,
            z: f64,
            u: &mut f64,
            v: &mut f64,
        ) -> bool;

        /// `Part.Face(bspline_surface, wires)`: trim the surface with `wires`.
        fn fc_face_on_bspline_surface(
            data: &[f64],
            wires: &TopTools_ListOfShape,
        ) -> UniquePtr<TopoDS_Shape>;

        /// The exact topological kind of a shape (`TopoDS_Shape::ShapeType`),
        /// or -1 for a null shape.
        fn fc_shape_kind(shape: &TopoDS_Shape) -> i32;

        /// The number of *direct* children (`TopoDS_Iterator`).
        fn fc_shape_child_count(shape: &TopoDS_Shape) -> i32;

        /// The `index`-th (1-based) direct child, or null.
        fn fc_shape_child(shape: &TopoDS_Shape, index: i32) -> UniquePtr<TopoDS_Shape>;
    }
}

pub(crate) use ffi::*;
