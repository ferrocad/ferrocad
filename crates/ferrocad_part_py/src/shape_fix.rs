//! `Part.ShapeFix` — the OCCT shape-fixing tools.
//!
//! Each Python class wraps one [`ferrocad_occt::shapefix`] fixer, which in turn
//! wraps the real OCCT `ShapeFix_*` object. Constructors and methods mirror the
//! upstream `ShapeFix_*Py` argument lists (including their overloads and their
//! `TypeError`s); shapes cross the boundary as `Part.Shape`.
//!
//! Outputs are wrapped as `Part.Shape` rather than the specialised
//! `Part.Face`/`Part.Wire`/`Part.Shell`/`Part.Solid` subclasses: the tests call
//! those methods for their shape, and every sub-shape query (`Faces`, `Edges`,
//! `Shells`, …) is available on `Part.Shape` itself.

use ferrocad_geom::Shape;
use ferrocad_occt::shapefix::{
    Edge as EdgeFixer, Face as FaceFixer, Root as RootFixer, ShapeFix as ShapeFixer,
    Shell as ShellFixer, Solid as SolidFixer, Wire as WireFixer,
};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyModule, PyTuple};

use crate::curves::{self, OCCError};
use crate::PartShape;

/// Read a `Part.Shape` (any subclass) as a kernel shape.
fn shape_of(value: &Bound<'_, PyAny>, what: &str) -> PyResult<Shape> {
    let shape = value
        .downcast::<PartShape>()
        .map_err(|_| PyTypeError::new_err(format!("{what} expects a Part.Shape")))?;
    shape
        .borrow()
        .shape()
        .ok_or_else(|| OCCError::new_err(format!("a null shape cannot be used as {what}")))
}

/// Read a `Part.Edge` as a kernel shape (upstream's `TopoShapeEdgePy`).
fn edge_shape(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let edge = value
        .downcast::<curves::PyEdge>()
        .map_err(|_| PyTypeError::new_err("expected a Part.Edge"))?;
    edge.borrow()
        .entry()
        .map(|entry| entry.shape)
        .ok_or_else(|| OCCError::new_err("an empty Part.Edge has no geometry"))
}

/// Read a `Part.Face` as a kernel shape (upstream's `TopoShapeFacePy`).
fn face_shape(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let face = value
        .downcast::<curves::PyFace>()
        .map_err(|_| PyTypeError::new_err("expected a Part.Face"))?;
    let guard = face.borrow();
    guard
        .as_super()
        .shape()
        .ok_or_else(|| OCCError::new_err("a null Part.Face has no geometry"))
}

/// The origin and unit normal of a planar surface object.
///
/// `Part.Plane` carries both; `Face.Surface` (`Part::GeomPlane`) carries only
/// the normal, so its origin is the world origin. The OCCT surface overloads
/// anchor their plane at a vertex of the edge, so only the normal is used
/// there; `ShapeFix_Wire.setSurface` does use the origin.
fn plane_of(value: &Bound<'_, PyAny>) -> PyResult<([f64; 3], [f64; 3])> {
    if value.downcast::<curves::PyPlane>().is_ok() {
        let origin = curves::vector(&value.getattr("Location")?)?;
        let normal = curves::vector(&value.getattr("Axis")?)?;
        return Ok((
            [origin.x, origin.y, origin.z],
            [normal.x, normal.y, normal.z],
        ));
    }
    if let Ok(surface) = value.downcast::<crate::solids::PyPlaneSurface>() {
        let normal = surface.call_method1("normal", (0.0_f64, 0.0_f64))?;
        let normal = curves::vector(&normal)?;
        return Ok(([0.0, 0.0, 0.0], [normal.x, normal.y, normal.z]));
    }
    Err(PyTypeError::new_err(
        "expected a planar surface (Part.Plane or Face.Surface)",
    ))
}

/// A result shape, or an `OCCError` when the fixer produced none.
fn shape_result(value: Option<Shape>) -> PyResult<PartShape> {
    value
        .map(PartShape::from_shape)
        .ok_or_else(|| OCCError::new_err("the shape fixer produced no shape"))
}

fn float_arg(args: &Bound<'_, PyTuple>, index: usize) -> PyResult<f64> {
    args.get_item(index)?.extract()
}

fn bool_arg(args: &Bound<'_, PyTuple>, index: usize) -> PyResult<bool> {
    args.get_item(index)?.extract()
}

fn type_error(expected: &str) -> PyErr {
    PyTypeError::new_err(format!("wrong arguments; expected {expected}"))
}

/// `Part.ShapeFix.Root` — precision and tolerance limits.
#[pyclass(name = "Root", module = "Part.ShapeFix", unsendable)]
pub struct PyRoot {
    fixer: RootFixer,
}

#[pymethods]
impl PyRoot {
    #[new]
    fn new() -> Self {
        Self {
            fixer: RootFixer::new(),
        }
    }

    #[getter]
    fn Precision(&self) -> f64 {
        self.fixer.precision()
    }
    #[setter]
    fn set_Precision(&self, value: f64) {
        self.fixer.set_precision(value)
    }

    #[getter]
    fn MinTolerance(&self) -> f64 {
        self.fixer.min_tolerance()
    }
    #[setter]
    fn set_MinTolerance(&self, value: f64) {
        self.fixer.set_min_tolerance(value)
    }

