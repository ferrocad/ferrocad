//! The `ShapeFix_*` fixers behind `Part.ShapeFix`.
//!
//! The generic OCCT bindings live in `opencascade_sys::shape_fix`; this module
//! keeps the FerroCAD semantics (`Shape` in/out, `false`/`None` on failure,
//! plane anchoring for the surface overloads). The fixers own
//! `Handle(ShapeFix_*)`, so no `Pin` appears here.
//!
//! Input shapes are [`ferrocad_geom::Shape`] values (they must come from this
//! backend); outputs are owned `Shape` values again. A failed cast or an OCCT
//! exception becomes `false`/`None`, which the Python layer reports as
//! `Part.OCCError` rather than inventing a result.

use cxx::UniquePtr;
use ferrocad_geom::Shape;
use opencascade_sys as ffi;

use crate::shape::{downcast, erase, OcctShape};

use ffi::geom::Handle_Geom_Surface;
use ffi::shape_fix::{
    Handle_ShapeFix_Edge, Handle_ShapeFix_Face, Handle_ShapeFix_Root, Handle_ShapeFix_Shape,
    Handle_ShapeFix_Shell, Handle_ShapeFix_Solid, Handle_ShapeFix_Wire,
};
use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::topo_ds::{
    TopoDS, TopoDS_Compound, TopoDS_Face, TopoDS_Shape, TopoDS_Shell, TopoDS_Solid, TopoDS_Wire,
};

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

/// Upcast an owned sub-shape result and erase it into a [`Shape`].
fn output_wire(wire: UniquePtr<TopoDS_Wire>) -> Option<Shape> {
    let wire = wire.as_ref()?;
    Some(erase(ffi::topo_ds::cast_wire_to_shape(wire)))
}

fn output_face(face: UniquePtr<TopoDS_Face>) -> Option<Shape> {
    let face = face.as_ref()?;
    Some(erase(ffi::topo_ds::cast_face_to_shape(face)))
}

fn output_shell(shell: UniquePtr<TopoDS_Shell>) -> Option<Shape> {
    let shell = shell.as_ref()?;
    Some(erase(ffi::topo_ds::cast_shell_to_shape(shell)))
}

fn output_solid(solid: UniquePtr<TopoDS_Solid>) -> Option<Shape> {
    let solid = solid.as_ref()?;
    Some(erase(ffi::topo_ds::cast_solid_to_shape(solid)))
}

fn output_compound(compound: UniquePtr<TopoDS_Compound>) -> Option<Shape> {
    let compound = compound.as_ref()?;
    Some(erase(ffi::topo_ds::cast_compound_to_shape(compound)))
}

/// A plane through `origin` with direction `normal`.
fn gp_plane(origin: [f64; 3], normal: [f64; 3]) -> UniquePtr<ffi::gp::gp_Pln> {
    let point = ffi::gp::new_point(origin[0], origin[1], origin[2]);
    let direction = ffi::gp::gp_Dir_new(normal[0], normal[1], normal[2]);
    ffi::gp::gp_Pln_new(&point, &direction)
}

/// A plane whose normal is given but whose origin is a point of `anchor`. Used
/// for the surface overloads of `ShapeFix_Edge`, where Python hands us a
/// `Face.Surface` (a normal, no location) and an edge that lies on it: any
/// vertex of the edge is a point of the surface.
fn anchor_plane(anchor: &TopoDS_Shape, normal: [f64; 3]) -> UniquePtr<Handle_Geom_Surface> {
    let mut origin = ffi::gp::new_point(0.0, 0.0, 0.0);
    let explorer = ffi::top_exp::TopExp_Explorer_new(anchor, TopAbs_ShapeEnum::TopAbs_VERTEX);
    if explorer.More() {
        let vertex = TopoDS::Vertex(explorer.Current());
        origin = ffi::b_rep::BRep_Tool_Pnt(vertex);
    }
    let direction = ffi::gp::gp_Dir_new(normal[0], normal[1], normal[2]);
    let plane = ffi::gp::gp_Pln_new(&origin, &direction);
    ffi::geom::plane_to_surface(&ffi::geom::Geom_Plane_new(&plane))
}

/// A plane surface through `origin` with direction `normal`.
fn plane_surface(origin: [f64; 3], normal: [f64; 3]) -> UniquePtr<Handle_Geom_Surface> {
    let plane = ffi::geom::Geom_Plane_new(&gp_plane(origin, normal));
    ffi::geom::plane_to_surface(&plane)
}

/// `Part.ShapeFix.Root` — precision and tolerance limits shared by all fixers.
pub struct Root {
    inner: UniquePtr<Handle_ShapeFix_Root>,
}

impl Root {
    pub fn new() -> Self {
        Root {
            inner: ffi::shape_fix::ShapeFix_Root_new(),
        }
    }

