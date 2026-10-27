//! The `ShapeFix_*` fixers behind `Part.ShapeFix`.
//!
//! A sibling cxx bridge over `include/fc_shapefix.hxx` (the same pattern as
//! [`crate::bridge`]): OCCT 7.x's shape-fixing package is not bridged by
//! `opencascade-sys`, so the header wraps each fixer and this module exposes it
//! as a plain Rust object. The fixers own `Handle(ShapeFix_*)`, which is why
//! every bridge method is `const` and no `Pin` appears here.
//!
//! Input shapes are [`ferrocad_geom::Shape`] values (they must come from this
//! backend); outputs are owned `Shape` values again. A failed cast or an OCCT
//! exception becomes `false`/`None`, which the Python layer reports as
//! `Part.OCCError` rather than inventing a result.

use cxx::UniquePtr;
use ferrocad_geom::Shape;
use opencascade_sys::topo_ds::TopoDS_Shape;

use crate::shape::{downcast, OcctShape};

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
            include!("ferrocad_occt/include/fc_shapefix.hxx");

            type TopoDS_Shape = opencascade_sys::topo_ds::TopoDS_Shape;

            type FcShapeFixRoot;
            type FcShapeFixEdge;
            type FcShapeFixWire;
            type FcShapeFixFace;
            type FcShapeFixShell;
            type FcShapeFixSolid;
            type FcShapeFixShape;
            type FcShapeList;

            // --- Root ------------------------------------------------------
            fn fc_shapefix_root_new() -> UniquePtr<FcShapeFixRoot>;
            fn set_precision(self: &FcShapeFixRoot, value: f64);
            fn precision(self: &FcShapeFixRoot) -> f64;
            fn set_min_tolerance(self: &FcShapeFixRoot, value: f64);
            fn min_tolerance(self: &FcShapeFixRoot) -> f64;
            fn set_max_tolerance(self: &FcShapeFixRoot, value: f64);
            fn max_tolerance(self: &FcShapeFixRoot) -> f64;
            fn limit_tolerance(self: &FcShapeFixRoot, value: f64) -> f64;

            // --- Edge ------------------------------------------------------
            fn fc_shapefix_edge_new() -> UniquePtr<FcShapeFixEdge>;
            fn fix_remove_pcurve_face(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                face: &TopoDS_Shape,
            ) -> bool;
            fn fix_remove_pcurve_surface(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                nx: f64,
                ny: f64,
                nz: f64,
            ) -> bool;
            fn fix_remove_curve3d(self: &FcShapeFixEdge, edge: &TopoDS_Shape) -> bool;
            fn fix_add_pcurve_face(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                face: &TopoDS_Shape,
                seam: bool,
                prec: f64,
            ) -> bool;
            fn fix_add_pcurve_surface(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                nx: f64,
                ny: f64,
                nz: f64,
                seam: bool,
                prec: f64,
            ) -> bool;
            fn fix_add_curve3d(self: &FcShapeFixEdge, edge: &TopoDS_Shape) -> bool;
            fn fix_vertex_tolerance_face(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                face: &TopoDS_Shape,
            ) -> bool;
            fn fix_vertex_tolerance(self: &FcShapeFixEdge, edge: &TopoDS_Shape) -> bool;
            fn fix_reversed2d_face(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                face: &TopoDS_Shape,
            ) -> bool;
            fn fix_reversed2d_surface(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                nx: f64,
                ny: f64,
                nz: f64,
            ) -> bool;
            fn fix_same_parameter(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                tolerance: f64,
            ) -> bool;
            fn fix_same_parameter_face(
                self: &FcShapeFixEdge,
                edge: &TopoDS_Shape,
                face: &TopoDS_Shape,
                tolerance: f64,
            ) -> bool;

            // --- Wire ------------------------------------------------------
            fn fc_shapefix_wire_new() -> UniquePtr<FcShapeFixWire>;
            fn fc_shapefix_wire_from_wire(
                wire: &TopoDS_Shape,
                face: &TopoDS_Shape,
                prec: f64,
            ) -> UniquePtr<FcShapeFixWire>;
            fn init(
                self: &FcShapeFixWire,
                wire: &TopoDS_Shape,
                face: &TopoDS_Shape,
                prec: f64,
            );
            fn load(self: &FcShapeFixWire, wire: &TopoDS_Shape) -> bool;
            fn set_precision(self: &FcShapeFixWire, value: f64);
            fn set_face(self: &FcShapeFixWire, face: &TopoDS_Shape) -> bool;
            fn set_surface(
                self: &FcShapeFixWire,
                ox: f64,
                oy: f64,
                oz: f64,
                nx: f64,
                ny: f64,
                nz: f64,
            ) -> bool;
            fn set_surface_placed(
                self: &FcShapeFixWire,
                ox: f64,
                oy: f64,
                oz: f64,
                nx: f64,
                ny: f64,
                nz: f64,
            ) -> bool;
            fn set_max_tail_angle(self: &FcShapeFixWire, angle: f64);
            fn set_max_tail_width(self: &FcShapeFixWire, width: f64);
            fn is_loaded(self: &FcShapeFixWire) -> bool;
            fn is_ready(self: &FcShapeFixWire) -> bool;
            fn nb_edges(self: &FcShapeFixWire) -> i32;
            fn wire(self: &FcShapeFixWire) -> UniquePtr<TopoDS_Shape>;
            fn wire_api_make(self: &FcShapeFixWire) -> UniquePtr<TopoDS_Shape>;
            fn face(self: &FcShapeFixWire) -> UniquePtr<TopoDS_Shape>;
            fn perform(self: &FcShapeFixWire) -> bool;
            fn clear_modes(self: &FcShapeFixWire);
            fn clear_statuses(self: &FcShapeFixWire);
            fn fix_edge_tool(self: &FcShapeFixWire) -> UniquePtr<FcShapeFixEdge>;
            fn fix_reorder(self: &FcShapeFixWire) -> bool;
            fn fix_small_global(self: &FcShapeFixWire, lock: bool, prec: f64) -> i32;
            fn fix_small_one(
                self: &FcShapeFixWire,
                num: i32,
                lock: bool,
                prec: f64,
            ) -> bool;
            fn fix_connected(self: &FcShapeFixWire, prec: f64) -> bool;
            fn fix_connected_one(self: &FcShapeFixWire, num: i32, prec: f64) -> bool;
            fn fix_edge_curves(self: &FcShapeFixWire) -> bool;
            fn fix_degenerated(self: &FcShapeFixWire) -> bool;
            fn fix_degenerated_one(self: &FcShapeFixWire, num: i32) -> bool;
            fn fix_self_intersection(self: &FcShapeFixWire) -> bool;
            fn fix_lacking(self: &FcShapeFixWire, force: bool) -> bool;
            fn fix_lacking_one(self: &FcShapeFixWire, num: i32, force: bool) -> bool;
            fn fix_closed(self: &FcShapeFixWire, prec: f64) -> bool;
            fn fix_gaps3d(self: &FcShapeFixWire) -> bool;
            fn fix_gaps2d(self: &FcShapeFixWire) -> bool;
            fn fix_seam(self: &FcShapeFixWire, num: i32) -> bool;
            fn fix_shifted(self: &FcShapeFixWire) -> bool;
            fn fix_notched_edges(self: &FcShapeFixWire) -> bool;
            fn fix_gap3d(self: &FcShapeFixWire, num: i32, convert: bool) -> bool;
            fn fix_gap2d(self: &FcShapeFixWire, num: i32, convert: bool) -> bool;
            fn fix_tails(self: &FcShapeFixWire) -> bool;
            fn modify_topology_mode(self: &FcShapeFixWire) -> bool;
            fn set_modify_topology_mode(self: &FcShapeFixWire, value: bool);
            fn modify_geometry_mode(self: &FcShapeFixWire) -> bool;
            fn set_modify_geometry_mode(self: &FcShapeFixWire, value: bool);
            fn closed_wire_mode(self: &FcShapeFixWire) -> bool;
            fn set_closed_wire_mode(self: &FcShapeFixWire, value: bool);
            fn preference_pcurve_mode(self: &FcShapeFixWire) -> bool;
            fn set_preference_pcurve_mode(self: &FcShapeFixWire, value: bool);
            fn fix_gaps_by_ranges_mode(self: &FcShapeFixWire) -> bool;
            fn set_fix_gaps_by_ranges_mode(self: &FcShapeFixWire, value: bool);
            fn modify_remove_loop_mode(self: &FcShapeFixWire) -> i32;
            fn set_modify_remove_loop_mode(self: &FcShapeFixWire, value: i32);
            fn fix_reorder_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_reorder_mode(self: &FcShapeFixWire, value: i32);
            fn fix_small_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_small_mode(self: &FcShapeFixWire, value: i32);
            fn fix_connected_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_connected_mode(self: &FcShapeFixWire, value: i32);
            fn fix_edge_curves_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_edge_curves_mode(self: &FcShapeFixWire, value: i32);
            fn fix_degenerated_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_degenerated_mode(self: &FcShapeFixWire, value: i32);
            fn fix_self_intersection_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_self_intersection_mode(self: &FcShapeFixWire, value: i32);
            fn fix_lacking_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_lacking_mode(self: &FcShapeFixWire, value: i32);
            fn fix_gaps3d_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_gaps3d_mode(self: &FcShapeFixWire, value: i32);
            fn fix_gaps2d_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_gaps2d_mode(self: &FcShapeFixWire, value: i32);
            fn fix_reversed2d_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_reversed2d_mode(self: &FcShapeFixWire, value: i32);
            fn fix_remove_pcurve_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_remove_pcurve_mode(self: &FcShapeFixWire, value: i32);
            fn fix_add_pcurve_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_add_pcurve_mode(self: &FcShapeFixWire, value: i32);
            fn fix_remove_curve3d_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_remove_curve3d_mode(self: &FcShapeFixWire, value: i32);
            fn fix_add_curve3d_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_add_curve3d_mode(self: &FcShapeFixWire, value: i32);
            fn fix_seam_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_seam_mode(self: &FcShapeFixWire, value: i32);
            fn fix_shifted_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_shifted_mode(self: &FcShapeFixWire, value: i32);
            fn fix_same_parameter_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_same_parameter_mode(self: &FcShapeFixWire, value: i32);
            fn fix_vertex_tolerance_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_vertex_tolerance_mode(self: &FcShapeFixWire, value: i32);
            fn fix_notched_edges_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_notched_edges_mode(self: &FcShapeFixWire, value: i32);
            fn fix_self_intersecting_edge_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_self_intersecting_edge_mode(self: &FcShapeFixWire, value: i32);
            fn fix_intersecting_edges_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_intersecting_edges_mode(self: &FcShapeFixWire, value: i32);
            fn fix_non_adjacent_intersecting_edges_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_non_adjacent_intersecting_edges_mode(
                self: &FcShapeFixWire,
                value: i32,
            );
            fn fix_tail_mode(self: &FcShapeFixWire) -> i32;
            fn set_fix_tail_mode(self: &FcShapeFixWire, value: i32);

            // --- Face ------------------------------------------------------
            fn fc_shapefix_face_new() -> UniquePtr<FcShapeFixFace>;
            fn fc_shapefix_face_from_face(face: &TopoDS_Shape) -> UniquePtr<FcShapeFixFace>;
            fn fc_shapefix_face_from_plane(
                ox: f64,
                oy: f64,
                oz: f64,
                nx: f64,
                ny: f64,
                nz: f64,
                prec: f64,
                fwd: bool,
            ) -> UniquePtr<FcShapeFixFace>;
            fn init_face(self: &FcShapeFixFace, face: &TopoDS_Shape) -> bool;
            fn init_plane(
                self: &FcShapeFixFace,
                ox: f64,
                oy: f64,
                oz: f64,
                nx: f64,
                ny: f64,
                nz: f64,
                prec: f64,
                fwd: bool,
            ) -> bool;
            fn add(self: &FcShapeFixFace, wire: &TopoDS_Shape) -> bool;
            fn face(self: &FcShapeFixFace) -> UniquePtr<TopoDS_Shape>;
            fn result(self: &FcShapeFixFace) -> UniquePtr<TopoDS_Shape>;
            fn perform(self: &FcShapeFixFace) -> bool;
            fn clear_modes(self: &FcShapeFixFace);
            fn fix_orientation(self: &FcShapeFixFace) -> bool;
            fn fix_add_natural_bound(self: &FcShapeFixFace) -> bool;
            fn fix_missing_seam(self: &FcShapeFixFace) -> bool;
            fn fix_small_area_wire(self: &FcShapeFixFace, remove_small: bool) -> bool;
            fn fix_loop_wire(self: &FcShapeFixFace) -> UniquePtr<FcShapeList>;
            fn fix_intersecting_wires(self: &FcShapeFixFace) -> bool;
            fn fix_wires_two_coincident_edges(self: &FcShapeFixFace) -> bool;
            fn fix_periodic_degenerated(self: &FcShapeFixFace) -> bool;
            fn fix_wire_tool(self: &FcShapeFixFace) -> UniquePtr<FcShapeFixWire>;
            fn fix_wire_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_wire_mode(self: &FcShapeFixFace, value: i32);
            fn fix_orientation_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_orientation_mode(self: &FcShapeFixFace, value: i32);
            fn fix_add_natural_bound_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_add_natural_bound_mode(self: &FcShapeFixFace, value: i32);
            fn fix_missing_seam_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_missing_seam_mode(self: &FcShapeFixFace, value: i32);
            fn fix_small_area_wire_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_small_area_wire_mode(self: &FcShapeFixFace, value: i32);
            fn remove_small_area_face_mode(self: &FcShapeFixFace) -> i32;
            fn set_remove_small_area_face_mode(self: &FcShapeFixFace, value: i32);
            fn fix_intersecting_wires_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_intersecting_wires_mode(self: &FcShapeFixFace, value: i32);
            fn fix_loop_wires_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_loop_wires_mode(self: &FcShapeFixFace, value: i32);
            fn fix_split_face_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_split_face_mode(self: &FcShapeFixFace, value: i32);
            fn auto_correct_precision_mode(self: &FcShapeFixFace) -> i32;
            fn set_auto_correct_precision_mode(self: &FcShapeFixFace, value: i32);
            fn fix_periodic_degenerated_mode(self: &FcShapeFixFace) -> i32;
            fn set_fix_periodic_degenerated_mode(self: &FcShapeFixFace, value: i32);

            // --- Shell -----------------------------------------------------
            fn fc_shapefix_shell_new() -> UniquePtr<FcShapeFixShell>;
            fn fc_shapefix_shell_from_shell(
                shell: &TopoDS_Shape,
            ) -> UniquePtr<FcShapeFixShell>;
            fn init(self: &FcShapeFixShell, shell: &TopoDS_Shape) -> bool;
            fn perform(self: &FcShapeFixShell) -> bool;
            fn shape(self: &FcShapeFixShell) -> UniquePtr<TopoDS_Shape>;
            fn shell(self: &FcShapeFixShell) -> UniquePtr<TopoDS_Shape>;
            fn error_faces(self: &FcShapeFixShell) -> UniquePtr<TopoDS_Shape>;
            fn nb_shells(self: &FcShapeFixShell) -> i32;
            fn fix_face_tool(self: &FcShapeFixShell) -> UniquePtr<FcShapeFixFace>;
            fn fix_face_orientation(
                self: &FcShapeFixShell,
                shell: &TopoDS_Shape,
                multi_connex: bool,
                non_manifold: bool,
            ) -> bool;
            fn set_non_manifold_flag(self: &FcShapeFixShell, value: bool);
            fn fix_face_mode(self: &FcShapeFixShell) -> i32;
            fn set_fix_face_mode(self: &FcShapeFixShell, value: i32);
            fn fix_orientation_mode(self: &FcShapeFixShell) -> i32;
            fn set_fix_orientation_mode(self: &FcShapeFixShell, value: i32);

            // --- Solid -----------------------------------------------------
            fn fc_shapefix_solid_new() -> UniquePtr<FcShapeFixSolid>;
            fn fc_shapefix_solid_from_solid(
                solid: &TopoDS_Shape,
            ) -> UniquePtr<FcShapeFixSolid>;
            fn init(self: &FcShapeFixSolid, solid: &TopoDS_Shape) -> bool;
            fn perform(self: &FcShapeFixSolid) -> bool;
            fn shape(self: &FcShapeFixSolid) -> UniquePtr<TopoDS_Shape>;
            fn solid_from_shell(
                self: &FcShapeFixSolid,
                shell: &TopoDS_Shape,
            ) -> UniquePtr<TopoDS_Shape>;
            fn solid(self: &FcShapeFixSolid) -> UniquePtr<TopoDS_Shape>;
            fn fix_shell_tool(self: &FcShapeFixSolid) -> UniquePtr<FcShapeFixShell>;
            fn fix_shell_mode(self: &FcShapeFixSolid) -> i32;
            fn set_fix_shell_mode(self: &FcShapeFixSolid, value: i32);
            fn fix_shell_orientation_mode(self: &FcShapeFixSolid) -> i32;
            fn set_fix_shell_orientation_mode(self: &FcShapeFixSolid, value: i32);
            fn create_open_solid_mode(self: &FcShapeFixSolid) -> bool;
            fn set_create_open_solid_mode(self: &FcShapeFixSolid, value: bool);

            // --- Shape -----------------------------------------------------
            fn fc_shapefix_shape_new() -> UniquePtr<FcShapeFixShape>;
            fn fc_shapefix_shape_from_shape(
                shape: &TopoDS_Shape,
            ) -> UniquePtr<FcShapeFixShape>;
            fn init(self: &FcShapeFixShape, shape: &TopoDS_Shape) -> bool;
            fn perform(self: &FcShapeFixShape) -> bool;
            fn shape(self: &FcShapeFixShape) -> UniquePtr<TopoDS_Shape>;
            fn fix_solid_tool(self: &FcShapeFixShape) -> UniquePtr<FcShapeFixSolid>;
            fn fix_shell_tool(self: &FcShapeFixShape) -> UniquePtr<FcShapeFixShell>;
            fn fix_face_tool(self: &FcShapeFixShape) -> UniquePtr<FcShapeFixFace>;
            fn fix_wire_tool(self: &FcShapeFixShape) -> UniquePtr<FcShapeFixWire>;
            fn fix_edge_tool(self: &FcShapeFixShape) -> UniquePtr<FcShapeFixEdge>;
            fn fix_solid_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_solid_mode(self: &FcShapeFixShape, value: i32);
            fn fix_free_shell_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_free_shell_mode(self: &FcShapeFixShape, value: i32);
            fn fix_free_face_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_free_face_mode(self: &FcShapeFixShape, value: i32);
            fn fix_free_wire_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_free_wire_mode(self: &FcShapeFixShape, value: i32);
            fn fix_same_parameter_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_same_parameter_mode(self: &FcShapeFixShape, value: i32);
            fn fix_vertex_position_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_vertex_position_mode(self: &FcShapeFixShape, value: i32);
            fn fix_vertex_tol_mode(self: &FcShapeFixShape) -> i32;
            fn set_fix_vertex_tol_mode(self: &FcShapeFixShape, value: i32);

            // --- List ------------------------------------------------------
            fn is_ok(self: &FcShapeList) -> bool;
            fn size(self: &FcShapeList) -> usize;
            fn at(self: &FcShapeList, index: usize) -> &TopoDS_Shape;
    }
}