    #[getter]
    fn MaxTolerance(&self) -> f64 {
        self.fixer.max_tolerance()
    }
    #[setter]
    fn set_MaxTolerance(&self, value: f64) {
        self.fixer.set_max_tolerance(value)
    }

    fn limitTolerance(&self, tolerance: f64) -> f64 {
        self.fixer.limit_tolerance(tolerance)
    }

    fn __repr__(&self) -> String {
        format!(
            "<Part.ShapeFix.Root precision={} min={} max={}>",
            self.fixer.precision(),
            self.fixer.min_tolerance(),
            self.fixer.max_tolerance()
        )
    }
}

/// `Part.ShapeFix.Edge` — the edge-level fixes.
#[pyclass(name = "Edge", module = "Part.ShapeFix", unsendable)]
pub struct PyEdge {
    fixer: EdgeFixer,
}

impl PyEdge {
    fn from_fixer(fixer: Option<EdgeFixer>) -> PyResult<Self> {
        fixer
            .map(|fixer| Self { fixer })
            .ok_or_else(|| OCCError::new_err("the shape fixer tool could not be created"))
    }
}

#[pymethods]
impl PyEdge {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        if !args.is_empty() {
            return Err(type_error("Part.ShapeFix.Edge()"));
        }
        Self::from_fixer(EdgeFixer::new())
    }

    #[pyo3(signature = (*args))]
    fn fixRemovePCurve(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            2 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                let face = face_shape(&args.get_item(1)?)?;
                Ok(self.fixer.remove_pcurve_face(&edge, &face))
            }
            3 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                let (_, normal) = plane_of(&args.get_item(1)?)?;
                Ok(self.fixer.remove_pcurve_surface(&edge, normal))
            }
            _ => Err(type_error(
                "fixRemovePCurve(edge, face) or fixRemovePCurve(edge, surface, placement)",
            )),
        }
    }

    fn fixRemoveCurve3d(&self, edge: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.fixer.remove_curve3d(&edge_shape(edge)?))
    }

    fn fixAddCurve3d(&self, edge: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.fixer.add_curve3d(&edge_shape(edge)?))
    }

    #[pyo3(signature = (*args))]
    fn fixAddPCurve(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        // (edge, face, seam[, prec]) or (edge, surface, placement, seam[, prec])
        let edge = edge_shape(&args.get_item(0)?)?;
        if args.len() >= 3 {
            if args.get_item(1)?.downcast::<PartShape>().is_ok() {
                let face = face_shape(&args.get_item(1)?)?;
                let seam = bool_arg(args, 2)?;
                let prec = if args.len() > 3 {
                    float_arg(args, 3)?
                } else {
                    0.0
                };
                return Ok(self.fixer.add_pcurve_face(&edge, &face, seam, prec));
            }
            if args.len() >= 4 {
                let (_, normal) = plane_of(&args.get_item(1)?)?;
                let seam = bool_arg(args, 3)?;
                let prec = if args.len() > 4 {
                    float_arg(args, 4)?
                } else {
                    0.0
                };
                return Ok(self.fixer.add_pcurve_surface(&edge, normal, seam, prec));
            }
        }
        Err(type_error(
            "fixAddPCurve(edge, face, seam[, prec]) or \
             fixAddPCurve(edge, surface, placement, seam[, prec])",
        ))
    }

    #[pyo3(signature = (*args))]
    fn fixVertexTolerance(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            1 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                Ok(self.fixer.vertex_tolerance(&edge))
            }
            2 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                let face = face_shape(&args.get_item(1)?)?;
                Ok(self.fixer.vertex_tolerance_face(&edge, &face))
            }
            _ => Err(type_error("fixVertexTolerance(edge[, face])")),
        }
    }

    #[pyo3(signature = (*args))]
    fn fixReversed2d(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            2 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                let face = face_shape(&args.get_item(1)?)?;
                Ok(self.fixer.reversed2d_face(&edge, &face))
            }
            3 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                let (_, normal) = plane_of(&args.get_item(1)?)?;
                Ok(self.fixer.reversed2d_surface(&edge, normal))
            }
            _ => Err(type_error(
                "fixReversed2d(edge, face) or fixReversed2d(edge, surface, placement)",
            )),
        }
    }

    #[pyo3(signature = (*args))]
    fn fixSameParameter(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            1 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                Ok(self.fixer.same_parameter(&edge, 0.0))
            }
            2 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                if args.get_item(1)?.downcast::<PartShape>().is_ok() {
                    let face = face_shape(&args.get_item(1)?)?;
                    Ok(self.fixer.same_parameter_face(&edge, &face, 0.0))
                } else {
                    let tolerance = float_arg(args, 1)?;
                    Ok(self.fixer.same_parameter(&edge, tolerance))
                }
            }
            3 => {
                let edge = edge_shape(&args.get_item(0)?)?;
                let face = face_shape(&args.get_item(1)?)?;
                let tolerance = float_arg(args, 2)?;
                Ok(self.fixer.same_parameter_face(&edge, &face, tolerance))
            }
            _ => Err(type_error(
                "fixSameParameter(edge[, tolerance]) or fixSameParameter(edge, face[, tolerance])",
            )),
        }
    }

    fn __repr__(&self) -> String {
        "<Part.ShapeFix.Edge>".to_string()
    }
}

/// `Part.ShapeFix.Wire` — the wire-level fixes and their modes.
#[pyclass(name = "Wire", module = "Part.ShapeFix", unsendable)]
pub struct PyWire {
    fixer: WireFixer,
}