    pub fn precision(&self) -> f64 {
        ffi::shape_fix::ShapeFix_Root_precision(&self.inner)
    }
    pub fn set_precision(&self, value: f64) {
        ffi::shape_fix::ShapeFix_Root_set_precision(&self.inner, value)
    }
    pub fn min_tolerance(&self) -> f64 {
        ffi::shape_fix::ShapeFix_Root_min_tolerance(&self.inner)
    }
    pub fn set_min_tolerance(&self, value: f64) {
        ffi::shape_fix::ShapeFix_Root_set_min_tolerance(&self.inner, value)
    }
    pub fn max_tolerance(&self) -> f64 {
        ffi::shape_fix::ShapeFix_Root_max_tolerance(&self.inner)
    }
    pub fn set_max_tolerance(&self, value: f64) {
        ffi::shape_fix::ShapeFix_Root_set_max_tolerance(&self.inner, value)
    }
    pub fn limit_tolerance(&self, value: f64) -> f64 {
        ffi::shape_fix::ShapeFix_Root_limit_tolerance(&self.inner, value)
    }
}

impl Default for Root {
    fn default() -> Self {
        Self::new()
    }
}

/// `Part.ShapeFix.Edge` — the edge-level fixes.
pub struct Edge {
    inner: UniquePtr<Handle_ShapeFix_Edge>,
}

impl Edge {
    pub fn new() -> Option<Self> {
        Edge::from_inner(ffi::shape_fix::ShapeFix_Edge_new())
    }

    fn from_inner(inner: UniquePtr<Handle_ShapeFix_Edge>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Edge { inner })
        }
    }

    pub fn remove_pcurve_face(&self, edge: &Shape, face: &Shape) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| ffi::shape_fix::ShapeFix_Edge_FixRemovePCurve_face(
            &self.inner,
            TopoDS::Edge(edge),
            TopoDS::Face(face),
        )
        .unwrap_or(false)))
    }

    /// The surface overload: `Face.Surface` carries a normal but no location,
    /// so the plane is anchored at a vertex of the edge (which lies on it).
    pub fn remove_pcurve_surface(&self, edge: &Shape, normal: [f64; 3]) -> bool {
        with_occt!(edge, |edge| {
            let surface = anchor_plane(edge, normal);
            let location = ffi::top_loc::Location_new();
            ffi::shape_fix::ShapeFix_Edge_FixRemovePCurve_surface(
                &self.inner,
                TopoDS::Edge(edge),
                &surface,
                &location,
            )
            .unwrap_or(false)
        })
    }

    pub fn remove_curve3d(&self, edge: &Shape) -> bool {
        with_occt!(edge, |edge| ffi::shape_fix::ShapeFix_Edge_FixRemoveCurve3d(
            &self.inner,
            TopoDS::Edge(edge),
        )
        .unwrap_or(false))
    }

    pub fn add_pcurve_face(&self, edge: &Shape, face: &Shape, seam: bool, prec: f64) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| ffi::shape_fix::ShapeFix_Edge_FixAddPCurve_face(
            &self.inner,
            TopoDS::Edge(edge),
            TopoDS::Face(face),
            seam,
            prec,
        )
        .unwrap_or(false)))
    }

    pub fn add_pcurve_surface(
        &self,
        edge: &Shape,
        normal: [f64; 3],
        seam: bool,
        prec: f64,
    ) -> bool {
        with_occt!(edge, |edge| {
            let surface = anchor_plane(edge, normal);
            let location = ffi::top_loc::Location_new();
            ffi::shape_fix::ShapeFix_Edge_FixAddPCurve_surface(
                &self.inner,
                TopoDS::Edge(edge),
                &surface,
                &location,
                seam,
                prec,
            )
            .unwrap_or(false)
        })
    }

    pub fn add_curve3d(&self, edge: &Shape) -> bool {
        with_occt!(edge, |edge| ffi::shape_fix::ShapeFix_Edge_FixAddCurve3d(
            &self.inner,
            TopoDS::Edge(edge),
        )
        .unwrap_or(false))
    }

    pub fn vertex_tolerance_face(&self, edge: &Shape, face: &Shape) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| ffi::shape_fix::ShapeFix_Edge_FixVertexTolerance_face(
            &self.inner,
            TopoDS::Edge(edge),
            TopoDS::Face(face),
        )
        .unwrap_or(false)))
    }

    pub fn vertex_tolerance(&self, edge: &Shape) -> bool {
        with_occt!(edge, |edge| ffi::shape_fix::ShapeFix_Edge_FixVertexTolerance(
            &self.inner,
            TopoDS::Edge(edge),
        )
        .unwrap_or(false))
    }

    pub fn reversed2d_face(&self, edge: &Shape, face: &Shape) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| ffi::shape_fix::ShapeFix_Edge_FixReversed2d_face(
            &self.inner,
            TopoDS::Edge(edge),
            TopoDS::Face(face),
        )
        .unwrap_or(false)))
    }

    pub fn reversed2d_surface(&self, edge: &Shape, normal: [f64; 3]) -> bool {
        with_occt!(edge, |edge| {
            let surface = anchor_plane(edge, normal);
            let location = ffi::top_loc::Location_new();
            ffi::shape_fix::ShapeFix_Edge_FixReversed2d_surface(
                &self.inner,
                TopoDS::Edge(edge),
                &surface,
                &location,
            )
            .unwrap_or(false)
        })
    }

    pub fn same_parameter(&self, edge: &Shape, tolerance: f64) -> bool {
        with_occt!(edge, |edge| ffi::shape_fix::ShapeFix_Edge_FixSameParameter(
            &self.inner,
            TopoDS::Edge(edge),
            tolerance,
        )
        .unwrap_or(false))
    }

    pub fn same_parameter_face(&self, edge: &Shape, face: &Shape, tolerance: f64) -> bool {
        with_occt!(edge, |edge| with_occt!(face, |face| ffi::shape_fix::ShapeFix_Edge_FixSameParameter_face(
            &self.inner,
            TopoDS::Edge(edge),
            TopoDS::Face(face),
            tolerance,
        )
        .unwrap_or(false)))
    }
}