/// Run `body` with the kernel shape behind `shape`; `false`/`None` when the
/// shape is null or came from another backend.
macro_rules! with_occt {
    ($shape:expr, |$inner:ident| $body:expr) => {{
        match downcast($shape) {
            Ok(occt) => {
                let guard = occt.borrow();
                match guard.as_ref() {
                    Some($inner) => $body,
                    None => Default::default(),
                }
            }
            Err(_) => Default::default(),
        }
    }};
}

/// Take an owned output shape, or `None` if OCCT returned a null handle.
fn output(shape: UniquePtr<TopoDS_Shape>) -> Option<Shape> {
    if shape.is_null() {
        None
    } else {
        Some(Shape::new(OcctShape(shape)))
    }
}

/// Call `f` with the kernel shape behind `shape`; `None` when the shape is null
/// or came from another backend.
fn with_shape<R>(shape: &Shape, f: impl FnOnce(&TopoDS_Shape) -> R) -> Option<R> {
    let occt = downcast(shape).ok()?;
    let guard = occt.borrow();
    guard.as_ref().map(f)
}

/// `Part.ShapeFix.Root` — precision and tolerance limits shared by all fixers.
pub struct Root {
    inner: UniquePtr<ffi::FcShapeFixRoot>,
}

impl Root {
    pub fn new() -> Self {
        Root {
            inner: ffi::fc_shapefix_root_new(),
        }
    }