impl PyWire {
    fn from_fixer(fixer: Option<WireFixer>) -> PyResult<Self> {
        fixer
            .map(|fixer| Self { fixer })
            .ok_or_else(|| OCCError::new_err("the shape fixer tool could not be created"))
    }

    fn wrap(&self, value: Option<Shape>) -> Option<PartShape> {
        value.map(PartShape::from_shape)
    }
}

#[pymethods]
impl PyWire {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Self::from_fixer(WireFixer::new()),
            3 => {
                let wire = shape_of(&args.get_item(0)?, "Part.ShapeFix.Wire")?;
                let face = shape_of(&args.get_item(1)?, "Part.ShapeFix.Wire")?;
                let prec = float_arg(args, 2)?;
                Self::from_fixer(WireFixer::from_wire_face(&wire, &face, prec))
            }
            _ => Err(type_error("Part.ShapeFix.Wire() or Wire(wire, face, prec)")),
        }
    }

    #[pyo3(signature = (*args))]
    fn init(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        if args.len() != 3 {
            return Err(type_error("init(wire, face, prec)"));
        }
        let wire = shape_of(&args.get_item(0)?, "init")?;
        let face = shape_of(&args.get_item(1)?, "init")?;
        let prec = float_arg(args, 2)?;
        if !self.fixer.init(&wire, &face, prec) {
            return Err(OCCError::new_err("ShapeFix_Wire.init did not accept the shapes"));
        }
        Ok(())
    }

    fn load(&self, wire: &Bound<'_, PyAny>) -> PyResult<()> {
        let wire = shape_of(wire, "load")?;
        if !self.fixer.load(&wire) {
            return Err(OCCError::new_err("ShapeFix_Wire.load did not accept the wire"));
        }
        Ok(())
    }

    fn setFace(&self, face: &Bound<'_, PyAny>) -> PyResult<()> {
        let face = shape_of(face, "setFace")?;
        if !self.fixer.set_face(&face) {
            return Err(OCCError::new_err("ShapeFix_Wire.setFace did not accept the face"));
        }
        Ok(())
    }

    #[pyo3(signature = (*args))]
    fn setSurface(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        if args.is_empty() || args.len() > 2 {
            return Err(type_error("setSurface(surface[, placement])"));
        }
        let (origin, normal) = plane_of(&args.get_item(0)?)?;
        let placed = args.len() == 2;
        if !self.fixer.set_surface(origin, normal, placed) {
            return Err(OCCError::new_err(
                "ShapeFix_Wire.setSurface did not accept the surface",
            ));
        }
        Ok(())
    }

    fn setMaxTailAngle(&self, angle: f64) {
        self.fixer.set_max_tail_angle(angle)
    }

    fn setMaxTailWidth(&self, width: f64) {
        self.fixer.set_max_tail_width(width)
    }

    fn isLoaded(&self) -> bool {
        self.fixer.is_loaded()
    }

    fn isReady(&self) -> bool {
        self.fixer.is_ready()
    }

    fn numberOfEdges(&self) -> i32 {
        self.fixer.number_of_edges()
    }

    fn wire(&self) -> Option<PartShape> {
        self.wrap(self.fixer.wire())
    }

    fn wireAPIMake(&self) -> Option<PartShape> {
        self.wrap(self.fixer.wire_api_make())
    }

    fn face(&self) -> Option<PartShape> {
        self.wrap(self.fixer.face())
    }

    fn perform(&self) -> bool {
        self.fixer.perform()
    }

    fn clearModes(&self) {
        self.fixer.clear_modes()
    }

    fn clearStatuses(&self) {
        self.fixer.clear_statuses()
    }

    fn fixEdgeTool(&self) -> PyResult<PyEdge> {
        PyEdge::from_fixer(self.fixer.fix_edge_tool())
    }

    fn fixReorder(&self) -> bool {
        self.fixer.fix_reorder()
    }

    #[pyo3(signature = (*args))]
    fn fixSmall(&self, py: Python<'_>, args: &Bound<'_, PyTuple>) -> PyResult<PyObject> {
        use pyo3::IntoPyObjectExt;
        match args.len() {
            1 => {
                let lock = bool_arg(args, 0)?;
                self.fixer.fix_small_global(lock, 0.0).into_py_any(py)
            }
            2 => {
                let lock = bool_arg(args, 0)?;
                let prec = float_arg(args, 1)?;
                self.fixer.fix_small_global(lock, prec).into_py_any(py)
            }
            3 => {
                let num: i32 = args.get_item(0)?.extract()?;
                let lock = bool_arg(args, 1)?;
                let prec = float_arg(args, 2)?;
                self.fixer.fix_small_one(num, lock, prec).into_py_any(py)
            }
            _ => Err(type_error("fixSmall(lock[, prec]) or fixSmall(num, lock, prec)")),
        }
    }

    #[pyo3(signature = (*args))]
    fn fixConnected(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            0 => Ok(self.fixer.fix_connected(-1.0)),
            1 => Ok(self.fixer.fix_connected(float_arg(args, 0)?)),
            2 => {
                let num: i32 = args.get_item(0)?.extract()?;
                let prec = float_arg(args, 1)?;
                Ok(self.fixer.fix_connected_one(num, prec))
            }
            _ => Err(type_error("fixConnected([prec]) or fixConnected(num, prec)")),
        }
    }

    fn fixEdgeCurves(&self) -> bool {
        self.fixer.fix_edge_curves()
    }

    #[pyo3(signature = (*args))]
    fn fixDegenerated(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            0 => Ok(self.fixer.fix_degenerated()),
            1 => {
                let num: i32 = args.get_item(0)?.extract()?;
                Ok(self.fixer.fix_degenerated_one(num))
            }
            _ => Err(type_error("fixDegenerated([num])")),
        }
    }

    fn fixSelfIntersection(&self) -> bool {
        self.fixer.fix_self_intersection()
    }

    #[pyo3(signature = (*args))]
    fn fixLacking(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            0 => Ok(self.fixer.fix_lacking(false)),
            1 => Ok(self.fixer.fix_lacking(bool_arg(args, 0)?)),
            2 => {
                let num: i32 = args.get_item(0)?.extract()?;
                let force = bool_arg(args, 1)?;
                Ok(self.fixer.fix_lacking_one(num, force))
            }
            _ => Err(type_error("fixLacking([force]) or fixLacking(num[, force])")),
        }
    }

    #[pyo3(signature = (*args))]
    fn fixClosed(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        match args.len() {
            0 => Ok(self.fixer.fix_closed(-1.0)),
            1 => Ok(self.fixer.fix_closed(float_arg(args, 0)?)),
            _ => Err(type_error("fixClosed([prec])")),
        }
    }

    fn fixGaps3d(&self) -> bool {
        self.fixer.fix_gaps3d()
    }

    fn fixGaps2d(&self) -> bool {
        self.fixer.fix_gaps2d()
    }

    fn fixSeam(&self, num: i32) -> bool {
        self.fixer.fix_seam(num)
    }

    fn fixShifted(&self) -> bool {
        self.fixer.fix_shifted()
    }

    fn fixNotchedEdges(&self) -> bool {
        self.fixer.fix_notched_edges()
    }

    #[pyo3(signature = (num, convert = false))]
    fn fixGap3d(&self, num: i32, convert: bool) -> bool {
        self.fixer.fix_gap3d(num, convert)
    }

    #[pyo3(signature = (num, convert = false))]
    fn fixGap2d(&self, num: i32, convert: bool) -> bool {
        self.fixer.fix_gap2d(num, convert)
    }

    fn fixTails(&self) -> bool {
        self.fixer.fix_tails()
    }

        #[getter]
    fn ModifyTopologyMode(&self) -> bool {
        self.fixer.modify_topology_mode()
    }
    #[setter]
    fn set_ModifyTopologyMode(&self, value: bool) {
        self.fixer.set_modify_topology_mode(value)
    }
        #[getter]
    fn ModifyGeometryMode(&self) -> bool {
        self.fixer.modify_geometry_mode()
    }
    #[setter]
    fn set_ModifyGeometryMode(&self, value: bool) {
        self.fixer.set_modify_geometry_mode(value)
    }
        #[getter]
    fn ModifyRemoveLoopMode(&self) -> bool {
        self.fixer.modify_remove_loop_mode() != 0
    }
    #[setter]
    fn set_ModifyRemoveLoopMode(&self, value: bool) {
        self.fixer.set_modify_remove_loop_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn ClosedWireMode(&self) -> bool {
        self.fixer.closed_wire_mode()
    }
    #[setter]
    fn set_ClosedWireMode(&self, value: bool) {
        self.fixer.set_closed_wire_mode(value)
    }
        #[getter]
    fn PreferencePCurveMode(&self) -> bool {
        self.fixer.preference_pcurve_mode()
    }
    #[setter]
    fn set_PreferencePCurveMode(&self, value: bool) {
        self.fixer.set_preference_pcurve_mode(value)
    }
        #[getter]
    fn FixGapsByRangesMode(&self) -> bool {
        self.fixer.fix_gaps_by_ranges_mode()
    }
    #[setter]
    fn set_FixGapsByRangesMode(&self, value: bool) {
        self.fixer.set_fix_gaps_by_ranges_mode(value)
    }
        #[getter]
    fn FixReorderMode(&self) -> bool {
        self.fixer.fix_reorder_mode() != 0
    }
    #[setter]
    fn set_FixReorderMode(&self, value: bool) {
        self.fixer.set_fix_reorder_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSmallMode(&self) -> bool {
        self.fixer.fix_small_mode() != 0
    }
    #[setter]
    fn set_FixSmallMode(&self, value: bool) {
        self.fixer.set_fix_small_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixConnectedMode(&self) -> bool {
        self.fixer.fix_connected_mode() != 0
    }
    #[setter]
    fn set_FixConnectedMode(&self, value: bool) {
        self.fixer.set_fix_connected_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixEdgeCurvesMode(&self) -> bool {
        self.fixer.fix_edge_curves_mode() != 0
    }
    #[setter]
    fn set_FixEdgeCurvesMode(&self, value: bool) {
        self.fixer.set_fix_edge_curves_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixDegeneratedMode(&self) -> bool {
        self.fixer.fix_degenerated_mode() != 0
    }
    #[setter]
    fn set_FixDegeneratedMode(&self, value: bool) {
        self.fixer.set_fix_degenerated_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSelfIntersectionMode(&self) -> bool {
        self.fixer.fix_self_intersection_mode() != 0
    }
    #[setter]
    fn set_FixSelfIntersectionMode(&self, value: bool) {
        self.fixer.set_fix_self_intersection_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixLackingMode(&self) -> bool {
        self.fixer.fix_lacking_mode() != 0
    }
    #[setter]
    fn set_FixLackingMode(&self, value: bool) {
        self.fixer.set_fix_lacking_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixGaps3dMode(&self) -> bool {
        self.fixer.fix_gaps3d_mode() != 0
    }
    #[setter]
    fn set_FixGaps3dMode(&self, value: bool) {
        self.fixer.set_fix_gaps3d_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixGaps2dMode(&self) -> bool {
        self.fixer.fix_gaps2d_mode() != 0
    }
    #[setter]
    fn set_FixGaps2dMode(&self, value: bool) {
        self.fixer.set_fix_gaps2d_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixReversed2dMode(&self) -> bool {
        self.fixer.fix_reversed2d_mode() != 0
    }
    #[setter]
    fn set_FixReversed2dMode(&self, value: bool) {
        self.fixer.set_fix_reversed2d_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixRemovePCurveMode(&self) -> bool {
        self.fixer.fix_remove_pcurve_mode() != 0
    }
    #[setter]
    fn set_FixRemovePCurveMode(&self, value: bool) {
        self.fixer.set_fix_remove_pcurve_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixAddPCurveMode(&self) -> bool {
        self.fixer.fix_add_pcurve_mode() != 0
    }
    #[setter]
    fn set_FixAddPCurveMode(&self, value: bool) {
        self.fixer.set_fix_add_pcurve_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixRemoveCurve3dMode(&self) -> bool {
        self.fixer.fix_remove_curve3d_mode() != 0
    }
    #[setter]
    fn set_FixRemoveCurve3dMode(&self, value: bool) {
        self.fixer.set_fix_remove_curve3d_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixAddCurve3dMode(&self) -> bool {
        self.fixer.fix_add_curve3d_mode() != 0
    }
    #[setter]
    fn set_FixAddCurve3dMode(&self, value: bool) {
        self.fixer.set_fix_add_curve3d_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSeamMode(&self) -> bool {
        self.fixer.fix_seam_mode() != 0
    }
    #[setter]
    fn set_FixSeamMode(&self, value: bool) {
        self.fixer.set_fix_seam_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixShiftedMode(&self) -> bool {
        self.fixer.fix_shifted_mode() != 0
    }
    #[setter]
    fn set_FixShiftedMode(&self, value: bool) {
        self.fixer.set_fix_shifted_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSameParameterMode(&self) -> bool {
        self.fixer.fix_same_parameter_mode() != 0
    }
    #[setter]
    fn set_FixSameParameterMode(&self, value: bool) {
        self.fixer.set_fix_same_parameter_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixVertexToleranceMode(&self) -> bool {
        self.fixer.fix_vertex_tolerance_mode() != 0
    }
    #[setter]
    fn set_FixVertexToleranceMode(&self, value: bool) {
        self.fixer.set_fix_vertex_tolerance_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixNotchedEdgesMode(&self) -> bool {
        self.fixer.fix_notched_edges_mode() != 0
    }
    #[setter]
    fn set_FixNotchedEdgesMode(&self, value: bool) {
        self.fixer.set_fix_notched_edges_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSelfIntersectingEdgeMode(&self) -> bool {
        self.fixer.fix_self_intersecting_edge_mode() != 0
    }
    #[setter]
    fn set_FixSelfIntersectingEdgeMode(&self, value: bool) {
        self.fixer.set_fix_self_intersecting_edge_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixIntersectingEdgesMode(&self) -> bool {
        self.fixer.fix_intersecting_edges_mode() != 0
    }
    #[setter]
    fn set_FixIntersectingEdgesMode(&self, value: bool) {
        self.fixer.set_fix_intersecting_edges_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixNonAdjacentIntersectingEdgesMode(&self) -> bool {
        self.fixer.fix_non_adjacent_intersecting_edges_mode() != 0
    }
    #[setter]
    fn set_FixNonAdjacentIntersectingEdgesMode(&self, value: bool) {
        self.fixer.set_fix_non_adjacent_intersecting_edges_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixTailMode(&self) -> bool {
        self.fixer.fix_tail_mode() != 0
    }
    #[setter]
    fn set_FixTailMode(&self, value: bool) {
        self.fixer.set_fix_tail_mode(if value { 1 } else { 0 })
    }

    fn __repr__(&self) -> String {
        format!(
            "<Part.ShapeFix.Wire loaded={} ready={} edges={}>",
            self.fixer.is_loaded(),
            self.fixer.is_ready(),
            self.fixer.number_of_edges()
        )
    }
}

/// `Part.ShapeFix.Face` — the face-level fixes.
#[pyclass(name = "Face", module = "Part.ShapeFix", unsendable)]
pub struct PyFace {
    fixer: FaceFixer,
}

impl PyFace {
    fn from_fixer(fixer: Option<FaceFixer>) -> PyResult<Self> {
        fixer
            .map(|fixer| Self { fixer })
            .ok_or_else(|| OCCError::new_err("the shape fixer tool could not be created"))
    }
}

#[pymethods]
impl PyFace {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Self::from_fixer(FaceFixer::new()),
            1 => {
                let shape = shape_of(&args.get_item(0)?, "Part.ShapeFix.Face")?;
                Self::from_fixer(FaceFixer::from_face(&shape))
            }
            2 | 3 => {
                let (origin, normal) = plane_of(&args.get_item(0)?)?;
                let prec = float_arg(args, 1)?;
                let forward = if args.len() == 3 {
                    bool_arg(args, 2)?
                } else {
                    true
                };
                Self::from_fixer(FaceFixer::from_plane(origin, normal, prec, forward))
            }
            _ => Err(type_error(
                "Part.ShapeFix.Face(), Face(face) or Face(surface, precision[, forward])",
            )),
        }
    }

    #[pyo3(signature = (*args))]
    fn init(&self, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        match args.len() {
            1 => {
                let face = shape_of(&args.get_item(0)?, "init")?;
                if !self.fixer.init_face(&face) {
                    return Err(OCCError::new_err("ShapeFix_Face.init did not accept the face"));
                }
                Ok(())
            }
            2 | 3 => {
                let (origin, normal) = plane_of(&args.get_item(0)?)?;
                let prec = float_arg(args, 1)?;
                let forward = if args.len() == 3 {
                    bool_arg(args, 2)?
                } else {
                    true
                };
                if !self.fixer.init_plane(origin, normal, prec, forward) {
                    return Err(OCCError::new_err(
                        "ShapeFix_Face.init did not accept the surface",
                    ));
                }
                Ok(())
            }
            _ => Err(type_error("init(face) or init(surface, precision[, forward])")),
        }
    }

    fn fixWireTool(&self) -> PyResult<PyWire> {
        PyWire::from_fixer(self.fixer.fix_wire_tool())
    }

    fn clearModes(&self) {
        self.fixer.clear_modes()
    }

    fn add(&self, wire: &Bound<'_, PyAny>) -> PyResult<()> {
        let wire = shape_of(wire, "add")?;
        if !self.fixer.add(&wire) {
            return Err(OCCError::new_err("ShapeFix_Face.add did not accept the wire"));
        }
        Ok(())
    }

    fn fixOrientation(&self) -> bool {
        self.fixer.fix_orientation()
    }

    fn fixAddNaturalBound(&self) -> bool {
        self.fixer.fix_add_natural_bound()
    }

    fn fixMissingSeam(&self) -> bool {
        self.fixer.fix_missing_seam()
    }

    fn fixSmallAreaWire(&self, remove_small: bool) -> bool {
        self.fixer.fix_small_area_wire(remove_small)
    }

    fn fixLoopWire(&self, py: Python<'_>) -> PyResult<(bool, Vec<Py<PartShape>>)> {
        let (ok, wires) = self.fixer.fix_loop_wire();
        let wires = wires
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect::<PyResult<Vec<_>>>()?;
        Ok((ok, wires))
    }

    fn fixIntersectingWires(&self) -> bool {
        self.fixer.fix_intersecting_wires()
    }

    fn fixWiresTwoCoincidentEdges(&self) -> bool {
        self.fixer.fix_wires_two_coincident_edges()
    }

    fn fixPeriodicDegenerated(&self) -> bool {
        self.fixer.fix_periodic_degenerated()
    }

    fn perform(&self) -> bool {
        self.fixer.perform()
    }

    fn face(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.face())
    }

    fn result(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.result())
    }

        #[getter]
    fn FixWireMode(&self) -> bool {
        self.fixer.fix_wire_mode() != 0
    }
    #[setter]
    fn set_FixWireMode(&self, value: bool) {
        self.fixer.set_fix_wire_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixOrientationMode(&self) -> bool {
        self.fixer.fix_orientation_mode() != 0
    }
    #[setter]
    fn set_FixOrientationMode(&self, value: bool) {
        self.fixer.set_fix_orientation_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixAddNaturalBoundMode(&self) -> bool {
        self.fixer.fix_add_natural_bound_mode() != 0
    }
    #[setter]
    fn set_FixAddNaturalBoundMode(&self, value: bool) {
        self.fixer.set_fix_add_natural_bound_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixMissingSeamMode(&self) -> bool {
        self.fixer.fix_missing_seam_mode() != 0
    }
    #[setter]
    fn set_FixMissingSeamMode(&self, value: bool) {
        self.fixer.set_fix_missing_seam_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSmallAreaWireMode(&self) -> bool {
        self.fixer.fix_small_area_wire_mode() != 0
    }
    #[setter]
    fn set_FixSmallAreaWireMode(&self, value: bool) {
        self.fixer.set_fix_small_area_wire_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn RemoveSmallAreaFaceMode(&self) -> bool {
        self.fixer.remove_small_area_face_mode() != 0
    }
    #[setter]
    fn set_RemoveSmallAreaFaceMode(&self, value: bool) {
        self.fixer.set_remove_small_area_face_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixIntersectingWiresMode(&self) -> bool {
        self.fixer.fix_intersecting_wires_mode() != 0
    }
    #[setter]
    fn set_FixIntersectingWiresMode(&self, value: bool) {
        self.fixer.set_fix_intersecting_wires_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixLoopWiresMode(&self) -> bool {
        self.fixer.fix_loop_wires_mode() != 0
    }
    #[setter]
    fn set_FixLoopWiresMode(&self, value: bool) {
        self.fixer.set_fix_loop_wires_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSplitFaceMode(&self) -> bool {
        self.fixer.fix_split_face_mode() != 0
    }
    #[setter]
    fn set_FixSplitFaceMode(&self, value: bool) {
        self.fixer.set_fix_split_face_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn AutoCorrectPrecisionMode(&self) -> bool {
        self.fixer.auto_correct_precision_mode() != 0
    }
    #[setter]
    fn set_AutoCorrectPrecisionMode(&self, value: bool) {
        self.fixer.set_auto_correct_precision_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixPeriodicDegeneratedMode(&self) -> bool {
        self.fixer.fix_periodic_degenerated_mode() != 0
    }
    #[setter]
    fn set_FixPeriodicDegeneratedMode(&self, value: bool) {
        self.fixer.set_fix_periodic_degenerated_mode(if value { 1 } else { 0 })
    }

    fn __repr__(&self) -> String {
        "<Part.ShapeFix.Face>".to_string()
    }
}

/// `Part.ShapeFix.Shell` — the shell-level fixes.
#[pyclass(name = "Shell", module = "Part.ShapeFix", unsendable)]
pub struct PyShell {
    fixer: ShellFixer,
}

impl PyShell {
    fn from_fixer(fixer: Option<ShellFixer>) -> PyResult<Self> {
        fixer
            .map(|fixer| Self { fixer })
            .ok_or_else(|| OCCError::new_err("the shape fixer tool could not be created"))
    }
}

#[pymethods]
impl PyShell {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Self::from_fixer(ShellFixer::new()),
            1 => {
                let shape = shape_of(&args.get_item(0)?, "Part.ShapeFix.Shell")?;
                Self::from_fixer(ShellFixer::from_shell(&shape))
            }
            _ => Err(type_error("Part.ShapeFix.Shell() or Shell(shell)")),
        }
    }

    fn init(&self, shell: &Bound<'_, PyAny>) -> PyResult<()> {
        let shell = shape_of(shell, "init")?;
        if !self.fixer.init(&shell) {
            return Err(OCCError::new_err("ShapeFix_Shell.init did not accept the shell"));
        }
        Ok(())
    }

    fn perform(&self) -> bool {
        self.fixer.perform()
    }

    fn shell(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.shell())
    }

    fn shape(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.shape())
    }

    fn fixFaceTool(&self) -> PyResult<PyFace> {
        PyFace::from_fixer(self.fixer.fix_face_tool())
    }

    #[pyo3(signature = (*args))]
    fn fixFaceOrientation(&self, args: &Bound<'_, PyTuple>) -> PyResult<bool> {
        if args.is_empty() || args.len() > 3 {
            return Err(type_error(
                "fixFaceOrientation(shell[, multi_connex[, non_manifold]])",
            ));
        }
        let shell = shape_of(&args.get_item(0)?, "fixFaceOrientation")?;
        let multi_connex = if args.len() > 1 {
            bool_arg(args, 1)?
        } else {
            true
        };
        let non_manifold = if args.len() > 2 {
            bool_arg(args, 2)?
        } else {
            false
        };
        Ok(self
            .fixer
            .fix_face_orientation(&shell, multi_connex, non_manifold))
    }

    fn errorFaces(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.error_faces())
    }

    fn numberOfShells(&self) -> i32 {
        self.fixer.number_of_shells()
    }

    fn setNonManifoldFlag(&self, non_manifold: bool) {
        self.fixer.set_non_manifold_flag(non_manifold)
    }

        #[getter]
    fn FixFaceMode(&self) -> bool {
        self.fixer.fix_face_mode() != 0
    }
    #[setter]
    fn set_FixFaceMode(&self, value: bool) {
        self.fixer.set_fix_face_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixOrientationMode(&self) -> bool {
        self.fixer.fix_orientation_mode() != 0
    }
    #[setter]
    fn set_FixOrientationMode(&self, value: bool) {
        self.fixer.set_fix_orientation_mode(if value { 1 } else { 0 })
    }

    fn __repr__(&self) -> String {
        format!("<Part.ShapeFix.Shell shells={}>", self.fixer.number_of_shells())
    }
}

/// `Part.ShapeFix.Solid` — the solid-level fixes.
#[pyclass(name = "Solid", module = "Part.ShapeFix", unsendable)]
pub struct PySolid {
    fixer: SolidFixer,
}

impl PySolid {
    fn from_fixer(fixer: Option<SolidFixer>) -> PyResult<Self> {
        fixer
            .map(|fixer| Self { fixer })
            .ok_or_else(|| OCCError::new_err("the shape fixer tool could not be created"))
    }
}

#[pymethods]
impl PySolid {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Self::from_fixer(SolidFixer::new()),
            1 => {
                let shape = shape_of(&args.get_item(0)?, "Part.ShapeFix.Solid")?;
                Self::from_fixer(SolidFixer::from_solid(&shape))
            }
            _ => Err(type_error("Part.ShapeFix.Solid() or Solid(solid)")),
        }
    }

    fn init(&self, solid: &Bound<'_, PyAny>) -> PyResult<()> {
        let solid = shape_of(solid, "init")?;
        if !self.fixer.init(&solid) {
            return Err(OCCError::new_err("ShapeFix_Solid.init did not accept the solid"));
        }
        Ok(())
    }

    fn perform(&self) -> bool {
        self.fixer.perform()
    }

    fn solidFromShell(&self, shell: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shell = shape_of(shell, "solidFromShell")?;
        shape_result(self.fixer.solid_from_shell(&shell))
    }

    fn solid(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.solid())
    }

    fn shape(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.shape())
    }

    fn fixShellTool(&self) -> PyResult<PyShell> {
        PyShell::from_fixer(self.fixer.fix_shell_tool())
    }

        #[getter]
    fn FixShellMode(&self) -> bool {
        self.fixer.fix_shell_mode() != 0
    }
    #[setter]
    fn set_FixShellMode(&self, value: bool) {
        self.fixer.set_fix_shell_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixShellOrientationMode(&self) -> bool {
        self.fixer.fix_shell_orientation_mode() != 0
    }
    #[setter]
    fn set_FixShellOrientationMode(&self, value: bool) {
        self.fixer.set_fix_shell_orientation_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn CreateOpenSolidMode(&self) -> bool {
        self.fixer.create_open_solid_mode()
    }
    #[setter]
    fn set_CreateOpenSolidMode(&self, value: bool) {
        self.fixer.set_create_open_solid_mode(value)
    }

    fn __repr__(&self) -> String {
        "<Part.ShapeFix.Solid>".to_string()
    }
}

/// `Part.ShapeFix.Shape` — the whole-shape fixer.
#[pyclass(name = "Shape", module = "Part.ShapeFix", unsendable)]
pub struct PyShapeFix {
    fixer: ShapeFixer,
}

impl PyShapeFix {
    fn from_fixer(fixer: Option<ShapeFixer>) -> PyResult<Self> {
        fixer
            .map(|fixer| Self { fixer })
            .ok_or_else(|| OCCError::new_err("the shape fixer tool could not be created"))
    }
}

#[pymethods]
impl PyShapeFix {
    #[new]
    #[pyo3(signature = (*args))]
    fn new(args: &Bound<'_, PyTuple>) -> PyResult<Self> {
        match args.len() {
            0 => Self::from_fixer(ShapeFixer::new()),
            1 => {
                let shape = shape_of(&args.get_item(0)?, "Part.ShapeFix.Shape")?;
                Self::from_fixer(ShapeFixer::from_shape(&shape))
            }
            _ => Err(type_error("Part.ShapeFix.Shape() or Shape(shape)")),
        }
    }

    fn init(&self, shape: &Bound<'_, PyAny>) -> PyResult<()> {
        let shape = shape_of(shape, "init")?;
        if !self.fixer.init(&shape) {
            return Err(OCCError::new_err("ShapeFix_Shape.init did not accept the shape"));
        }
        Ok(())
    }

    fn perform(&self) -> bool {
        self.fixer.perform()
    }

    fn shape(&self) -> PyResult<PartShape> {
        shape_result(self.fixer.shape())
    }

    fn fixSolidTool(&self) -> PyResult<PySolid> {
        PySolid::from_fixer(self.fixer.fix_solid_tool())
    }

    fn fixShellTool(&self) -> PyResult<PyShell> {
        PyShell::from_fixer(self.fixer.fix_shell_tool())
    }

    fn fixFaceTool(&self) -> PyResult<PyFace> {
        PyFace::from_fixer(self.fixer.fix_face_tool())
    }

    fn fixWireTool(&self) -> PyResult<PyWire> {
        PyWire::from_fixer(self.fixer.fix_wire_tool())
    }

    fn fixEdgeTool(&self) -> PyResult<PyEdge> {
        PyEdge::from_fixer(self.fixer.fix_edge_tool())
    }

        #[getter]
    fn FixSolidMode(&self) -> bool {
        self.fixer.fix_solid_mode() != 0
    }
    #[setter]
    fn set_FixSolidMode(&self, value: bool) {
        self.fixer.set_fix_solid_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixFreeShellMode(&self) -> bool {
        self.fixer.fix_free_shell_mode() != 0
    }
    #[setter]
    fn set_FixFreeShellMode(&self, value: bool) {
        self.fixer.set_fix_free_shell_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixFreeFaceMode(&self) -> bool {
        self.fixer.fix_free_face_mode() != 0
    }
    #[setter]
    fn set_FixFreeFaceMode(&self, value: bool) {
        self.fixer.set_fix_free_face_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixFreeWireMode(&self) -> bool {
        self.fixer.fix_free_wire_mode() != 0
    }
    #[setter]
    fn set_FixFreeWireMode(&self, value: bool) {
        self.fixer.set_fix_free_wire_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixSameParameterMode(&self) -> bool {
        self.fixer.fix_same_parameter_mode() != 0
    }
    #[setter]
    fn set_FixSameParameterMode(&self, value: bool) {
        self.fixer.set_fix_same_parameter_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixVertexPositionMode(&self) -> bool {
        self.fixer.fix_vertex_position_mode() != 0
    }
    #[setter]
    fn set_FixVertexPositionMode(&self, value: bool) {
        self.fixer.set_fix_vertex_position_mode(if value { 1 } else { 0 })
    }
        #[getter]
    fn FixVertexTolMode(&self) -> bool {
        self.fixer.fix_vertex_tol_mode() != 0
    }
    #[setter]
    fn set_FixVertexTolMode(&self, value: bool) {
        self.fixer.set_fix_vertex_tol_mode(if value { 1 } else { 0 })
    }

    fn __repr__(&self) -> String {
        "<Part.ShapeFix.Shape>".to_string()
    }
}

/// Register `Part.ShapeFix` as a submodule of `Part`.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let module = PyModule::new(m.py(), "Part.ShapeFix")?;
    module.add_class::<PyRoot>()?;
    module.add_class::<PyShapeFix>()?;
    module.add_class::<PySolid>()?;
    module.add_class::<PyShell>()?;
    module.add_class::<PyFace>()?;
    module.add_class::<PyWire>()?;
    module.add_class::<PyEdge>()?;
    m.add("ShapeFix", &module)?;
    Ok(())
}