impl Default for Edge {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Edge cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Wire` — the wire-level fixes and their modes.
pub struct Wire {
    inner: UniquePtr<Handle_ShapeFix_Wire>,
}

impl Wire {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::shape_fix::ShapeFix_Wire_new())
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
        Self::from_inner(
            ffi::shape_fix::ShapeFix_Wire_from_wire(TopoDS::Wire(wire), TopoDS::Face(face), prec)
                .ok()?,
        )
    }

    fn from_inner(inner: UniquePtr<Handle_ShapeFix_Wire>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Wire { inner })
        }
    }

    pub fn init(&self, wire: &Shape, face: &Shape, prec: f64) -> bool {
        with_occt!(wire, |wire| with_occt!(face, |face| {
            let _ = ffi::shape_fix::ShapeFix_Wire_Init(
                &self.inner,
                TopoDS::Wire(wire),
                TopoDS::Face(face),
                prec,
            );
            true
        }))
    }

    pub fn load(&self, wire: &Shape) -> bool {
        with_occt!(wire, |wire| ffi::shape_fix::ShapeFix_Wire_Load(
            &self.inner,
            TopoDS::Wire(wire),
        )
        .is_ok())
    }

    /// `ShapeFix_Root::SetPrecision` - the tolerance `fixWire(tolerance)` uses
    /// when no face is supplied.
    pub fn set_precision(&self, value: f64) {
        let root = ffi::shape_fix::ShapeFix_Wire_as_root(&self.inner);
        ffi::shape_fix::ShapeFix_Root_set_precision(root, value);
    }

    pub fn set_face(&self, face: &Shape) -> bool {
        with_occt!(face, |face| ffi::shape_fix::ShapeFix_Wire_SetFace(
            &self.inner,
            TopoDS::Face(face),
        )
        .is_ok())
    }

    pub fn set_surface(&self, origin: [f64; 3], normal: [f64; 3], placed: bool) -> bool {
        let surface = plane_surface(origin, normal);
        if placed {
            let location = ffi::top_loc::Location_new();
            ffi::shape_fix::ShapeFix_Wire_SetSurface_placed(&self.inner, &surface, &location).is_ok()
        } else {
            ffi::shape_fix::ShapeFix_Wire_SetSurface(&self.inner, &surface).is_ok()
        }
    }

    pub fn set_max_tail_angle(&self, angle: f64) {
        ffi::shape_fix::ShapeFix_Wire_SetMaxTailAngle(&self.inner, angle)
    }
    pub fn set_max_tail_width(&self, width: f64) {
        ffi::shape_fix::ShapeFix_Wire_SetMaxTailWidth(&self.inner, width)
    }
    pub fn is_loaded(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_IsLoaded(&self.inner)
    }
    pub fn is_ready(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_IsReady(&self.inner)
    }
    pub fn number_of_edges(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_NbEdges(&self.inner)
    }
    pub fn wire(&self) -> Option<Shape> {
        output_wire(ffi::shape_fix::ShapeFix_Wire_Wire(&self.inner).ok()?)
    }
    pub fn wire_api_make(&self) -> Option<Shape> {
        output_wire(ffi::shape_fix::ShapeFix_Wire_WireAPIMake(&self.inner).ok()?)
    }
    pub fn face(&self) -> Option<Shape> {
        output_face(ffi::shape_fix::ShapeFix_Wire_Face(&self.inner).ok()?)
    }
    pub fn perform(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_Perform(&self.inner).unwrap_or(false)
    }
    pub fn clear_modes(&self) {
        ffi::shape_fix::ShapeFix_Wire_ClearModes(&self.inner)
    }
    pub fn clear_statuses(&self) {
        ffi::shape_fix::ShapeFix_Wire_ClearStatuses(&self.inner)
    }
    pub fn fix_edge_tool(&self) -> Option<Edge> {
        Edge::from_inner(ffi::shape_fix::ShapeFix_Wire_FixEdgeTool(&self.inner))
    }

    pub fn fix_reorder(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixReorder(&self.inner).unwrap_or(false)
    }
    pub fn fix_small_global(&self, lock: bool, prec: f64) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixSmall_global(&self.inner, lock, prec).unwrap_or(0)
    }
    pub fn fix_small_one(&self, num: i32, lock: bool, prec: f64) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixSmall_one(&self.inner, num, lock, prec).unwrap_or(false)
    }
    pub fn fix_connected(&self, prec: f64) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixConnected(&self.inner, prec).unwrap_or(false)
    }
    pub fn fix_connected_one(&self, num: i32, prec: f64) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixConnected_one(&self.inner, num, prec).unwrap_or(false)
    }
    pub fn fix_edge_curves(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixEdgeCurves(&self.inner).unwrap_or(false)
    }
    pub fn fix_degenerated(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixDegenerated(&self.inner).unwrap_or(false)
    }
    pub fn fix_degenerated_one(&self, num: i32) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixDegenerated_one(&self.inner, num).unwrap_or(false)
    }
    pub fn fix_self_intersection(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixSelfIntersection(&self.inner).unwrap_or(false)
    }
    pub fn fix_lacking(&self, force: bool) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixLacking(&self.inner, force).unwrap_or(false)
    }
    pub fn fix_lacking_one(&self, num: i32, force: bool) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixLacking_one(&self.inner, num, force).unwrap_or(false)
    }
    pub fn fix_closed(&self, prec: f64) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixClosed(&self.inner, prec).unwrap_or(false)
    }
    pub fn fix_gaps3d(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixGaps3d(&self.inner).unwrap_or(false)
    }
    pub fn fix_gaps2d(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixGaps2d(&self.inner).unwrap_or(false)
    }
    pub fn fix_seam(&self, num: i32) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixSeam(&self.inner, num).unwrap_or(false)
    }
    pub fn fix_shifted(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixShifted(&self.inner).unwrap_or(false)
    }
    pub fn fix_notched_edges(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixNotchedEdges(&self.inner).unwrap_or(false)
    }
    pub fn fix_gap3d(&self, num: i32, convert: bool) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixGap3d(&self.inner, num, convert).unwrap_or(false)
    }
    pub fn fix_gap2d(&self, num: i32, convert: bool) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixGap2d(&self.inner, num, convert).unwrap_or(false)
    }
    pub fn fix_tails(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixTails(&self.inner).unwrap_or(false)
    }

    // Modes reported as booleans where OCCT stores a `Standard_Boolean`, and as
    // integers where it stores a three-state `Standard_Integer`.
    pub fn modify_topology_mode(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_ModifyTopologyMode(&self.inner)
    }
    pub fn set_modify_topology_mode(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Wire_set_ModifyTopologyMode(&self.inner, value)
    }
    pub fn modify_geometry_mode(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_ModifyGeometryMode(&self.inner)
    }
    pub fn set_modify_geometry_mode(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Wire_set_ModifyGeometryMode(&self.inner, value)
    }
    pub fn closed_wire_mode(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_ClosedWireMode(&self.inner)
    }
    pub fn set_closed_wire_mode(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Wire_set_ClosedWireMode(&self.inner, value)
    }
    pub fn preference_pcurve_mode(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_PreferencePCurveMode(&self.inner)
    }
    pub fn set_preference_pcurve_mode(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Wire_set_PreferencePCurveMode(&self.inner, value)
    }
    pub fn fix_gaps_by_ranges_mode(&self) -> bool {
        ffi::shape_fix::ShapeFix_Wire_FixGapsByRangesMode(&self.inner)
    }
    pub fn set_fix_gaps_by_ranges_mode(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Wire_set_FixGapsByRangesMode(&self.inner, value)
    }
    pub fn modify_remove_loop_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_ModifyRemoveLoopMode(&self.inner)
    }
    pub fn set_modify_remove_loop_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_ModifyRemoveLoopMode(&self.inner, value)
    }
    pub fn fix_reorder_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixReorderMode(&self.inner)
    }
    pub fn set_fix_reorder_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixReorderMode(&self.inner, value)
    }
    pub fn fix_small_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixSmallMode(&self.inner)
    }
    pub fn set_fix_small_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixSmallMode(&self.inner, value)
    }
    pub fn fix_connected_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixConnectedMode(&self.inner)
    }
    pub fn set_fix_connected_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixConnectedMode(&self.inner, value)
    }
    pub fn fix_edge_curves_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixEdgeCurvesMode(&self.inner)
    }
    pub fn set_fix_edge_curves_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixEdgeCurvesMode(&self.inner, value)
    }
    pub fn fix_degenerated_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixDegeneratedMode(&self.inner)
    }
    pub fn set_fix_degenerated_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixDegeneratedMode(&self.inner, value)
    }
    pub fn fix_self_intersection_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixSelfIntersectionMode(&self.inner)
    }
    pub fn set_fix_self_intersection_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixSelfIntersectionMode(&self.inner, value)
    }
    pub fn fix_lacking_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixLackingMode(&self.inner)
    }
    pub fn set_fix_lacking_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixLackingMode(&self.inner, value)
    }
    pub fn fix_gaps3d_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixGaps3dMode(&self.inner)
    }
    pub fn set_fix_gaps3d_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixGaps3dMode(&self.inner, value)
    }
    pub fn fix_gaps2d_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixGaps2dMode(&self.inner)
    }
    pub fn set_fix_gaps2d_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixGaps2dMode(&self.inner, value)
    }
    pub fn fix_reversed2d_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixReversed2dMode(&self.inner)
    }
    pub fn set_fix_reversed2d_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixReversed2dMode(&self.inner, value)
    }
    pub fn fix_remove_pcurve_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixRemovePCurveMode(&self.inner)
    }
    pub fn set_fix_remove_pcurve_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixRemovePCurveMode(&self.inner, value)
    }
    pub fn fix_add_pcurve_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixAddPCurveMode(&self.inner)
    }
    pub fn set_fix_add_pcurve_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixAddPCurveMode(&self.inner, value)
    }
    pub fn fix_remove_curve3d_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixRemoveCurve3dMode(&self.inner)
    }
    pub fn set_fix_remove_curve3d_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixRemoveCurve3dMode(&self.inner, value)
    }
    pub fn fix_add_curve3d_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixAddCurve3dMode(&self.inner)
    }
    pub fn set_fix_add_curve3d_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixAddCurve3dMode(&self.inner, value)
    }
    pub fn fix_seam_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixSeamMode(&self.inner)
    }
    pub fn set_fix_seam_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixSeamMode(&self.inner, value)
    }
    pub fn fix_shifted_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixShiftedMode(&self.inner)
    }
    pub fn set_fix_shifted_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixShiftedMode(&self.inner, value)
    }
    pub fn fix_same_parameter_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixSameParameterMode(&self.inner)
    }
    pub fn set_fix_same_parameter_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixSameParameterMode(&self.inner, value)
    }
    pub fn fix_vertex_tolerance_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixVertexToleranceMode(&self.inner)
    }
    pub fn set_fix_vertex_tolerance_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixVertexToleranceMode(&self.inner, value)
    }
    pub fn fix_notched_edges_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixNotchedEdgesMode(&self.inner)
    }
    pub fn set_fix_notched_edges_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixNotchedEdgesMode(&self.inner, value)
    }
    pub fn fix_self_intersecting_edge_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixSelfIntersectingEdgeMode(&self.inner)
    }
    pub fn set_fix_self_intersecting_edge_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixSelfIntersectingEdgeMode(&self.inner, value)
    }
    pub fn fix_intersecting_edges_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixIntersectingEdgesMode(&self.inner)
    }
    pub fn set_fix_intersecting_edges_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixIntersectingEdgesMode(&self.inner, value)
    }
    pub fn fix_non_adjacent_intersecting_edges_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixNonAdjacentIntersectingEdgesMode(&self.inner)
    }
    pub fn set_fix_non_adjacent_intersecting_edges_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixNonAdjacentIntersectingEdgesMode(&self.inner, value)
    }
    pub fn fix_tail_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Wire_FixTailMode(&self.inner)
    }
    pub fn set_fix_tail_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Wire_set_FixTailMode(&self.inner, value)
    }
}

impl Default for Wire {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Wire cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Face` — the face-level fixes.
pub struct Face {
    inner: UniquePtr<Handle_ShapeFix_Face>,
}

impl Face {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::shape_fix::ShapeFix_Face_new())
    }

    pub fn from_face(face: &Shape) -> Option<Self> {
        Self::from_inner(
            with_shape(face, |face| {
                ffi::shape_fix::ShapeFix_Face_from_face(TopoDS::Face(face))
            })?
            .ok()?,
        )
    }

    pub fn from_plane(
        origin: [f64; 3],
        normal: [f64; 3],
        prec: f64,
        forward: bool,
    ) -> Option<Self> {
        let surface = plane_surface(origin, normal);
        let inner = ffi::shape_fix::ShapeFix_Face_new();
        ffi::shape_fix::ShapeFix_Face_Init_surface(&inner, &surface, prec, forward).ok()?;
        Self::from_inner(inner)
    }

    fn from_inner(inner: UniquePtr<Handle_ShapeFix_Face>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Face { inner })
        }
    }

    pub fn init_face(&self, face: &Shape) -> bool {
        with_occt!(face, |face| ffi::shape_fix::ShapeFix_Face_Init(
            &self.inner,
            TopoDS::Face(face),
        )
        .is_ok())
    }

    pub fn init_plane(
        &self,
        origin: [f64; 3],
        normal: [f64; 3],
        prec: f64,
        forward: bool,
    ) -> bool {
        let surface = plane_surface(origin, normal);
        ffi::shape_fix::ShapeFix_Face_Init_surface(&self.inner, &surface, prec, forward).is_ok()
    }

    pub fn add(&self, wire: &Shape) -> bool {
        with_occt!(wire, |wire| ffi::shape_fix::ShapeFix_Face_Add(
            &self.inner,
            TopoDS::Wire(wire),
        )
        .is_ok())
    }

    pub fn perform(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_Perform(&self.inner).unwrap_or(false)
    }
    pub fn fix_orientation(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixOrientation(&self.inner).unwrap_or(false)
    }
    pub fn fix_add_natural_bound(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixAddNaturalBound(&self.inner).unwrap_or(false)
    }
    pub fn fix_missing_seam(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixMissingSeam(&self.inner).unwrap_or(false)
    }
    pub fn fix_small_area_wire(&self, remove_small: bool) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixSmallAreaWire(&self.inner, remove_small).unwrap_or(false)
    }

    /// `(fixed, replaced_wires)` — upstream's `(bool, list)` tuple.
    pub fn fix_loop_wire(&self) -> (bool, Vec<Shape>) {
        let mut wires = ffi::top_tools::new_Handle_TopTools_HSequenceOfShape();
        let Ok(ok) = ffi::shape_fix::ShapeFix_Face_FixLoopWire(&self.inner, wires.pin_mut())
        else {
            return (false, Vec::new());
        };
        let count = ffi::top_tools::TopTools_HSequenceOfShape_length(&wires);
        let replaced = (1..=count)
            .map(|index| erase(ffi::top_tools::TopTools_HSequenceOfShape_value(&wires, index)))
            .collect();
        (ok, replaced)
    }

    pub fn fix_intersecting_wires(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixIntersectingWires(&self.inner).unwrap_or(false)
    }
    pub fn fix_wires_two_coincident_edges(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixWiresTwoCoincEdges(&self.inner).unwrap_or(false)
    }
    pub fn fix_periodic_degenerated(&self) -> bool {
        ffi::shape_fix::ShapeFix_Face_FixPeriodicDegenerated(&self.inner).unwrap_or(false)
    }
    pub fn face(&self) -> Option<Shape> {
        output_face(ffi::shape_fix::ShapeFix_Face_Face(&self.inner).ok()?)
    }
    pub fn result(&self) -> Option<Shape> {
        output(ffi::shape_fix::ShapeFix_Face_Result(&self.inner).ok()?)
    }
    pub fn fix_wire_tool(&self) -> Option<Wire> {
        Wire::from_inner(ffi::shape_fix::ShapeFix_Face_FixWireTool(&self.inner))
    }
    pub fn clear_modes(&self) {
        ffi::shape_fix::ShapeFix_Face_ClearModes(&self.inner)
    }

    pub fn fix_wire_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixWireMode(&self.inner)
    }
    pub fn set_fix_wire_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixWireMode(&self.inner, value)
    }
    pub fn fix_orientation_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixOrientationMode(&self.inner)
    }
    pub fn set_fix_orientation_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixOrientationMode(&self.inner, value)
    }
    pub fn fix_add_natural_bound_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixAddNaturalBoundMode(&self.inner)
    }
    pub fn set_fix_add_natural_bound_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixAddNaturalBoundMode(&self.inner, value)
    }
    pub fn fix_missing_seam_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixMissingSeamMode(&self.inner)
    }
    pub fn set_fix_missing_seam_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixMissingSeamMode(&self.inner, value)
    }
    pub fn fix_small_area_wire_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixSmallAreaWireMode(&self.inner)
    }
    pub fn set_fix_small_area_wire_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixSmallAreaWireMode(&self.inner, value)
    }
    pub fn remove_small_area_face_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_RemoveSmallAreaFaceMode(&self.inner)
    }
    pub fn set_remove_small_area_face_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_RemoveSmallAreaFaceMode(&self.inner, value)
    }
    pub fn fix_intersecting_wires_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixIntersectingWiresMode(&self.inner)
    }
    pub fn set_fix_intersecting_wires_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixIntersectingWiresMode(&self.inner, value)
    }
    pub fn fix_loop_wires_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixLoopWiresMode(&self.inner)
    }
    pub fn set_fix_loop_wires_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixLoopWiresMode(&self.inner, value)
    }
    pub fn fix_split_face_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixSplitFaceMode(&self.inner)
    }
    pub fn set_fix_split_face_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixSplitFaceMode(&self.inner, value)
    }
    pub fn auto_correct_precision_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_AutoCorrectPrecisionMode(&self.inner)
    }
    pub fn set_auto_correct_precision_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_AutoCorrectPrecisionMode(&self.inner, value)
    }
    pub fn fix_periodic_degenerated_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Face_FixPeriodicDegeneratedMode(&self.inner)
    }
    pub fn set_fix_periodic_degenerated_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Face_set_FixPeriodicDegeneratedMode(&self.inner, value)
    }
}