    pub fn precision(&self) -> f64 {
        self.inner.precision()
    }
    pub fn set_precision(&self, value: f64) {
        self.inner.set_precision(value)
    }
    pub fn min_tolerance(&self) -> f64 {
        self.inner.min_tolerance()
    }
    pub fn set_min_tolerance(&self, value: f64) {
        self.inner.set_min_tolerance(value)
    }
    pub fn max_tolerance(&self) -> f64 {
        self.inner.max_tolerance()
    }
    pub fn set_max_tolerance(&self, value: f64) {
        self.inner.set_max_tolerance(value)
    }
    pub fn limit_tolerance(&self, value: f64) -> f64 {
        self.inner.limit_tolerance(value)
    }
}

impl Default for Root {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ShapeFix.Edge` — the edge-level fixes.
pub struct Edge {
    inner: UniquePtr<ffi::FcShapeFixEdge>,
}

impl Edge {
    pub fn new() -> Option<Self> {
        let inner = ffi::fc_shapefix_edge_new();
        if inner.is_null() {
            None
        } else {
            Some(Edge { inner })
        }
    }

    fn from_inner(inner: UniquePtr<ffi::FcShapeFixEdge>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Edge { inner })
        }
    }

    pub fn remove_pcurve_face(&self, edge: &Shape, face: &Shape) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| self
            .inner
            .fix_remove_pcurve_face(edge, face)))
    }

    /// The surface overload: `Face.Surface` carries a normal but no location,
    /// so the plane is anchored at a vertex of the edge (which lies on it).
    pub fn remove_pcurve_surface(&self, edge: &Shape, normal: [f64; 3]) -> bool {
        with_occt!(edge, |edge| self.inner.fix_remove_pcurve_surface(
            edge, normal[0], normal[1], normal[2]
        ))
    }

    pub fn remove_curve3d(&self, edge: &Shape) -> bool {
        with_occt!(edge, |edge| self.inner.fix_remove_curve3d(edge))
    }

    pub fn add_pcurve_face(&self, edge: &Shape, face: &Shape, seam: bool, prec: f64) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| self
            .inner
            .fix_add_pcurve_face(edge, face, seam, prec)))
    }

    pub fn add_pcurve_surface(
        &self,
        edge: &Shape,
        normal: [f64; 3],
        seam: bool,
        prec: f64,
    ) -> bool {
        with_occt!(edge, |edge| self.inner.fix_add_pcurve_surface(
            edge, normal[0], normal[1], normal[2], seam, prec
        ))
    }

    pub fn add_curve3d(&self, edge: &Shape) -> bool {
        with_occt!(edge, |edge| self.inner.fix_add_curve3d(edge))
    }

    pub fn vertex_tolerance_face(&self, edge: &Shape, face: &Shape) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| self
            .inner
            .fix_vertex_tolerance_face(edge, face)))
    }

    pub fn vertex_tolerance(&self, edge: &Shape) -> bool {
        with_occt!(edge, |edge| self.inner.fix_vertex_tolerance(edge))
    }

    pub fn reversed2d_face(&self, edge: &Shape, face: &Shape) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| self
            .inner
            .fix_reversed2d_face(edge, face)))
    }

    pub fn reversed2d_surface(&self, edge: &Shape, normal: [f64; 3]) -> bool {
        with_occt!(edge, |edge| self.inner.fix_reversed2d_surface(
            edge, normal[0], normal[1], normal[2]
        ))
    }

    pub fn same_parameter(&self, edge: &Shape, tolerance: f64) -> bool {
        with_occt!(edge, |edge| self
            .inner
            .fix_same_parameter(edge, tolerance))
    }

    pub fn same_parameter_face(&self, edge: &Shape, face: &Shape, tolerance: f64) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| self
            .inner
            .fix_same_parameter_face(edge, face, tolerance)))
    }
}

impl Default for Edge {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Edge cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Wire` — the wire-level fixes and their modes.
pub struct Wire {
    inner: UniquePtr<ffi::FcShapeFixWire>,
}

impl Wire {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::fc_shapefix_wire_new())
    }

    pub fn from_wire_face(wire: &Shape, face: &Shape, prec: f64) -> Option<Self> {
        let (Ok(wire), Ok(face)) = (downcast(wire), downcast(face)) else {
            return None;
        };
        let wire_guard = wire.borrow();
        let face_guard = face.borrow();
        let (Some(wire), Some(face)) = (wire_guard.as_ref(), face_guard.as_ref()) else {
            return None;
        };
        Self::from_inner(ffi::fc_shapefix_wire_from_wire(wire, face, prec))
    }

    fn from_inner(inner: UniquePtr<ffi::FcShapeFixWire>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Wire { inner })
        }
    }

    pub fn init(&self, wire: &Shape, face: &Shape, prec: f64) -> bool {
        with_occt!(wire, |wire| with_occt!(face, |face| {
            self.inner.init(wire, face, prec);
            true
        }))
    }

    pub fn load(&self, wire: &Shape) -> bool {
        with_occt!(wire, |wire| self.inner.load(wire))
    }

    /// `ShapeFix_Root::SetPrecision` - the tolerance `fixWire(tolerance)` uses
    /// when no face is supplied.
    pub fn set_precision(&self, value: f64) {
        self.inner.set_precision(value)
    }

    pub fn set_face(&self, face: &Shape) -> bool {
        with_occt!(face, |face| self.inner.set_face(face))
    }

    pub fn set_surface(&self, origin: [f64; 3], normal: [f64; 3], placed: bool) -> bool {
        if placed {
            self.inner.set_surface_placed(
                origin[0], origin[1], origin[2], normal[0], normal[1], normal[2],
            )
        } else {
            self.inner
                .set_surface(origin[0], origin[1], origin[2], normal[0], normal[1], normal[2])
        }
    }

    pub fn set_max_tail_angle(&self, angle: f64) {
        self.inner.set_max_tail_angle(angle)
    }
    pub fn set_max_tail_width(&self, width: f64) {
        self.inner.set_max_tail_width(width)
    }
    pub fn is_loaded(&self) -> bool {
        self.inner.is_loaded()
    }
    pub fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }
    pub fn number_of_edges(&self) -> i32 {
        self.inner.nb_edges()
    }
    pub fn wire(&self) -> Option<Shape> {
        output(self.inner.wire())
    }
    pub fn wire_api_make(&self) -> Option<Shape> {
        output(self.inner.wire_api_make())
    }
    pub fn face(&self) -> Option<Shape> {
        output(self.inner.face())
    }
    pub fn perform(&self) -> bool {
        self.inner.perform()
    }
    pub fn clear_modes(&self) {
        self.inner.clear_modes()
    }
    pub fn clear_statuses(&self) {
        self.inner.clear_statuses()
    }
    pub fn fix_edge_tool(&self) -> Option<Edge> {
        Edge::from_inner(self.inner.fix_edge_tool())
    }

    pub fn fix_reorder(&self) -> bool {
        self.inner.fix_reorder()
    }
    pub fn fix_small_global(&self, lock: bool, prec: f64) -> i32 {
        self.inner.fix_small_global(lock, prec)
    }
    pub fn fix_small_one(&self, num: i32, lock: bool, prec: f64) -> bool {
        self.inner.fix_small_one(num, lock, prec)
    }
    pub fn fix_connected(&self, prec: f64) -> bool {
        self.inner.fix_connected(prec)
    }
    pub fn fix_connected_one(&self, num: i32, prec: f64) -> bool {
        self.inner.fix_connected_one(num, prec)
    }
    pub fn fix_edge_curves(&self) -> bool {
        self.inner.fix_edge_curves()
    }
    pub fn fix_degenerated(&self) -> bool {
        self.inner.fix_degenerated()
    }
    pub fn fix_degenerated_one(&self, num: i32) -> bool {
        self.inner.fix_degenerated_one(num)
    }
    pub fn fix_self_intersection(&self) -> bool {
        self.inner.fix_self_intersection()
    }
    pub fn fix_lacking(&self, force: bool) -> bool {
        self.inner.fix_lacking(force)
    }
    pub fn fix_lacking_one(&self, num: i32, force: bool) -> bool {
        self.inner.fix_lacking_one(num, force)
    }
    pub fn fix_closed(&self, prec: f64) -> bool {
        self.inner.fix_closed(prec)
    }
    pub fn fix_gaps3d(&self) -> bool {
        self.inner.fix_gaps3d()
    }
    pub fn fix_gaps2d(&self) -> bool {
        self.inner.fix_gaps2d()
    }
    pub fn fix_seam(&self, num: i32) -> bool {
        self.inner.fix_seam(num)
    }
    pub fn fix_shifted(&self) -> bool {
        self.inner.fix_shifted()
    }
    pub fn fix_notched_edges(&self) -> bool {
        self.inner.fix_notched_edges()
    }
    pub fn fix_gap3d(&self, num: i32, convert: bool) -> bool {
        self.inner.fix_gap3d(num, convert)
    }
    pub fn fix_gap2d(&self, num: i32, convert: bool) -> bool {
        self.inner.fix_gap2d(num, convert)
    }
    pub fn fix_tails(&self) -> bool {
        self.inner.fix_tails()
    }

    // Modes reported as booleans where OCCT stores a `Standard_Boolean`, and as
    // integers where it stores a three-state `Standard_Integer`.
    pub fn modify_topology_mode(&self) -> bool {
        self.inner.modify_topology_mode()
    }
    pub fn set_modify_topology_mode(&self, value: bool) {
        self.inner.set_modify_topology_mode(value)
    }
    pub fn modify_geometry_mode(&self) -> bool {
        self.inner.modify_geometry_mode()
    }
    pub fn set_modify_geometry_mode(&self, value: bool) {
        self.inner.set_modify_geometry_mode(value)
    }
    pub fn closed_wire_mode(&self) -> bool {
        self.inner.closed_wire_mode()
    }
    pub fn set_closed_wire_mode(&self, value: bool) {
        self.inner.set_closed_wire_mode(value)
    }
    pub fn preference_pcurve_mode(&self) -> bool {
        self.inner.preference_pcurve_mode()
    }
    pub fn set_preference_pcurve_mode(&self, value: bool) {
        self.inner.set_preference_pcurve_mode(value)
    }
    pub fn fix_gaps_by_ranges_mode(&self) -> bool {
        self.inner.fix_gaps_by_ranges_mode()
    }
    pub fn set_fix_gaps_by_ranges_mode(&self, value: bool) {
        self.inner.set_fix_gaps_by_ranges_mode(value)
    }
    pub fn modify_remove_loop_mode(&self) -> i32 {
        self.inner.modify_remove_loop_mode()
    }
    pub fn set_modify_remove_loop_mode(&self, value: i32) {
        self.inner.set_modify_remove_loop_mode(value)
    }
    pub fn fix_reorder_mode(&self) -> i32 {
        self.inner.fix_reorder_mode()
    }
    pub fn set_fix_reorder_mode(&self, value: i32) {
        self.inner.set_fix_reorder_mode(value)
    }
    pub fn fix_small_mode(&self) -> i32 {
        self.inner.fix_small_mode()
    }
    pub fn set_fix_small_mode(&self, value: i32) {
        self.inner.set_fix_small_mode(value)
    }
    pub fn fix_connected_mode(&self) -> i32 {
        self.inner.fix_connected_mode()
    }
    pub fn set_fix_connected_mode(&self, value: i32) {
        self.inner.set_fix_connected_mode(value)
    }
    pub fn fix_edge_curves_mode(&self) -> i32 {
        self.inner.fix_edge_curves_mode()
    }
    pub fn set_fix_edge_curves_mode(&self, value: i32) {
        self.inner.set_fix_edge_curves_mode(value)
    }
    pub fn fix_degenerated_mode(&self) -> i32 {
        self.inner.fix_degenerated_mode()
    }
    pub fn set_fix_degenerated_mode(&self, value: i32) {
        self.inner.set_fix_degenerated_mode(value)
    }
    pub fn fix_self_intersection_mode(&self) -> i32 {
        self.inner.fix_self_intersection_mode()
    }
    pub fn set_fix_self_intersection_mode(&self, value: i32) {
        self.inner.set_fix_self_intersection_mode(value)
    }
    pub fn fix_lacking_mode(&self) -> i32 {
        self.inner.fix_lacking_mode()
    }
    pub fn set_fix_lacking_mode(&self, value: i32) {
        self.inner.set_fix_lacking_mode(value)
    }
    pub fn fix_gaps3d_mode(&self) -> i32 {
        self.inner.fix_gaps3d_mode()
    }
    pub fn set_fix_gaps3d_mode(&self, value: i32) {
        self.inner.set_fix_gaps3d_mode(value)
    }
    pub fn fix_gaps2d_mode(&self) -> i32 {
        self.inner.fix_gaps2d_mode()
    }
    pub fn set_fix_gaps2d_mode(&self, value: i32) {
        self.inner.set_fix_gaps2d_mode(value)
    }
    pub fn fix_reversed2d_mode(&self) -> i32 {
        self.inner.fix_reversed2d_mode()
    }
    pub fn set_fix_reversed2d_mode(&self, value: i32) {
        self.inner.set_fix_reversed2d_mode(value)
    }
    pub fn fix_remove_pcurve_mode(&self) -> i32 {
        self.inner.fix_remove_pcurve_mode()
    }
    pub fn set_fix_remove_pcurve_mode(&self, value: i32) {
        self.inner.set_fix_remove_pcurve_mode(value)
    }
    pub fn fix_add_pcurve_mode(&self) -> i32 {
        self.inner.fix_add_pcurve_mode()
    }
    pub fn set_fix_add_pcurve_mode(&self, value: i32) {
        self.inner.set_fix_add_pcurve_mode(value)
    }
    pub fn fix_remove_curve3d_mode(&self) -> i32 {
        self.inner.fix_remove_curve3d_mode()
    }
    pub fn set_fix_remove_curve3d_mode(&self, value: i32) {
        self.inner.set_fix_remove_curve3d_mode(value)
    }
    pub fn fix_add_curve3d_mode(&self) -> i32 {
        self.inner.fix_add_curve3d_mode()
    }
    pub fn set_fix_add_curve3d_mode(&self, value: i32) {
        self.inner.set_fix_add_curve3d_mode(value)
    }
    pub fn fix_seam_mode(&self) -> i32 {
        self.inner.fix_seam_mode()
    }
    pub fn set_fix_seam_mode(&self, value: i32) {
        self.inner.set_fix_seam_mode(value)
    }
    pub fn fix_shifted_mode(&self) -> i32 {
        self.inner.fix_shifted_mode()
    }
    pub fn set_fix_shifted_mode(&self, value: i32) {
        self.inner.set_fix_shifted_mode(value)
    }
    pub fn fix_same_parameter_mode(&self) -> i32 {
        self.inner.fix_same_parameter_mode()
    }
    pub fn set_fix_same_parameter_mode(&self, value: i32) {
        self.inner.set_fix_same_parameter_mode(value)
    }
    pub fn fix_vertex_tolerance_mode(&self) -> i32 {
        self.inner.fix_vertex_tolerance_mode()
    }
    pub fn set_fix_vertex_tolerance_mode(&self, value: i32) {
        self.inner.set_fix_vertex_tolerance_mode(value)
    }
    pub fn fix_notched_edges_mode(&self) -> i32 {
        self.inner.fix_notched_edges_mode()
    }
    pub fn set_fix_notched_edges_mode(&self, value: i32) {
        self.inner.set_fix_notched_edges_mode(value)
    }
    pub fn fix_self_intersecting_edge_mode(&self) -> i32 {
        self.inner.fix_self_intersecting_edge_mode()
    }
    pub fn set_fix_self_intersecting_edge_mode(&self, value: i32) {
        self.inner.set_fix_self_intersecting_edge_mode(value)
    }
    pub fn fix_intersecting_edges_mode(&self) -> i32 {
        self.inner.fix_intersecting_edges_mode()
    }
    pub fn set_fix_intersecting_edges_mode(&self, value: i32) {
        self.inner.set_fix_intersecting_edges_mode(value)
    }
    pub fn fix_non_adjacent_intersecting_edges_mode(&self) -> i32 {
        self.inner.fix_non_adjacent_intersecting_edges_mode()
    }
    pub fn set_fix_non_adjacent_intersecting_edges_mode(&self, value: i32) {
        self.inner.set_fix_non_adjacent_intersecting_edges_mode(value)
    }
    pub fn fix_tail_mode(&self) -> i32 {
        self.inner.fix_tail_mode()
    }
    pub fn set_fix_tail_mode(&self, value: i32) {
        self.inner.set_fix_tail_mode(value)
    }
}