impl Default for Face {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Face cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Shell` — the shell-level fixes.
pub struct Shell {
    inner: UniquePtr<Handle_ShapeFix_Shell>,
}

impl Shell {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::shape_fix::ShapeFix_Shell_new())
    }

    pub fn from_shell(shell: &Shape) -> Option<Self> {
        Self::from_inner(
            with_shape(shell, |shell| {
                ffi::shape_fix::ShapeFix_Shell_from_shell(TopoDS::Shell(shell))
            })?
            .ok()?,
        )
    }

    fn from_inner(inner: UniquePtr<Handle_ShapeFix_Shell>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Shell { inner })
        }
    }

    pub fn init(&self, shell: &Shape) -> bool {
        with_occt!(shell, |shell| ffi::shape_fix::ShapeFix_Shell_Init(
            &self.inner,
            TopoDS::Shell(shell),
        )
        .is_ok())
    }

    pub fn perform(&self) -> bool {
        ffi::shape_fix::ShapeFix_Shell_Perform(&self.inner).unwrap_or(false)
    }
    pub fn shell(&self) -> Option<Shape> {
        output_shell(ffi::shape_fix::ShapeFix_Shell_Shell(&self.inner).ok()?)
    }
    pub fn shape(&self) -> Option<Shape> {
        output(ffi::shape_fix::ShapeFix_Shell_Shape(&self.inner).ok()?)
    }
    pub fn error_faces(&self) -> Option<Shape> {
        output_compound(ffi::shape_fix::ShapeFix_Shell_ErrorFaces(&self.inner).ok()?)
    }
    pub fn number_of_shells(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shell_NbShells(&self.inner)
    }
    pub fn fix_face_tool(&self) -> Option<Face> {
        Face::from_inner(ffi::shape_fix::ShapeFix_Shell_FixFaceTool(&self.inner))
    }
    pub fn fix_face_orientation(
        &self,
        shell: &Shape,
        multi_connex: bool,
        non_manifold: bool,
    ) -> bool {
        with_occt!(shell, |shell| ffi::shape_fix::ShapeFix_Shell_FixFaceOrientation(
            &self.inner,
            TopoDS::Shell(shell),
            multi_connex,
            non_manifold,
        )
        .unwrap_or(false))
    }
    pub fn set_non_manifold_flag(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Shell_SetNonManifoldFlag(&self.inner, value)
    }
    pub fn fix_face_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shell_FixFaceMode(&self.inner)
    }
    pub fn set_fix_face_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shell_set_FixFaceMode(&self.inner, value)
    }
    pub fn fix_orientation_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shell_FixOrientationMode(&self.inner)
    }
    pub fn set_fix_orientation_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shell_set_FixOrientationMode(&self.inner, value)
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Shell cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Solid` — the solid-level fixes.
pub struct Solid {
    inner: UniquePtr<Handle_ShapeFix_Solid>,
}

impl Solid {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::shape_fix::ShapeFix_Solid_new())
    }

    pub fn from_solid(solid: &Shape) -> Option<Self> {
        Self::from_inner(
            with_shape(solid, |solid| {
                ffi::shape_fix::ShapeFix_Solid_from_solid(TopoDS::Solid(solid))
            })?
            .ok()?,
        )
    }

    fn from_inner(inner: UniquePtr<Handle_ShapeFix_Solid>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(Solid { inner })
        }
    }

    pub fn init(&self, solid: &Shape) -> bool {
        with_occt!(solid, |solid| ffi::shape_fix::ShapeFix_Solid_Init(
            &self.inner,
            TopoDS::Solid(solid),
        )
        .is_ok())
    }

    pub fn perform(&self) -> bool {
        ffi::shape_fix::ShapeFix_Solid_Perform(&self.inner).unwrap_or(false)
    }
    pub fn solid_from_shell(&self, shell: &Shape) -> Option<Shape> {
        with_occt!(shell, |shell| output_solid(
            ffi::shape_fix::ShapeFix_Solid_SolidFromShell(&self.inner, TopoDS::Shell(shell)).ok()?
        ))
    }
    pub fn solid(&self) -> Option<Shape> {
        output(ffi::shape_fix::ShapeFix_Solid_Solid(&self.inner).ok()?)
    }
    pub fn shape(&self) -> Option<Shape> {
        output(ffi::shape_fix::ShapeFix_Solid_Shape(&self.inner).ok()?)
    }
    pub fn fix_shell_tool(&self) -> Option<Shell> {
        Shell::from_inner(ffi::shape_fix::ShapeFix_Solid_FixShellTool(&self.inner))
    }
    pub fn fix_shell_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Solid_FixShellMode(&self.inner)
    }
    pub fn set_fix_shell_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Solid_set_FixShellMode(&self.inner, value)
    }
    pub fn fix_shell_orientation_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Solid_FixShellOrientationMode(&self.inner)
    }
    pub fn set_fix_shell_orientation_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Solid_set_FixShellOrientationMode(&self.inner, value)
    }
    pub fn create_open_solid_mode(&self) -> bool {
        ffi::shape_fix::ShapeFix_Solid_CreateOpenSolidMode(&self.inner)
    }
    pub fn set_create_open_solid_mode(&self, value: bool) {
        ffi::shape_fix::ShapeFix_Solid_set_CreateOpenSolidMode(&self.inner, value)
    }
}