impl Default for Wire {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Wire cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Face` — the face-level fixes.
pub struct Face {
    inner: UniquePtr<ffi::FcShapeFixFace>,
}

impl Face {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::fc_shapefix_face_new())
    }

    pub fn from_face(face: &Shape) -> Option<Self> {
        Self::from_inner(with_shape(face, ffi::fc_shapefix_face_from_face)?)
    }

    pub fn from_plane(
        origin: [f64; 3],
        normal: [f64; 3],
        prec: f64,
        forward: bool,
    ) -> Option<Self> {
        let inner = ffi::fc_shapefix_face_from_plane(
            origin[0], origin[1], origin[2], normal[0], normal[1], normal[2], prec, forward,
        );
        Self::from_inner(inner)
    }

    fn from_inner(inner: UniquePtr<ffi::FcShapeFixFace>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Face { inner })
        }
    }

    pub fn init_face(&self, face: &Shape) -> bool {
        with_occt!(face, |face| self.inner.init_face(face))
    }

    pub fn init_plane(
        &self,
        origin: [f64; 3],
        normal: [f64; 3],
        prec: f64,
        forward: bool,
    ) -> bool {
        self.inner.init_plane(
            origin[0], origin[1], origin[2], normal[0], normal[1], normal[2], prec, forward,
        )
    }

    pub fn add(&self, wire: &Shape) -> bool {
        with_occt!(wire, |wire| self.inner.add(wire))
    }

    pub fn perform(&self) -> bool {
        self.inner.perform()
    }
    pub fn fix_orientation(&self) -> bool {
        self.inner.fix_orientation()
    }
    pub fn fix_add_natural_bound(&self) -> bool {
        self.inner.fix_add_natural_bound()
    }
    pub fn fix_missing_seam(&self) -> bool {
        self.inner.fix_missing_seam()
    }
    pub fn fix_small_area_wire(&self, remove_small: bool) -> bool {
        self.inner.fix_small_area_wire(remove_small)
    }

    /// `(fixed, replaced_wires)` — upstream's `(bool, list)` tuple.
    pub fn fix_loop_wire(&self) -> (bool, Vec<Shape>) {
        let list = self.inner.fix_loop_wire();
        if list.is_null() {
            return (false, Vec::new());
        }
        let ok = list.is_ok();
        let wires = (0..list.size())
            .map(|index| crate::shape::erase(list.at(index)))
            .collect();
        (ok, wires)
    }

    pub fn fix_intersecting_wires(&self) -> bool {
        self.inner.fix_intersecting_wires()
    }
    pub fn fix_wires_two_coincident_edges(&self) -> bool {
        self.inner.fix_wires_two_coincident_edges()
    }
    pub fn fix_periodic_degenerated(&self) -> bool {
        self.inner.fix_periodic_degenerated()
    }
    pub fn face(&self) -> Option<Shape> {
        output(self.inner.face())
    }
    pub fn result(&self) -> Option<Shape> {
        output(self.inner.result())
    }
    pub fn fix_wire_tool(&self) -> Option<Wire> {
        Wire::from_inner(self.inner.fix_wire_tool())
    }
    pub fn clear_modes(&self) {
        self.inner.clear_modes()
    }

    pub fn fix_wire_mode(&self) -> i32 {
        self.inner.fix_wire_mode()
    }
    pub fn set_fix_wire_mode(&self, value: i32) {
        self.inner.set_fix_wire_mode(value)
    }
    pub fn fix_orientation_mode(&self) -> i32 {
        self.inner.fix_orientation_mode()
    }
    pub fn set_fix_orientation_mode(&self, value: i32) {
        self.inner.set_fix_orientation_mode(value)
    }
    pub fn fix_add_natural_bound_mode(&self) -> i32 {
        self.inner.fix_add_natural_bound_mode()
    }
    pub fn set_fix_add_natural_bound_mode(&self, value: i32) {
        self.inner.set_fix_add_natural_bound_mode(value)
    }
    pub fn fix_missing_seam_mode(&self) -> i32 {
        self.inner.fix_missing_seam_mode()
    }
    pub fn set_fix_missing_seam_mode(&self, value: i32) {
        self.inner.set_fix_missing_seam_mode(value)
    }
    pub fn fix_small_area_wire_mode(&self) -> i32 {
        self.inner.fix_small_area_wire_mode()
    }
    pub fn set_fix_small_area_wire_mode(&self, value: i32) {
        self.inner.set_fix_small_area_wire_mode(value)
    }
    pub fn remove_small_area_face_mode(&self) -> i32 {
        self.inner.remove_small_area_face_mode()
    }
    pub fn set_remove_small_area_face_mode(&self, value: i32) {
        self.inner.set_remove_small_area_face_mode(value)
    }
    pub fn fix_intersecting_wires_mode(&self) -> i32 {
        self.inner.fix_intersecting_wires_mode()
    }
    pub fn set_fix_intersecting_wires_mode(&self, value: i32) {
        self.inner.set_fix_intersecting_wires_mode(value)
    }
    pub fn fix_loop_wires_mode(&self) -> i32 {
        self.inner.fix_loop_wires_mode()
    }
    pub fn set_fix_loop_wires_mode(&self, value: i32) {
        self.inner.set_fix_loop_wires_mode(value)
    }
    pub fn fix_split_face_mode(&self) -> i32 {
        self.inner.fix_split_face_mode()
    }
    pub fn set_fix_split_face_mode(&self, value: i32) {
        self.inner.set_fix_split_face_mode(value)
    }
    pub fn auto_correct_precision_mode(&self) -> i32 {
        self.inner.auto_correct_precision_mode()
    }
    pub fn set_auto_correct_precision_mode(&self, value: i32) {
        self.inner.set_auto_correct_precision_mode(value)
    }
    pub fn fix_periodic_degenerated_mode(&self) -> i32 {
        self.inner.fix_periodic_degenerated_mode()
    }
    pub fn set_fix_periodic_degenerated_mode(&self, value: i32) {
        self.inner.set_fix_periodic_degenerated_mode(value)
    }
}

impl Default for Face {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Face cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Shell` — the shell-level fixes.
pub struct Shell {
    inner: UniquePtr<ffi::FcShapeFixShell>,
}

impl Shell {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::fc_shapefix_shell_new())
    }

    pub fn from_shell(shell: &Shape) -> Option<Self> {
        Self::from_inner(with_shape(shell, ffi::fc_shapefix_shell_from_shell)?)
    }

    fn from_inner(inner: UniquePtr<ffi::FcShapeFixShell>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Shell { inner })
        }
    }

    pub fn init(&self, shell: &Shape) -> bool {
        with_occt!(shell, |shell| self.inner.init(shell))
    }

    pub fn perform(&self) -> bool {
        self.inner.perform()
    }
    pub fn shell(&self) -> Option<Shape> {
        output(self.inner.shell())
    }
    pub fn shape(&self) -> Option<Shape> {
        output(self.inner.shape())
    }
    pub fn error_faces(&self) -> Option<Shape> {
        output(self.inner.error_faces())
    }
    pub fn number_of_shells(&self) -> i32 {
        self.inner.nb_shells()
    }
    pub fn fix_face_tool(&self) -> Option<Face> {
        Face::from_inner(self.inner.fix_face_tool())
    }
    pub fn fix_face_orientation(
        &self,
        shell: &Shape,
        multi_connex: bool,
        non_manifold: bool,
    ) -> bool {
        with_occt!(shell, |shell| self
            .inner
            .fix_face_orientation(shell, multi_connex, non_manifold))
    }
    pub fn set_non_manifold_flag(&self, value: bool) {
        self.inner.set_non_manifold_flag(value)
    }
    pub fn fix_face_mode(&self) -> i32 {
        self.inner.fix_face_mode()
    }
    pub fn set_fix_face_mode(&self, value: i32) {
        self.inner.set_fix_face_mode(value)
    }
    pub fn fix_orientation_mode(&self) -> i32 {
        self.inner.fix_orientation_mode()
    }
    pub fn set_fix_orientation_mode(&self, value: i32) {
        self.inner.set_fix_orientation_mode(value)
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Shell cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Solid` — the solid-level fixes.
pub struct Solid {
    inner: UniquePtr<ffi::FcShapeFixSolid>,
}

impl Solid {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::fc_shapefix_solid_new())
    }

    pub fn from_solid(solid: &Shape) -> Option<Self> {
        Self::from_inner(with_shape(solid, ffi::fc_shapefix_solid_from_solid)?)
    }

    fn from_inner(inner: UniquePtr<ffi::FcShapeFixSolid>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Solid { inner })
        }
    }

    pub fn init(&self, solid: &Shape) -> bool {
        with_occt!(solid, |solid| self.inner.init(solid))
    }

    pub fn perform(&self) -> bool {
        self.inner.perform()
    }
    pub fn solid_from_shell(&self, shell: &Shape) -> Option<Shape> {
        with_occt!(shell, |shell| output(self.inner.solid_from_shell(shell)))
    }
    pub fn solid(&self) -> Option<Shape> {
        output(self.inner.solid())
    }
    pub fn shape(&self) -> Option<Shape> {
        output(self.inner.shape())
    }
    pub fn fix_shell_tool(&self) -> Option<Shell> {
        Shell::from_inner(self.inner.fix_shell_tool())
    }
    pub fn fix_shell_mode(&self) -> i32 {
        self.inner.fix_shell_mode()
    }
    pub fn set_fix_shell_mode(&self, value: i32) {
        self.inner.set_fix_shell_mode(value)
    }
    pub fn fix_shell_orientation_mode(&self) -> i32 {
        self.inner.fix_shell_orientation_mode()
    }
    pub fn set_fix_shell_orientation_mode(&self, value: i32) {
        self.inner.set_fix_shell_orientation_mode(value)
    }
    pub fn create_open_solid_mode(&self) -> bool {
        self.inner.create_open_solid_mode()
    }
    pub fn set_create_open_solid_mode(&self, value: bool) {
        self.inner.set_create_open_solid_mode(value)
    }
}

impl Default for Solid {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Solid cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Shape` — the whole-shape fixer.
pub struct ShapeFix {
    inner: UniquePtr<ffi::FcShapeFixShape>,
}

impl ShapeFix {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::fc_shapefix_shape_new())
    }

    pub fn from_shape(shape: &Shape) -> Option<Self> {
        Self::from_inner(with_shape(shape, ffi::fc_shapefix_shape_from_shape)?)
    }

    fn from_inner(inner: UniquePtr<ffi::FcShapeFixShape>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(ShapeFix { inner })
        }
    }

    pub fn init(&self, shape: &Shape) -> bool {
        with_occt!(shape, |shape| self.inner.init(shape))
    }

    pub fn perform(&self) -> bool {
        self.inner.perform()
    }
    pub fn shape(&self) -> Option<Shape> {
        output(self.inner.shape())
    }
    pub fn fix_solid_tool(&self) -> Option<Solid> {
        Solid::from_inner(self.inner.fix_solid_tool())
    }
    pub fn fix_shell_tool(&self) -> Option<Shell> {
        Shell::from_inner(self.inner.fix_shell_tool())
    }
    pub fn fix_face_tool(&self) -> Option<Face> {
        Face::from_inner(self.inner.fix_face_tool())
    }
    pub fn fix_wire_tool(&self) -> Option<Wire> {
        Wire::from_inner(self.inner.fix_wire_tool())
    }
    pub fn fix_edge_tool(&self) -> Option<Edge> {
        Edge::from_inner(self.inner.fix_edge_tool())
    }
    pub fn fix_solid_mode(&self) -> i32 {
        self.inner.fix_solid_mode()
    }
    pub fn set_fix_solid_mode(&self, value: i32) {
        self.inner.set_fix_solid_mode(value)
    }
    pub fn fix_free_shell_mode(&self) -> i32 {
        self.inner.fix_free_shell_mode()
    }
    pub fn set_fix_free_shell_mode(&self, value: i32) {
        self.inner.set_fix_free_shell_mode(value)
    }
    pub fn fix_free_face_mode(&self) -> i32 {
        self.inner.fix_free_face_mode()
    }
    pub fn set_fix_free_face_mode(&self, value: i32) {
        self.inner.set_fix_free_face_mode(value)
    }
    pub fn fix_free_wire_mode(&self) -> i32 {
        self.inner.fix_free_wire_mode()
    }
    pub fn set_fix_free_wire_mode(&self, value: i32) {
        self.inner.set_fix_free_wire_mode(value)
    }
    pub fn fix_same_parameter_mode(&self) -> i32 {
        self.inner.fix_same_parameter_mode()
    }
    pub fn set_fix_same_parameter_mode(&self, value: i32) {
        self.inner.set_fix_same_parameter_mode(value)
    }
    pub fn fix_vertex_position_mode(&self) -> i32 {
        self.inner.fix_vertex_position_mode()
    }
    pub fn set_fix_vertex_position_mode(&self, value: i32) {
        self.inner.set_fix_vertex_position_mode(value)
    }
    pub fn fix_vertex_tol_mode(&self) -> i32 {
        self.inner.fix_vertex_tol_mode()
    }
    pub fn set_fix_vertex_tol_mode(&self, value: i32) {
        self.inner.set_fix_vertex_tol_mode(value)
    }
}

impl Default for ShapeFix {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Shape cannot fail to allocate")
    }
}