impl Default for Solid {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Solid cannot fail to allocate")
    }
}

/// `Part.ShapeFix.Shape` — the whole-shape fixer.
pub struct ShapeFix {
    inner: UniquePtr<Handle_ShapeFix_Shape>,
}

impl ShapeFix {
    pub fn new() -> Option<Self> {
        Self::from_inner(ffi::shape_fix::ShapeFix_Shape_new())
    }

    pub fn from_shape(shape: &Shape) -> Option<Self> {
        Self::from_inner(
            with_shape(shape, ffi::shape_fix::ShapeFix_Shape_from_shape)?.ok()?,
        )
    }

    fn from_inner(inner: UniquePtr<Handle_ShapeFix_Shape>) -> Option<Self> {
        if inner.is_null() {
            None
        } else {
            Some(ShapeFix { inner })
        }
    }

    pub fn init(&self, shape: &Shape) -> bool {
        with_occt!(shape, |shape| ffi::shape_fix::ShapeFix_Shape_Init(
            &self.inner,
            shape,
        )
        .is_ok())
    }

    pub fn perform(&self) -> bool {
        ffi::shape_fix::ShapeFix_Shape_Perform(&self.inner).unwrap_or(false)
    }
    pub fn shape(&self) -> Option<Shape> {
        output(ffi::shape_fix::ShapeFix_Shape_Shape(&self.inner).ok()?)
    }
    pub fn fix_solid_tool(&self) -> Option<Solid> {
        Solid::from_inner(ffi::shape_fix::ShapeFix_Shape_FixSolidTool(&self.inner))
    }
    pub fn fix_shell_tool(&self) -> Option<Shell> {
        Shell::from_inner(ffi::shape_fix::ShapeFix_Shape_FixShellTool(&self.inner))
    }
    pub fn fix_face_tool(&self) -> Option<Face> {
        Face::from_inner(ffi::shape_fix::ShapeFix_Shape_FixFaceTool(&self.inner))
    }
    pub fn fix_wire_tool(&self) -> Option<Wire> {
        Wire::from_inner(ffi::shape_fix::ShapeFix_Shape_FixWireTool(&self.inner))
    }
    pub fn fix_edge_tool(&self) -> Option<Edge> {
        Edge::from_inner(ffi::shape_fix::ShapeFix_Shape_FixEdgeTool(&self.inner))
    }
    pub fn fix_solid_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixSolidMode(&self.inner)
    }
    pub fn set_fix_solid_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixSolidMode(&self.inner, value)
    }
    pub fn fix_free_shell_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixFreeShellMode(&self.inner)
    }
    pub fn set_fix_free_shell_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixFreeShellMode(&self.inner, value)
    }
    pub fn fix_free_face_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixFreeFaceMode(&self.inner)
    }
    pub fn set_fix_free_face_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixFreeFaceMode(&self.inner, value)
    }
    pub fn fix_free_wire_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixFreeWireMode(&self.inner)
    }
    pub fn set_fix_free_wire_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixFreeWireMode(&self.inner, value)
    }
    pub fn fix_same_parameter_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixSameParameterMode(&self.inner)
    }
    pub fn set_fix_same_parameter_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixSameParameterMode(&self.inner, value)
    }
    pub fn fix_vertex_position_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixVertexPositionMode(&self.inner)
    }
    pub fn set_fix_vertex_position_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixVertexPositionMode(&self.inner, value)
    }
    pub fn fix_vertex_tol_mode(&self) -> i32 {
        ffi::shape_fix::ShapeFix_Shape_FixVertexTolMode(&self.inner)
    }
    pub fn set_fix_vertex_tol_mode(&self, value: i32) {
        ffi::shape_fix::ShapeFix_Shape_set_FixVertexTolMode(&self.inner, value)
    }
}

impl Default for ShapeFix {
    fn default() -> Self {
        Self::new().expect("a ShapeFix_Shape cannot fail to allocate")
    }
}
