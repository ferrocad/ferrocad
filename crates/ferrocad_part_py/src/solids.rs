//! `Part.Shell`, `Part.Solid` and the shell/solid factories in Python.
//!
//! A shell keeps the faces it was built from and a solid the shell it was built
//! from, so a later `add` can rebuild the B-Rep — the same pattern as
//! `Part.Compound` in `containers.rs`. The functional forms (`Part.makeShell`,
//! `Part.makeSolid`, `Part.makeShellFromWires`, `Part.makeLoft`,
//! `Part.makeRuledSurface`, `Part.makeFilledFace`) return a plain `Part.Shape`,
//! as upstream's `shape2pyshape` does.
//!
//! Deliberate limits for this slice, reported rather than silently ignored:
//! `makeLoft` accepts upstream's `ruled`/`closed`/`max_degree` keywords but only
//! builds the default (smoothed, open, degree-5) loft; `makeFilledFace` builds
//! the default filled face and ignores the fitting parameters upstream exposes.

use std::f64::consts::PI;
use std::ffi::CString;
use std::sync::{Arc, Mutex};

use ferrocad_core::object_registry::{self, BodyMembership, ObjectType, PropertyChangeError};
use ferrocad_core::{Document, Matrix4, ObjectId, Placement, Property, Rotation, prop_status};
use ferrocad_curves::{Arc as CurveArc, Circle, EdgeCurve, LineSegment, Vector3};
use ferrocad_geom::{ElementKind, Lineage, Shape, SurfaceSpec};
use ferrocad_occt::surface_ops;
use ferrocad_part::{
    make_shape_list_property, make_shape_property, shape_list_of, shape_of, ShapeListProperty,
    ShapeProperty,
};
use pyo3::exceptions::{
    PyAttributeError, PyDeprecationWarning, PyIndexError, PyRuntimeError, PyTypeError,
    PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule, PyTuple};

use crate::containers::PyCompound;
use crate::curves::{self, OCCError};
use crate::{shape_argument, PartShape};

/// Map a kernel failure to `Part.OCCError`.
fn kernel_error(error: ferrocad_geom::GeomError) -> PyErr {
    OCCError::new_err(error.to_string())
}

/// The shapes named by a `Part.Shape`, a `Part.Compound`, or an iterable of
/// either (upstream's `getPyShapes`).
fn shape_list(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if let Ok(compound) = value.downcast::<PyCompound>() {
        let compound = compound.borrow();
        return Ok(compound.children(compound.as_super()));
    }
    if value.downcast::<PartShape>().is_ok() {
        return Ok(vec![shape_argument(value)?]);
    }
    if value.downcast::<pyo3::types::PyString>().is_ok() {
        return Err(PyTypeError::new_err(
            "expected a shape or a sequence of shapes",
        ));
    }
    let mut shapes = Vec::new();
    for item in value.try_iter()? {
        shapes.extend(shape_list(&item?)?);
    }
    Ok(shapes)
}

/// Wrap a factory result with the element map its shape carries on its own.
///
/// `makeShell`, `makeSolid`, `makeLoft` and `makeFilledFace` return shapes whose
/// result elements upstream's `makeElement*` namers name positionally, and the
/// kernel reports exactly those (26/26/24/9). Attaching the map here — rather
/// than in the shared `store` helper — keeps the guards for shapes built
/// elsewhere off.
fn with_intrinsic_lineage(shape: Shape) -> PartShape {
    let lineage = ferrocad_part::backend()
        .intrinsic_lineage(&shape)
        .unwrap_or_default();
    PartShape::from_shape_with_lineage(shape, lineage)
}

/// The shells of a shape: itself when it is a shell, otherwise its shells.
fn shells_of(shape: &Shape) -> Vec<Shape> {
    ferrocad_part::backend()
        .elements(shape, ElementKind::Shell)
        .into_iter()
        .map(|element| element.shape)
        .collect()
}

/// `Part.Shell(faces=None)` — a shell, a `Part.Shape` subclass.
#[pyclass(name = "Shell", module = "Part", extends = PartShape)]
pub struct PyShell {
    faces: Mutex<Vec<Shape>>,
}

impl PyShell {
    /// Wrap an existing shell shape (a `Shape.Shells` result) as a `Part.Shell`,
    /// keeping its B-Rep and lineage rather than rebuilding the shell.
    pub(crate) fn from_shape(shape: Shape) -> PyClassInitializer<Self> {
        let faces = ferrocad_part::backend()
            .elements(&shape, ElementKind::Face)
            .into_iter()
            .map(|element| element.shape)
            .collect();
        PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self {
            faces: Mutex::new(faces),
        })
    }

    fn initializer(faces: Vec<Shape>) -> PyResult<PyClassInitializer<Self>> {
        let base = if faces.is_empty() {
            PartShape::empty()
        } else {
            let shape = ferrocad_part::backend()
                .make_shell(&faces)
                .map_err(kernel_error)?;
            PartShape::from_shape(shape)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            faces: Mutex::new(faces),
        }))
    }
}

#[pymethods]
impl PyShell {
    /// `Part.Shell()` is the null shell; `Part.Shell(faces)` builds a shell from
    /// those faces (upstream's constructor with a list of shapes).
    #[new]
    #[pyo3(signature = (shapes = None))]
    fn new(shapes: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        let faces = match shapes {
            Some(shapes) => shape_list(shapes)?,
            None => Vec::new(),
        };
        Self::initializer(faces)
    }

    /// Append a face and rebuild the shell.
    fn add(mut slf: PyRefMut<'_, Self>, face: &Bound<'_, PyAny>) -> PyResult<()> {
        let added = shape_list(face)?;
        let rebuilt = {
            let mut faces = slf.faces.lock().unwrap_or_else(|e| e.into_inner());
            faces.extend(added);
            ferrocad_part::backend()
                .make_shell(&faces)
                .map_err(kernel_error)?
        };
        slf.as_super().replace_shape(Some(rebuilt));
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.faces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// The shell's free edges (`Shell.getFreeEdges`): the boundary edges shared
    /// by a single face, as a compound (`ShapeAnalysis_Shell`).
    ///
    /// The compound keeps the shell's placement. This is how an opening is found
    /// (`Part.Wire(Part.sortEdges(shell.getFreeEdges().Edges))`).
    fn getFreeEdges(slf: PyRef<'_, Self>) -> PyResult<PartShape> {
        let shape = slf
            .as_super()
            .shape()
            .ok_or_else(|| OCCError::new_err("an empty shell has no edges"))?;
        let free = ferrocad_part::backend()
            .shell_free_edges(&shape)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(free))
    }

    fn __repr__(&self) -> String {
        format!("Part.Shell({} faces)", self.__len__())
    }
}

/// `Part.Solid(shell=None)` — a solid, a `Part.Shape` subclass.
#[pyclass(name = "Solid", module = "Part", extends = PartShape)]
pub struct PySolid {
    shells: Mutex<Vec<Shape>>,
}

impl PySolid {
    /// Wrap an existing solid shape (a `Shape.Solids` result) as a `Part.Solid`,
    /// keeping its B-Rep and lineage rather than rebuilding the solid.
    pub(crate) fn from_shape(shape: Shape) -> PyClassInitializer<Self> {
        let shells = shells_of(&shape);
        PyClassInitializer::from(PartShape::from_shape(shape)).add_subclass(Self {
            shells: Mutex::new(shells),
        })
    }

    fn initializer(shells: Vec<Shape>) -> PyResult<PyClassInitializer<Self>> {
        let base = if shells.is_empty() {
            PartShape::empty()
        } else {
            let shape = solid_from(&shells)?;
            PartShape::from_shape(shape)
        };
        Ok(PyClassInitializer::from(base).add_subclass(Self {
            shells: Mutex::new(shells),
        }))
    }
}

/// Build a solid from the collected shells.
///
/// The first shell is the outer skin and any further shells are cavities
/// (`BRepBuilderAPI_MakeSolid` + `OrientClosedSolid` in the kernel).
fn solid_from(shells: &[Shape]) -> PyResult<Shape> {
    match shells {
        [shell] => ferrocad_part::backend()
            .make_solid(shell)
            .map_err(kernel_error),
        [] => Err(OCCError::new_err("a solid needs a shell")),
        _ => surface_ops::make_hollow_solid(shells).map_err(kernel_error),
    }
}

#[pymethods]
impl PySolid {
    /// `Part.Solid()` is the null solid; `Part.Solid(shape)` builds a solid from
    /// the shells of `shape` (upstream's constructor).
    #[new]
    #[pyo3(signature = (shape = None))]
    fn new(shape: Option<&Bound<'_, PyAny>>) -> PyResult<PyClassInitializer<Self>> {
        let shells = match shape {
            Some(shape) => shells_of(&shape_argument(shape)?),
            None => Vec::new(),
        };
        Self::initializer(shells)
    }

    /// Append a shell (or the shells of a solid) and rebuild the solid.
    fn add(mut slf: PyRefMut<'_, Self>, shape: &Bound<'_, PyAny>) -> PyResult<()> {
        let added = shells_of(&shape_argument(shape)?);
        if added.is_empty() {
            return Err(OCCError::new_err("Part.Solid.add expects a shell"));
        }
        // Build from the extended list before committing it, so a rejected add
        // leaves the object as it was.
        let rebuilt = {
            let mut shells = slf.shells.lock().unwrap_or_else(|e| e.into_inner());
            let mut candidate = shells.clone();
            candidate.extend(added);
            let shape = solid_from(&candidate)?;
            *shells = candidate;
            shape
        };
        slf.as_super().replace_shape(Some(rebuilt));
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.shells
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn __repr__(&self) -> String {
        format!("Part.Solid({} shells)", self.__len__())
    }
}

/// `Part.makeShell(faces, op=None)` — create a shell out of a list of faces.
#[pyfunction]
#[pyo3(signature = (shapes, op = None))]
fn makeShell(shapes: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shape = ferrocad_part::backend()
        .make_shell(&shape_list(shapes)?)
        .map_err(kernel_error)?;
    Ok(with_intrinsic_lineage(shape))
}

/// `Part.makeShellFromWires(shape, op=None)` — create a shell from boundary
/// wires (upstream names the keyword `shape`, although it is a list of wires).
///
/// Each wire becomes one planar face; the wires must bound faces that together
/// form an open or closed shell.
#[pyfunction]
#[pyo3(signature = (shape, op = None))]
fn makeShellFromWires(shape: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shell = ferrocad_part::backend()
        .make_shell_from_wires(&shape_list(shape)?)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shell))
}

/// `Part.makeSolid(shape, op=None)` — create a solid from the shells of `shape`.
///
/// A shell is used directly. Several shells make a solid with cavities: the
/// first shell is the outer skin and the rest are hollows.
#[pyfunction]
#[pyo3(signature = (shape, op = None))]
fn makeSolid(shape: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shell = shape_argument(shape)?;
    let solid = solid_from(&shells_of(&shell))?;
    Ok(with_intrinsic_lineage(solid))
}

/// `Part.makeLoft(shapes, solid=False, ruled=False, closed=False, max_degree=5, op=None)`.
///
/// Lofts through the section wires in the given order. `ruled`, `closed` and
/// `max_degree` are accepted for upstream compatibility; this slice builds the
/// default smoothed, open, degree-5 loft.
#[pyfunction]
#[pyo3(signature = (shapes, solid = false, ruled = false, closed = false, max_degree = 5, op = None))]
fn makeLoft(
    shapes: &Bound<'_, PyAny>,
    solid: bool,
    ruled: bool,
    closed: bool,
    max_degree: i32,
    op: Option<&str>,
) -> PyResult<PartShape> {
    let _ = (ruled, closed, max_degree, op);
    let lofts = ferrocad_part::backend()
        .make_loft(&shape_list(shapes)?, solid)
        .map_err(kernel_error)?;
    Ok(with_intrinsic_lineage(lofts))
}

/// `Part.makeRuledSurface(edge1, edge2)` — a ruled surface between two edges.
#[pyfunction]
#[pyo3(signature = (edge1, edge2))]
fn makeRuledSurface(edge1: &Bound<'_, PyAny>, edge2: &Bound<'_, PyAny>) -> PyResult<PartShape> {
    let first = shape_argument(edge1)?;
    let second = shape_argument(edge2)?;
    let surface = ferrocad_part::backend()
        .make_ruled_surface(&first, &second)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(surface))
}

/// `Part.makeFilledFace(edges, op=None)` — create a face out of a list of edges.
///
/// The edges are joined into one wire and filled with the kernel's filling
/// surface. Upstream's optional fitting parameters are not implemented.
#[pyfunction]
#[pyo3(signature = (shapes, op = None))]
fn makeFilledFace(shapes: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let face = ferrocad_part::backend()
        .make_filled_face(&shape_list(shapes)?)
        .map_err(kernel_error)?;
    Ok(with_intrinsic_lineage(face))
}

/// `Part.makeFace(wires, face_maker="Part::FaceMakerBullseye", noElementMap=False)`.
///
/// `Part::FaceMakerBuildFace` is not a single-face builder: upstream's
/// `FaceMakerBuildFace::Build_Essence` collects every wire's edges, splits them
/// at their intersections and returns *all* bounded regions. The kernel's
/// `make_faces` is that algorithm, and the curve module already routes its
/// subclass `Part::FaceMakerUnified` there, so BuildFace is sent down the same
/// path. This is what turns the annulus cut by two connecting segments into the
/// inner disc plus the two halves of the annulus. Every other maker keeps the
/// curve module's constructor (`Part.Face`) and therefore its behaviour.
#[pyfunction]
#[pyo3(signature = (wires, face_maker = "Part::FaceMakerBullseye", noElementMap = false))]
fn makeFace(
    py: Python<'_>,
    wires: &Bound<'_, PyAny>,
    face_maker: &str,
    noElementMap: bool,
) -> PyResult<PyObject> {
    let _ = noElementMap;
    let maker = if face_maker == "Part::FaceMakerBuildFace" {
        "Part::FaceMakerUnified"
    } else {
        face_maker
    };
    let part = py.import("Part")?;
    Ok(part.getattr("Face")?.call1((wires, maker))?.unbind())
}

/// The precision below which a plane dimension or an extrusion length is zero
/// (OCCT's `Precision::Confusion`).
const CONFUSION: f64 = 1e-7;

/// Register the document object types this module provides. Called from the
/// Part module's kernel bootstrap, after `ferrocad_part::register`.
pub(crate) fn register_objects() {
    object_registry::register(RULED_SURFACE_TYPE, Arc::new(RuledSurface));
    object_registry::register(MIRRORING_TYPE, Arc::new(Mirroring));
    object_registry::register(SKETCH_TYPE, Arc::new(SketchObject));
    object_registry::register(BODY_TYPE, Arc::new(Body));
    object_registry::register(ADDITIVE_BOX_TYPE, Arc::new(AdditiveBox));
    object_registry::register(PRISM_TYPE, Arc::new(PrismPrimitive));
    object_registry::register(TORUS_TYPE, Arc::new(TorusPrimitive));
    object_registry::register(WEDGE_TYPE, Arc::new(WedgePrimitive));
    object_registry::register(REVOLUTION_TYPE, Arc::new(Revolution));
    object_registry::register(HELIX_TYPE, Arc::new(HelixPrimitive));
    object_registry::register(SWEEP_TYPE, Arc::new(Sweep));
}

// ---------------------------------------------------------------------------
// `Part.makePlane`
// ---------------------------------------------------------------------------

/// A unit vector in the plane normal to `normal`, used when the caller does not
/// supply an X direction (the same choice `gp_Ax3(point, normal)` makes: the
/// global X axis, or global Y when X is (nearly) parallel to the normal).
fn plane_x_direction(normal: &Vector3) -> Vector3 {
    let global_x = Vector3::new(1.0, 0.0, 0.0);
    let candidate = if normal.dot(&global_x).abs() < 0.9 {
        global_x
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    candidate.sub(&normal.scale(normal.dot(&candidate))).normalize()
}

/// `Part.makePlane(length, width, pnt=None, dir=None, xdir=None)` — a rectangular
/// planar face spanning `[0, length] x [0, width]` in the frame at `pnt`.
///
/// As upstream, the face defaults to the XY plane at the origin; `dir` is the
/// plane normal and `xdir` orients the patch's local X axis. The face is built
/// through the kernel (`make_edge`/`make_wire`/`make_face`), so it is real B-Rep
/// geometry with a `Part::GeomPlane` surface.
#[pyfunction]
#[pyo3(signature = (length, width, pnt = None, dir = None, xdir = None))]
fn makePlane(
    length: f64,
    width: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    xdir: Option<&Bound<'_, PyAny>>,
) -> PyResult<PartShape> {
    if length < CONFUSION {
        return Err(PyValueError::new_err("length of plane too small"));
    }
    if width < CONFUSION {
        return Err(PyValueError::new_err("width of plane too small"));
    }
    let origin = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(Vector3::zero);
    let normal = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    if normal.is_null() {
        return Err(OCCError::new_err("creation of plane failed"));
    }
    let normal = normal.normalize();
    let x_axis = match xdir {
        Some(value) => {
            let candidate = curves::vector(value)?;
            let projected = candidate.sub(&normal.scale(normal.dot(&candidate)));
            if projected.is_null() {
                return Err(OCCError::new_err("creation of plane failed"));
            }
            projected.normalize()
        }
        None => plane_x_direction(&normal),
    };
    let y_axis = normal.cross(&x_axis);

    let corners = [
        origin,
        origin.add(&x_axis.scale(length)),
        origin
            .add(&x_axis.scale(length))
            .add(&y_axis.scale(width)),
        origin.add(&y_axis.scale(width)),
    ];
    let mut edges = Vec::with_capacity(4);
    for index in 0..4 {
        let start = corners[index];
        let end = corners[(index + 1) % 4];
        let segment = LineSegment::new(start, end)
            .map_err(|error| OCCError::new_err(error.to_string()))?;
        edges.push(
            ferrocad_part::backend()
                .make_edge(&EdgeCurve::LineSegment(segment))
                .map_err(kernel_error)?,
        );
    }
    let wire = ferrocad_part::backend()
        .make_wire(&edges)
        .map_err(kernel_error)?;
    let face = ferrocad_part::backend()
        .make_face(&[wire])
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(face))
}

// ---------------------------------------------------------------------------
// `Part.insert` (deprecated)
// ---------------------------------------------------------------------------

/// The exact deprecation message upstream emits (`Base::PythonApiDeprecation`).
const INSERT_DEPRECATION: &str = "Method 'Part.insert' is deprecated since FreeCAD 26.3 \
    and will be removed in FreeCAD 27.2; use Import.insert instead.";

/// `Part.insert(filename, document)` — the deprecated import helper.
///
/// Order matters and mirrors upstream (`AppPartPy.cpp`): the extension is
/// validated first, then the deprecation warning is raised, and only afterwards
/// is the document looked up or created. When warnings are errors (as in the
/// upstream lifecycle test) the warning propagates and no document is created.
#[pyfunction]
fn insert(py: Python<'_>, filename: &str, document: &str) -> PyResult<()> {
    let path = std::path::Path::new(filename);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if extension.is_empty() {
        return Err(PyRuntimeError::new_err("No file extension"));
    }

    py.import("warnings")?.call_method1(
        "warn",
        (INSERT_DEPRECATION, py.get_type::<PyDeprecationWarning>()),
    )?;

    // Upstream creates the document only after the warning. STEP/IGES have no
    // importer in this slice; everything else is read as a kernel B-Rep file.
    let freecad = py.import("ferrocad")?;
    let documents = freecad.call_method0("listDocuments")?;
    let documents = documents.downcast::<PyDict>()?;
    let doc = match documents.get_item(document)? {
        Some(doc) => doc,
        None => freecad.call_method1("newDocument", (document,))?,
    };
    let lower = extension.to_ascii_lowercase();
    if matches!(lower.as_str(), "stp" | "step" | "igs" | "iges") {
        return Err(OCCError::new_err(format!(
            "importing .{extension} files is not implemented yet; read the file with Import instead"
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| OCCError::new_err(format!("cannot read {filename}: {error}")))?;
    let shape = ferrocad_part::backend()
        .load_shape(&bytes)
        .ok_or_else(|| OCCError::new_err(format!("cannot read a shape from {filename}")))?;
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Shape");
    let object = doc.call_method1("addObject", ("Part::Feature", name))?;
    object.setattr("Shape", Py::new(py, PartShape::from_shape(shape))?)?;
    doc.call_method0("recompute")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Document types: `Part::RuledSurface` and `Part::Extrusion`
// ---------------------------------------------------------------------------

/// `Part::RuledSurface` (`PartFeatures.cpp`).
pub const RULED_SURFACE_TYPE: &str = "Part::RuledSurface";

const SHAPE: &str = "Shape";

fn output_shape() -> Property {
    make_shape_property(ShapeProperty::empty())
}

fn output_property() -> (String, Property, u32) {
    (SHAPE.to_string(), output_shape(), prop_status::OUTPUT)
}

/// Store a recompute outcome the way the Part containers do: the rebuilt shape,
/// or an empty output with the invalid flag set.
fn store(doc: &mut Document, id: ObjectId, result: Result<Shape, String>) {
    match result {
        Ok(shape) => {
            doc.set_property_raw(
                id,
                SHAPE,
                make_shape_property(ShapeProperty::with_lineage(shape, Lineage::default())),
            );
        }
        Err(_) => {
            doc.mark_invalid(id);
            doc.set_property_raw(id, SHAPE, output_shape());
        }
    }
}

fn link_sub_property(doc: &Document, id: ObjectId, name: &str) -> Option<(String, Vec<String>)> {
    match doc.object(id)?.properties.get(name)? {
        Property::LinkSub(object, subs) if !object.is_empty() => {
            Some((object.clone(), subs.clone()))
        }
        Property::Link(object) if !object.is_empty() => Some((object.clone(), Vec::new())),
        _ => None,
    }
}

fn float_property(doc: &Document, id: ObjectId, name: &str, default: f64) -> f64 {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Float(value)) => *value,
        Some(Property::Integer(value)) => *value as f64,
        _ => default,
    }
}

fn bool_property(doc: &Document, id: ObjectId, name: &str, default: bool) -> bool {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Bool(value)) => *value,
        _ => default,
    }
}

fn vector_property(doc: &Document, id: ObjectId, name: &str) -> Option<Vector3> {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Vector(value)) => Some(*value),
        _ => None,
    }
}

fn enum_index_property(doc: &Document, id: ObjectId, name: &str) -> usize {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Enumeration(_, index)) => *index,
        _ => 0,
    }
}

/// The shape a link property names, resolving a single sub-element when one is
/// selected (upstream's `Feature::getTopoShape(..., NeedSubElement)`).
fn linked_shape(
    doc: &Document,
    link: &(String, Vec<String>),
) -> Result<Shape, String> {
    let (name, subs) = link;
    let id = doc
        .get_by_name(name)
        .ok_or_else(|| format!("no object named {name}"))?;
    let shape = doc
        .object(id)
        .and_then(|object| object.properties.get(SHAPE))
        .and_then(shape_of)
        .and_then(|property| property.shape())
        .ok_or_else(|| format!("object {name} has no shape"))?;
    match subs.as_slice() {
        [] => Ok(shape),
        [sub] if sub.is_empty() => Ok(shape),
        [sub] => ferrocad_part::backend()
            .resolve(&shape, sub)
            .ok_or_else(|| format!("object {name} has no sub-shape {sub}")),
        _ => Err("not exactly one sub-shape linked".to_string()),
    }
}

/// `Part::RuledSurface`: `Shape` is the ruled surface between `Curve1` and
/// `Curve2`. `Orientation` is accepted for compatibility; the seam's builder
/// always uses the curves' own orientation (upstream's `Automatic`).
struct RuledSurface;

impl ObjectType for RuledSurface {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Curve1".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "Curve2".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "Orientation".to_string(),
                Property::Enumeration(
                    vec![
                        "Automatic".to_string(),
                        "Forward".to_string(),
                        "Reversed".to_string(),
                    ],
                    0,
                ),
                prop_status::NONE,
            ),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Ruled Surface".to_string(), format!("Part ruled surface {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let first = link_sub_property(doc, id, "Curve1")
                .ok_or_else(|| "the ruled surface has no Curve1 link".to_string())?;
            let second = link_sub_property(doc, id, "Curve2")
                .ok_or_else(|| "the ruled surface has no Curve2 link".to_string())?;
            let first = linked_shape(doc, &first)?;
            let second = linked_shape(doc, &second)?;
            ferrocad_part::backend()
                .make_ruled_surface(&first, &second)
                .map_err(|error| error.to_string())
        })();
        store(doc, id, result);
    }
}

// ---------------------------------------------------------------------------
// Elementary surface descriptions: `Part.Cone` and `Part.Toroid`
// ---------------------------------------------------------------------------
//
// Upstream these are `Part::GeomCone` / `Part::GeomToroid`: analytic surface
// descriptions, distinct from the `Part.makeCone` primitive. This slice exposes
// the parameters and the analytic evaluation (`value`, `getDN`, `parameter`),
// which is enough for the surface tests without inventing a B-Rep. `toShape` on
// a cone returns an analytic face handle (`toNurbs()` / `Face1.Surface`) whose
// parameterisation is the cone's own, so the surface derivatives match exactly.

/// The orthonormal frame of a rotational surface: its X and Y directions.
fn surface_frame(axis: Vector3) -> (Vector3, Vector3) {
    let x = plane_x_direction(&axis);
    let y = axis.cross(&x);
    (x, y)
}

/// `Part.Cone` — an infinite conical surface (`Part::GeomCone`).
///
/// The empty constructor matches upstream: radius 1, semi-angle 45° and the +Z
/// axis through the origin. The `radius`/`center`/`axis` keywords are our
/// extension, mirroring `Part.Cylinder`/`Part.Sphere`.
#[pyclass(name = "Cone", module = "Part")]
#[derive(Clone)]
pub struct PyCone {
    radius: f64,
    /// The semi-angle in radians (upstream's `SemiAngle`).
    semi_angle: f64,
    center: Vector3,
    /// The unit axis direction.
    axis: Vector3,
    /// The surface's `u = 0` direction (upstream's `gp_Ax3::XDirection`).
    x_dir: Vector3,
    /// Whether the frame is direct (`y_dir = axis × x_dir`) or indirect.
    direct: bool,
}

impl PyCone {
    /// A cone with the backend's own surface frame, so `parameter` and
    /// `Face.valueAt` agree exactly.
    pub(crate) fn from_parts(
        radius: f64,
        center: Vector3,
        axis: Vector3,
        x_dir: Vector3,
        semi_angle: f64,
        direct: bool,
    ) -> Self {
        Self {
            radius,
            semi_angle,
            center,
            axis: axis.normalize(),
            x_dir: x_dir.normalize(),
            direct,
        }
    }

    fn frame(&self) -> (Vector3, Vector3) {
        let mut y = self.axis.cross(&self.x_dir);
        if !self.direct {
            y = y.neg();
        }
        (self.x_dir, y)
    }

    /// The analytic description `Part.Face(cone, wires)` builds on.
    pub(crate) fn spec(&self) -> SurfaceSpec {
        SurfaceSpec::Cone {
            origin: self.center,
            axis: self.axis,
            x_dir: self.x_dir,
            radius: self.radius,
            semi_angle: self.semi_angle,
            direct: self.direct,
        }
    }

    /// The apex: where the radius vanishes along the axis.
    fn apex(&self) -> Vector3 {
        self.center.add(&self.axis.scale(-self.radius / self.semi_angle.tan()))
    }
}

#[pymethods]
impl PyCone {
    #[new]
    #[pyo3(signature = (radius = 1.0, center = None, axis = None, semi_angle = None))]
    fn new(
        radius: f64,
        center: Option<&Bound<'_, PyAny>>,
        axis: Option<&Bound<'_, PyAny>>,
        semi_angle: Option<f64>,
    ) -> PyResult<Self> {
        let center = center
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(Vector3::zero);
        let axis = axis
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        if axis.is_null() {
            return Err(OCCError::new_err("Part.Cone needs a non-zero axis"));
        }
        let semi_angle = semi_angle.unwrap_or(PI / 4.0);
        if !semi_angle.is_finite() || semi_angle.abs() >= PI / 2.0 {
            return Err(OCCError::new_err(
                "Part.Cone needs a semi-angle below 90 degrees",
            ));
        }
        let axis = axis.normalize();
        let (x_dir, _) = surface_frame(axis);
        Ok(Self {
            radius,
            semi_angle,
            center,
            axis,
            x_dir,
            direct: true,
        })
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.radius
    }

    /// Upstream's `Part::GeomCone` type id.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomCone"
    }

    #[setter]
    fn set_Radius(&mut self, value: f64) {
        self.radius = value;
    }

    #[getter]
    fn SemiAngle(&self) -> f64 {
        self.semi_angle
    }

    #[setter]
    fn set_SemiAngle(&mut self, value: f64) -> PyResult<()> {
        if !value.is_finite() || value.abs() >= PI / 2.0 {
            return Err(OCCError::new_err(
                "Part.Cone needs a semi-angle below 90 degrees",
            ));
        }
        self.semi_angle = value;
        Ok(())
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        curves::py_vector(py, self.center)
    }

    #[setter]
    fn set_Center(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.center = curves::vector(value)?;
        Ok(())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        curves::py_vector(py, self.axis)
    }

    #[setter]
    fn set_Axis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = curves::vector(value)?;
        if axis.is_null() {
            return Err(OCCError::new_err("Part.Cone needs a non-zero axis"));
        }
        self.axis = axis.normalize();
        let (x_dir, _) = surface_frame(self.axis);
        self.x_dir = x_dir;
        self.direct = true;
        Ok(())
    }

    #[getter]
    fn Apex(&self, py: Python<'_>) -> PyResult<PyObject> {
        curves::py_vector(py, self.apex())
    }

    /// The surface point at `(u, v)`: `u` around the axis, `v` the slant
    /// distance along the surface (`Geom_ConicalSurface`'s parameterisation).
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let (x, y) = self.frame();
        let radius = self.radius + v * self.semi_angle.sin();
        let radial = x.scale(u.cos()).add(&y.scale(u.sin()));
        let point = self
            .center
            .add(&radial.scale(radius))
            .add(&self.axis.scale(v * self.semi_angle.cos()));
        curves::py_vector(py, point)
    }

    /// `getDN(u, v, Nu, Nv)` — the analytic derivative of the cone surface.
    fn getDN(&self, py: Python<'_>, u: f64, v: f64, Nu: u32, Nv: u32) -> PyResult<PyObject> {
        let (x, y) = self.frame();
        let (sin, cos) = (self.semi_angle.sin(), self.semi_angle.cos());
        let radius = self.radius + v * sin;
        let radial = x.scale(u.cos()).add(&y.scale(u.sin()));
        let tangent = x.scale(-u.sin()).add(&y.scale(u.cos()));
        let vector = match (Nu, Nv) {
            (1, 0) => tangent.scale(radius),
            (0, 1) => radial.scale(sin).add(&self.axis.scale(cos)),
            (1, 1) => tangent.scale(sin),
            (2, 0) => radial.scale(-radius),
            (0, 2) => Vector3::zero(),
            _ => {
                return Err(OCCError::new_err(
                    "Cone.getDN: only derivatives up to order two are implemented",
                ))
            }
        };
        curves::py_vector(py, vector)
    }

    /// The `(u, v)` parameters of a point on the surface (the inverse of
    /// [`value`](PyCone::value)). `v` is the slant distance.
    fn parameter(&self, point: &Bound<'_, PyAny>) -> PyResult<(f64, f64)> {
        let point = curves::vector(point)?;
        let relative = point.sub(&self.center);
        let axial = relative.dot(&self.axis);
        let (x, y) = self.frame();
        let radial = relative.sub(&self.axis.scale(axial));
        let mut u = radial.dot(&y).atan2(radial.dot(&x));
        if u < 0.0 {
            u += 2.0 * PI;
        }
        let cos = self.semi_angle.cos();
        let v = if cos.abs() > 1e-12 { axial / cos } else { axial };
        Ok((u, v))
    }

    /// The cone bounded by `(u1, u2) x (v1, v2)`, as an analytic face handle.
    fn toShape(&self, _u1: f64, _u2: f64, _v1: f64, _v2: f64) -> PyConeShape {
        PyConeShape {
            cone: self.clone(),
        }
    }

    /// A cone is periodic around its axis (`GeomSurface.isUPeriodic`).
    fn isUPeriodic(&self) -> bool {
        true
    }

    /// A cone's slant is unbounded, not periodic (`GeomSurface.isVPeriodic`).
    fn isVPeriodic(&self) -> bool {
        false
    }

    /// The angular period around the axis (`GeomSurface.UPeriod`).
    fn UPeriod(&self) -> f64 {
        2.0 * PI
    }

    /// A cone has no V period (`GeomSurface.VPeriod`).
    fn VPeriod(&self) -> f64 {
        0.0
    }

    /// The parametric bounds `(u1, u2, v1, v2)` (`Geom_Surface::Bounds`): one
    /// full turn around the axis and the whole (unbounded) slant.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        let infinite = f64::MAX;
        (0.0, 2.0 * PI, -infinite, infinite)
    }

    fn __repr__(&self) -> String {
        "<Cone object>".to_string()
    }
}

/// The cone patch returned by `Part.Cone.toShape`.
#[pyclass(name = "ConeShape", module = "Part")]
#[derive(Clone)]
pub struct PyConeShape {
    cone: PyCone,
}

#[pymethods]
impl PyConeShape {
    /// The face's NURBS form. The cone is already exact and rational-quadratic,
    /// so this handle keeps the same analytic surface.
    fn toNurbs(&self) -> Self {
        self.clone()
    }

    #[getter]
    fn Face1(&self) -> PyConeFace {
        PyConeFace {
            surface: self.cone.clone(),
        }
    }
}

/// `ConeShape.Face1`: a face whose surface is the cone.
#[pyclass(name = "ConeFace", module = "Part")]
#[derive(Clone)]
pub struct PyConeFace {
    surface: PyCone,
}

#[pymethods]
impl PyConeFace {
    #[getter]
    fn Surface(&self) -> PyCone {
        self.surface.clone()
    }
}

/// `Part.Toroid` — an infinite toroidal surface (`Part::GeomToroid`).
///
/// The empty constructor matches upstream: major radius 5, minor radius 1, +Z
/// axis through the origin.
#[pyclass(name = "Toroid", module = "Part")]
#[derive(Clone)]
pub struct PyToroid {
    major_radius: f64,
    minor_radius: f64,
    center: Vector3,
    axis: Vector3,
    /// The surface's `u = 0` direction (upstream's `gp_Ax3::XDirection`).
    x_dir: Vector3,
    /// Whether the frame is direct (`y_dir = axis × x_dir`) or indirect.
    direct: bool,
}

impl PyToroid {
    /// A torus with the backend's own surface frame, so `parameter` and
    /// `Face.valueAt` agree exactly.
    pub(crate) fn from_parts(
        major_radius: f64,
        minor_radius: f64,
        center: Vector3,
        axis: Vector3,
        x_dir: Vector3,
        direct: bool,
    ) -> Self {
        Self {
            major_radius,
            minor_radius,
            center,
            axis: axis.normalize(),
            x_dir: x_dir.normalize(),
            direct,
        }
    }

    fn frame(&self) -> (Vector3, Vector3) {
        let mut y = self.axis.cross(&self.x_dir);
        if !self.direct {
            y = y.neg();
        }
        (self.x_dir, y)
    }

    /// The analytic description `Part.Face(torus, wires)` builds on.
    pub(crate) fn spec(&self) -> SurfaceSpec {
        SurfaceSpec::Torus {
            center: self.center,
            axis: self.axis,
            x_dir: self.x_dir,
            major_radius: self.major_radius,
            minor_radius: self.minor_radius,
            direct: self.direct,
        }
    }
}

#[pymethods]
impl PyToroid {
    #[new]
    #[pyo3(signature = (major_radius = 5.0, minor_radius = 1.0, center = None, axis = None))]
    fn new(
        major_radius: f64,
        minor_radius: f64,
        center: Option<&Bound<'_, PyAny>>,
        axis: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        if !(major_radius > 0.0) || !(minor_radius > 0.0) || minor_radius > major_radius {
            return Err(PyRuntimeError::new_err(
                "Major radius must be positive and higher than minor radius",
            ));
        }
        let center = center
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(Vector3::zero);
        let axis = axis
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
        if axis.is_null() {
            return Err(OCCError::new_err("Part.Toroid needs a non-zero axis"));
        }
        let axis = axis.normalize();
        let (x_dir, _) = surface_frame(axis);
        Ok(Self {
            major_radius,
            minor_radius,
            center,
            axis,
            x_dir,
            direct: true,
        })
    }

    #[getter]
    fn MajorRadius(&self) -> f64 {
        self.major_radius
    }

    /// Upstream's `Part::GeomToroid` type id.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomToroid"
    }

    #[setter]
    fn set_MajorRadius(&mut self, value: f64) -> PyResult<()> {
        if !(value > 0.0) || value < self.minor_radius {
            return Err(PyRuntimeError::new_err(
                "Major radius must be positive and higher than minor radius",
            ));
        }
        self.major_radius = value;
        Ok(())
    }

    #[getter]
    fn MinorRadius(&self) -> f64 {
        self.minor_radius
    }

    #[setter]
    fn set_MinorRadius(&mut self, value: f64) -> PyResult<()> {
        if !(value > 0.0) || value > self.major_radius {
            return Err(PyRuntimeError::new_err(
                "Minor radius must be positive and lower than major radius",
            ));
        }
        self.minor_radius = value;
        Ok(())
    }

    #[getter]
    fn Center(&self, py: Python<'_>) -> PyResult<PyObject> {
        curves::py_vector(py, self.center)
    }

    #[setter]
    fn set_Center(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.center = curves::vector(value)?;
        Ok(())
    }

    #[getter]
    fn Axis(&self, py: Python<'_>) -> PyResult<PyObject> {
        curves::py_vector(py, self.axis)
    }

    #[setter]
    fn set_Axis(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let axis = curves::vector(value)?;
        if axis.is_null() {
            return Err(OCCError::new_err("Part.Toroid needs a non-zero axis"));
        }
        self.axis = axis.normalize();
        let (x_dir, _) = surface_frame(self.axis);
        self.x_dir = x_dir;
        self.direct = true;
        Ok(())
    }

    /// The surface point at `(u, v)`: `u` around the axis, `v` around the tube.
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let (x, y) = self.frame();
        let radial = x.scale(u.cos()).add(&y.scale(u.sin()));
        let point = self
            .center
            .add(&radial.scale(self.major_radius + self.minor_radius * v.cos()))
            .add(&self.axis.scale(self.minor_radius * v.sin()));
        curves::py_vector(py, point)
    }

    /// The `(u, v)` parameters of a point on the surface (the inverse of
    /// [`value`](PyToroid::value)): `u` around the axis, `v` around the tube.
    ///
    /// Upstream's `GeometrySurfacePy::parameter` answers this through
    /// `ShapeAnalysis_Surface::ValueOfUV`, which is how `Face.Surface.parameter`
    /// reads a torus face's click point.
    fn parameter(&self, point: &Bound<'_, PyAny>) -> PyResult<(f64, f64)> {
        let point = curves::vector(point)?;
        let relative = point.sub(&self.center);
        let axial = relative.dot(&self.axis);
        let (x, y) = self.frame();
        let radial = relative.sub(&self.axis.scale(axial));
        let mut u = radial.dot(&y).atan2(radial.dot(&x));
        if u < 0.0 {
            u += 2.0 * PI;
        }
        let mut v = axial.atan2(radial.length() - self.major_radius);
        if v < 0.0 {
            v += 2.0 * PI;
        }
        Ok((u, v))
    }

    /// The rational B-spline description of the torus: the knot vector of the
    /// standard full-circle rational quadratic representation in both
    /// directions, so the parameter ranges are the full `0..2π`.
    fn toBSpline(&self) -> PyBSplineSurface {
        PyBSplineSurface::torus()
    }

    /// A torus is periodic in both directions (`GeomSurface.isUPeriodic`).
    fn isUPeriodic(&self) -> bool {
        true
    }

    /// A torus is periodic in both directions (`GeomSurface.isVPeriodic`).
    fn isVPeriodic(&self) -> bool {
        true
    }

    /// The angular period around the main axis (`GeomSurface.UPeriod`).
    fn UPeriod(&self) -> f64 {
        2.0 * PI
    }

    /// The angular period around the tube (`GeomSurface.VPeriod`).
    fn VPeriod(&self) -> f64 {
        2.0 * PI
    }

    /// The parametric bounds `(u1, u2, v1, v2)` (`Geom_Surface::Bounds`): a full
    /// turn in each direction.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        (0.0, 2.0 * PI, 0.0, 2.0 * PI)
    }

    fn __repr__(&self) -> String {
        "<Toroid object>".to_string()
    }
}

/// The knot vector of a full circle in its rational quadratic B-spline
/// representation (four 90° arcs, nine poles, degree two).
fn circle_knots() -> Vec<f64> {
    let quarter = PI / 2.0;
    vec![
        0.0,
        0.0,
        0.0,
        quarter,
        quarter,
        2.0 * quarter,
        2.0 * quarter,
        3.0 * quarter,
        3.0 * quarter,
        4.0 * quarter,
        4.0 * quarter,
        4.0 * quarter,
    ]
}

/// `Part.BSplineSurface` — the surface counterpart of `Part.BSplineCurve`.
///
/// It carries the full control-point description upstream's class exposes:
/// poles, weights, knots with multiplicities and the two degrees. Two ways build
/// it:
///
/// - [`interpolate`](Self::interpolate) (`GeomAPI_PointsToBSplineSurface` through
///   a grid), which keeps the knot sequences and bounds `Toroid.toBSpline` reports;
/// - [`buildFromPolesMultsKnots`](Self::buildFromPolesMultsKnots), the explicit
///   control-point form, with upstream's own validation.
///
/// `Face.Surface` returns this class for a B-spline face, so `UDegree`, `getPoles`
/// and `Face.valueAt`/`parameter` agree with the face's own surface.
#[pyclass(name = "BSplineSurface", module = "Part")]
#[derive(Clone)]
pub struct PyBSplineSurface {
    /// The expanded (unique knot repeated by its multiplicity) knot sequence.
    u_knots: Vec<f64>,
    v_knots: Vec<f64>,
    u_periodic: bool,
    v_periodic: bool,
    /// The parameter bounds when they differ from the knot extents (an
    /// interpolated surface); `None` derives them from the knot vectors.
    u_bounds: Option<(f64, f64)>,
    v_bounds: Option<(f64, f64)>,
    /// The interpolation grid, kept so `toShape` can rebuild the surface's face
    /// (`None` for the torus alias or an explicitly built surface).
    points: Option<Vec<Vec<Vector3>>>,
    /// The explicit pole/knot geometry, present once the surface has been built.
    surface: Option<ferrocad_occt::part_ops::BsplineSurface>,
}

impl PyBSplineSurface {
    fn torus() -> Self {
        Self {
            u_knots: circle_knots(),
            v_knots: circle_knots(),
            u_periodic: false,
            v_periodic: false,
            u_bounds: None,
            v_bounds: None,
            points: None,
            surface: None,
        }
    }

    /// The `Part.BSplineSurface` view of a B-spline face (`Face.Surface`).
    pub(crate) fn from_geometry(geometry: ferrocad_occt::part_ops::BsplineSurface) -> Self {
        let expand = |knots: &[(f64, i32)]| -> Vec<f64> {
            knots
                .iter()
                .flat_map(|(knot, multiplicity)| std::iter::repeat_n(*knot, (*multiplicity).max(0) as usize))
                .collect()
        };
        Self {
            u_knots: expand(&geometry.u_knots),
            v_knots: expand(&geometry.v_knots),
            u_periodic: geometry.u_periodic,
            v_periodic: geometry.v_periodic,
            u_bounds: Some(geometry.u_bounds),
            v_bounds: Some(geometry.v_bounds),
            points: None,
            surface: Some(geometry),
        }
    }

    /// The explicit geometry, if the surface has any.
    pub(crate) fn geometry(&self) -> Option<&ferrocad_occt::part_ops::BsplineSurface> {
        self.surface.as_ref()
    }

    fn require_geometry(&self) -> PyResult<&ferrocad_occt::part_ops::BsplineSurface> {
        self.surface.as_ref().ok_or_else(|| {
            OCCError::new_err(
                "this BSplineSurface has no control points yet (call buildFromPolesMultsKnots or interpolate)",
            )
        })
    }
}

#[pymethods]
impl PyBSplineSurface {
    #[new]
    fn new() -> Self {
        Self {
            u_knots: Vec::new(),
            v_knots: Vec::new(),
            u_periodic: false,
            v_periodic: false,
            u_bounds: None,
            v_bounds: None,
            points: None,
            surface: None,
        }
    }

    /// The registered type name (`Part::GeomBSplineSurface`).
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomBSplineSurface"
    }

    /// Build the surface from control points:
    /// `buildFromPolesMultsKnots(poles, umults, vmults, uknots=None, vknots=None,
    /// uperiodic=False, vperiodic=False, udegree=3, vdegree=3, weights=None)`.
    ///
    /// `poles` is indexed `[u][v]`. With no knots the two vectors are uniformly
    /// spaced on `[0, 1]`; the pole/multiplicity and inner-multiplicity checks are
    /// upstream's (`BSplineSurfacePy::buildFromPolesMultsKnots`).
    #[pyo3(signature = (poles, umults, vmults, uknots = None, vknots = None, uperiodic = false, vperiodic = false, udegree = 3, vdegree = 3, weights = None))]
    #[allow(clippy::too_many_arguments)]
    fn buildFromPolesMultsKnots(
        &mut self,
        poles: &Bound<'_, PyAny>,
        umults: &Bound<'_, PyAny>,
        vmults: &Bound<'_, PyAny>,
        uknots: Option<&Bound<'_, PyAny>>,
        vknots: Option<&Bound<'_, PyAny>>,
        uperiodic: bool,
        vperiodic: bool,
        udegree: i32,
        vdegree: i32,
        weights: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let mut rows: Vec<Vec<Vector3>> = Vec::new();
        for row in poles.try_iter()? {
            rows.push(curves::parse_points(&row?)?);
        }
        let nu = rows.len();
        let nv = rows.first().map_or(0, Vec::len);
        if nu < 2 || nv < 2 {
            return Err(OCCError::new_err("not enough points given"));
        }
        if rows.iter().any(|row| row.len() != nv) {
            return Err(OCCError::new_err("poles are not a rectangular grid"));
        }
        // Upstream reads poles as [u][v] into a 1..nu by 1..nv OCCT array.
        let mut grid: Vec<Vec<(Vector3, f64)>> = vec![vec![(Vector3::zero(), 1.0); nv]; nu];
        for (u, row) in rows.iter().enumerate() {
            for (v, point) in row.iter().enumerate() {
                grid[u][v] = (*point, 1.0);
            }
        }
        if let Some(weights) = weights {
            let mut weight_rows: Vec<Vec<f64>> = Vec::new();
            for row in weights.try_iter()? {
                weight_rows.push(curves::parse_floats(&row?)?);
            }
            if weight_rows.len() != nu || weight_rows.iter().any(|row| row.len() != nv) {
                return Err(OCCError::new_err("weights and poles mismatch"));
            }
            for (u, row) in weight_rows.iter().enumerate() {
                for (v, weight) in row.iter().enumerate() {
                    grid[u][v].1 = *weight;
                }
            }
        }

        let umults = curves::parse_integers(umults)?;
        let vmults = curves::parse_integers(vmults)?;
        let uknots = uknots.map(curves::parse_floats).transpose()?;
        let vknots = vknots.map(curves::parse_floats).transpose()?;
        if uknots.as_ref().is_some_and(|knots| knots.len() != umults.len())
            || vknots.as_ref().is_some_and(|knots| knots.len() != vmults.len())
        {
            return Err(OCCError::new_err("number of knots and mults mismatch"));
        }
        let u_knots = uknots.unwrap_or_else(|| uniform_knots(umults.len()));
        let v_knots = vknots.unwrap_or_else(|| uniform_knots(vmults.len()));

        let sum = |mults: &[i32], periodic: bool| -> i32 {
            let mut total = 0;
            for (index, multiplicity) in mults.iter().enumerate() {
                if index + 1 < mults.len() || !periodic {
                    total += multiplicity;
                }
            }
            total
        };
        let sum_u = sum(&umults, uperiodic);
        let sum_v = sum(&vmults, vperiodic);
        if (uperiodic && sum_u != nu as i32)
            || (!uperiodic && sum_u - udegree - 1 != nu as i32)
            || (vperiodic && sum_v != nv as i32)
            || (!vperiodic && sum_v - vdegree - 1 != nv as i32)
        {
            return Err(OCCError::new_err("number of poles and sum of mults mismatch"));
        }
        if umults[1..umults.len().saturating_sub(1)]
            .iter()
            .any(|multiplicity| *multiplicity > udegree)
            || vmults[1..vmults.len().saturating_sub(1)]
                .iter()
                .any(|multiplicity| *multiplicity > vdegree)
        {
            return Err(OCCError::new_err(
                "multiplicity of inner knot higher than degree",
            ));
        }

        let geometry = ferrocad_occt::part_ops::BsplineSurface {
            u_degree: udegree,
            v_degree: vdegree,
            u_periodic: uperiodic,
            v_periodic: vperiodic,
            u_bounds: (
                u_knots.first().copied().unwrap_or(0.0),
                u_knots.last().copied().unwrap_or(1.0),
            ),
            v_bounds: (
                v_knots.first().copied().unwrap_or(0.0),
                v_knots.last().copied().unwrap_or(1.0),
            ),
            poles: grid,
            u_knots: u_knots.into_iter().zip(umults).collect(),
            v_knots: v_knots.into_iter().zip(vmults).collect(),
        };
        // Build the OCCT surface once so a malformed description fails here, as
        // upstream's constructor does, rather than at the first query.
        ferrocad_occt::part_ops::bspline_surface_face(&geometry).map_err(kernel_error)?;
        self.u_knots = geometry
            .u_knots
            .iter()
            .flat_map(|(knot, multiplicity)| std::iter::repeat_n(*knot, (*multiplicity).max(0) as usize))
            .collect();
        self.v_knots = geometry
            .v_knots
            .iter()
            .flat_map(|(knot, multiplicity)| std::iter::repeat_n(*knot, (*multiplicity).max(0) as usize))
            .collect();
        self.u_periodic = uperiodic;
        self.v_periodic = vperiodic;
        self.u_bounds = Some(geometry.u_bounds);
        self.v_bounds = Some(geometry.v_bounds);
        self.points = None;
        self.surface = Some(geometry);
        Ok(())
    }

    /// `interpolate(Points, X0, dX, Y0, dY)` — interpolate a grid of points
    /// (`GeomAPI_PointsToBSplineSurface`).
    ///
    /// `Points` is indexed `[u][v]` and must be at least 2 by 2. With no spacing
    /// arguments each entry is a vector; with `X0`/`dX`/`Y0`/`dY` the entries are
    /// z values on a regular XY grid, as upstream's second form. The surface is
    /// stored as its bounds and expanded knot sequences, which `bounds`, `UKnotSequence`
    /// and `VKnotSequence` report.
    #[pyo3(signature = (points, x0 = None, dx = None, y0 = None, dy = None))]
    fn interpolate(
        &mut self,
        points: &Bound<'_, PyAny>,
        x0: Option<f64>,
        dx: Option<f64>,
        y0: Option<f64>,
        dy: Option<f64>,
    ) -> PyResult<()> {
        let spaced = x0.is_some() || dx.is_some() || y0.is_some() || dy.is_some();
        let mut rows: Vec<Vec<Vector3>> = Vec::new();
        for (u, row) in points.try_iter()?.enumerate() {
            let mut values = Vec::new();
            for (v, item) in row?.try_iter()?.enumerate() {
                let item = item?;
                if spaced {
                    let z = item.extract::<f64>().map_err(|_| {
                        PyTypeError::new_err(
                            "interpolate with a spacing expects a grid of z values",
                        )
                    })?;
                    values.push(Vector3::new(
                        x0.unwrap_or(0.0) + u as f64 * dx.unwrap_or(1.0),
                        y0.unwrap_or(0.0) + v as f64 * dy.unwrap_or(1.0),
                        z,
                    ));
                } else {
                    values.push(curves::vector(&item)?);
                }
            }
            rows.push(values);
        }
        if rows.len() < 2 || rows[0].len() < 2 {
            return Err(OCCError::new_err("not enough points given"));
        }
        let data = ferrocad_part::backend()
            .bspline_surface_interpolate(&rows)
            .map_err(kernel_error)?;
        self.u_knots = data.u_knots;
        self.v_knots = data.v_knots;
        self.u_bounds = Some(data.u_bounds);
        self.v_bounds = Some(data.v_bounds);
        self.u_periodic = false;
        self.v_periodic = false;
        self.points = Some(rows.clone());
        // Upstream's `interpolate` replaces the surface handle, so `value`,
        // `parameter` and the degree/pole getters keep working on the surface
        // that was just built (they read the explicit geometry).
        let face = ferrocad_part::backend()
            .bspline_surface_face(&rows)
            .map_err(kernel_error)?;
        self.surface = ferrocad_occt::part_ops::bspline_surface_data(&face);
        Ok(())
    }

    /// The face of the surface (`BSplineSurface.toShape`).
    ///
    /// A surface built from poles reports its face over the whole parameter
    /// domain; an interpolated one rebuilds the interpolation face. An empty or
    /// directly constructed surface raises `OCCError`.
    fn toShape(&self, py: Python<'_>) -> PyResult<Py<curves::PyFace>> {
        let shape = if let Some(geometry) = &self.surface {
            ferrocad_occt::part_ops::bspline_surface_face(geometry).map_err(kernel_error)?
        } else if let Some(points) = &self.points {
            ferrocad_part::backend()
                .bspline_surface_face(points)
                .map_err(|error| OCCError::new_err(error.to_string()))?
        } else {
            return Err(OCCError::new_err(
                "BSplineSurface.toShape needs a built surface (call buildFromPolesMultsKnots or interpolate first)",
            ));
        };
        Py::new(py, curves::PyFace::from_shape(shape))
    }

    /// The point at surface parameters `(u, v)` (`GeomSurface.value`).
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let geometry = self.require_geometry()?;
        let point = ferrocad_occt::part_ops::bspline_surface_value(geometry, u, v)
            .map_err(kernel_error)?;
        curves::py_vector(py, point)
    }

    /// The `(u, v)` of `point` on the surface (`GeomSurface.parameter`).
    fn parameter(&self, py: Python<'_>, point: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let geometry = self.require_geometry()?;
        let (u, v) = ferrocad_occt::part_ops::bspline_surface_parameter(
            geometry,
            curves::vector(point)?,
        )
        .map_err(kernel_error)?;
        Ok(PyTuple::new(py, [u, v])?.into_any().unbind())
    }

    fn setUPeriodic(&mut self) {
        self.u_periodic = true;
    }

    fn setVPeriodic(&mut self) {
        self.v_periodic = true;
    }

    fn isUPeriodic(&self) -> bool {
        self.u_periodic
    }

    fn isVPeriodic(&self) -> bool {
        self.v_periodic
    }

    // --- control-point description -------------------------------------

    /// The degree in `u` (`BSplineSurface.UDegree`).
    #[getter]
    fn UDegree(&self) -> PyResult<i32> {
        Ok(self.require_geometry()?.u_degree)
    }

    /// The degree in `v` (`BSplineSurface.VDegree`).
    #[getter]
    fn VDegree(&self) -> PyResult<i32> {
        Ok(self.require_geometry()?.v_degree)
    }

    #[getter]
    fn MaxDegree(&self) -> PyResult<i32> {
        let geometry = self.require_geometry()?;
        Ok(geometry.u_degree.max(geometry.v_degree))
    }

    #[getter]
    fn NbUPoles(&self) -> PyResult<usize> {
        Ok(self.require_geometry()?.poles.len())
    }

    #[getter]
    fn NbVPoles(&self) -> PyResult<usize> {
        Ok(self.require_geometry()?.poles.first().map_or(0, Vec::len))
    }

    #[getter]
    fn NbUKnots(&self) -> PyResult<usize> {
        Ok(self.require_geometry()?.u_knots.len())
    }

    #[getter]
    fn NbVKnots(&self) -> PyResult<usize> {
        Ok(self.require_geometry()?.v_knots.len())
    }

    #[getter]
    fn FirstUKnotIndex(&self) -> PyResult<i32> {
        self.require_geometry()?;
        Ok(1)
    }

    #[getter]
    fn LastUKnotIndex(&self) -> PyResult<i32> {
        Ok(self.require_geometry()?.u_knots.len() as i32)
    }

    #[getter]
    fn FirstVKnotIndex(&self) -> PyResult<i32> {
        self.require_geometry()?;
        Ok(1)
    }

    #[getter]
    fn LastVKnotIndex(&self) -> PyResult<i32> {
        Ok(self.require_geometry()?.v_knots.len() as i32)
    }

    /// Whether the surface is closed in `u` (`BSplineSurface.isUClosed`).
    fn isUClosed(&self) -> PyResult<bool> {
        let geometry = self.require_geometry()?;
        Ok(!geometry.poles.is_empty()
            && geometry.poles.iter().all(|row| {
                row.first().zip(row.last()).is_some_and(|(first, last)| {
                    first.0.distance(&last.0) <= ferrocad_curves::TOLERANCE
                })
            }))
    }

    /// Whether the surface is closed in `v` (`BSplineSurface.isVClosed`).
    fn isVClosed(&self) -> PyResult<bool> {
        let geometry = self.require_geometry()?;
        Ok(!geometry.poles.is_empty()
            && geometry
                .poles
                .iter()
                .all(|row| {
                    row.first().zip(row.last()).is_some_and(|(first, last)| {
                        first.0.distance(&last.0) <= ferrocad_curves::TOLERANCE
                    })
                }))
    }

    /// Whether the surface carries non-unit weights (`BSplineSurface.isURational`).
    fn isURational(&self) -> PyResult<bool> {
        Ok(self
            .require_geometry()?
            .poles
            .iter()
            .flatten()
            .any(|(_, weight)| (*weight - 1.0).abs() > 1e-12))
    }

    fn isVRational(&self) -> PyResult<bool> {
        self.isURational()
    }

    /// The poles as a `[u][v]` grid of vectors (`BSplineSurface.getPoles`).
    fn getPoles(&self, py: Python<'_>) -> PyResult<Vec<Vec<PyObject>>> {
        let geometry = self.require_geometry()?;
        geometry
            .poles
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(point, _)| curves::py_vector(py, *point))
                    .collect()
            })
            .collect()
    }

    /// The pole at 1-based `(u, v)` (`BSplineSurface.getPole`).
    fn getPole(&self, py: Python<'_>, u: i32, v: i32) -> PyResult<PyObject> {
        let geometry = self.require_geometry()?;
        let point = pole_at(geometry, u, v)?.0;
        curves::py_vector(py, point)
    }

    /// The weights as a `[u][v]` grid (`BSplineSurface.getWeights`).
    fn getWeights(&self) -> PyResult<Vec<Vec<f64>>> {
        let geometry = self.require_geometry()?;
        Ok(geometry
            .poles
            .iter()
            .map(|row| row.iter().map(|(_, weight)| *weight).collect())
            .collect())
    }

    /// The weight at 1-based `(u, v)` (`BSplineSurface.getWeight`).
    fn getWeight(&self, u: i32, v: i32) -> PyResult<f64> {
        Ok(pole_at(self.require_geometry()?, u, v)?.1)
    }

    /// `(poles, weights)` in one call (`BSplineSurface.getPolesAndWeights`).
    fn getPolesAndWeights(
        &self,
        py: Python<'_>,
    ) -> PyResult<(Vec<Vec<PyObject>>, Vec<Vec<f64>>)> {
        Ok((self.getPoles(py)?, self.getWeights()?))
    }

    /// The `index`-th distinct U knot, 1-based (`BSplineSurface.getUKnot`).
    fn getUKnot(&self, index: i32) -> PyResult<f64> {
        let geometry = self.require_geometry()?;
        knot_at(&geometry.u_knots, index)
    }

    fn getVKnot(&self, index: i32) -> PyResult<f64> {
        let geometry = self.require_geometry()?;
        knot_at(&geometry.v_knots, index)
    }

    /// The distinct U knots (`BSplineSurface.getUKnots`).
    fn getUKnots(&self) -> PyResult<Vec<f64>> {
        Ok(self
            .require_geometry()?
            .u_knots
            .iter()
            .map(|(knot, _)| *knot)
            .collect())
    }

    /// The distinct V knots (`BSplineSurface.getVKnots`).
    fn getVKnots(&self) -> PyResult<Vec<f64>> {
        Ok(self
            .require_geometry()?
            .v_knots
            .iter()
            .map(|(knot, _)| *knot)
            .collect())
    }

    /// The U knot multiplicities (`BSplineSurface.getUMultiplicities`).
    fn getUMultiplicities(&self) -> PyResult<Vec<i32>> {
        Ok(self
            .require_geometry()?
            .u_knots
            .iter()
            .map(|(_, multiplicity)| *multiplicity)
            .collect())
    }

    fn getVMultiplicities(&self) -> PyResult<Vec<i32>> {
        Ok(self
            .require_geometry()?
            .v_knots
            .iter()
            .map(|(_, multiplicity)| *multiplicity)
            .collect())
    }

    /// The multiplicity of the `index`-th U knot (`BSplineSurface.getUMultiplicity`).
    fn getUMultiplicity(&self, index: i32) -> PyResult<i32> {
        let geometry = self.require_geometry()?;
        knot_multiplicity(&geometry.u_knots, index)
    }

    fn getVMultiplicity(&self, index: i32) -> PyResult<i32> {
        let geometry = self.require_geometry()?;
        knot_multiplicity(&geometry.v_knots, index)
    }

    /// Whether `other` describes the same surface (`Geometry.isSame(other, tol,
    /// angulartol)`). Upstream takes both tolerances; they widen the pole
    /// comparison, everything else (degrees, knots, periodicity) must match.
    #[pyo3(signature = (other, tol = None, angulartol = None))]
    fn isSame(
        &self,
        other: &Bound<'_, PyAny>,
        tol: Option<f64>,
        angulartol: Option<f64>,
    ) -> bool {
        let _ = angulartol;
        let Ok(other) = other.downcast::<Self>() else {
            return false;
        };
        let other = other.borrow();
        if self.same_geometry(&other) {
            return true;
        }
        match (tol, self.surface.as_ref(), other.surface.as_ref()) {
            (Some(tol), Some(first), Some(second)) => {
                first.u_degree == second.u_degree
                    && first.v_degree == second.v_degree
                    && first.u_periodic == second.u_periodic
                    && first.v_periodic == second.v_periodic
                    && first.u_knots == second.u_knots
                    && first.v_knots == second.v_knots
                    && first.poles.len() == second.poles.len()
                    && first.poles.iter().zip(&second.poles).all(|(a, b)| {
                        a.len() == b.len()
                            && a.iter().zip(b).all(|((point, weight), (other, other_weight))| {
                                point.distance(other) <= tol && (weight - other_weight).abs() <= tol
                            })
                    })
            }
            _ => false,
        }
    }

    /// Whether `other` describes the same surface (`Geometry.isEqual`).
    fn isEqual(&self, other: &Bound<'_, PyAny>) -> bool {
        self.isSame(other, None, None)
    }

    /// `(u1, u2, v1, v2)` — the current parameter bounds.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        let u = self.u_bounds.unwrap_or_else(|| self.knot_bounds(&self.u_knots));
        let v = self.v_bounds.unwrap_or_else(|| self.knot_bounds(&self.v_knots));
        (u.0, u.1, v.0, v.1)
    }

    #[getter]
    fn UKnotSequence(&self) -> Vec<f64> {
        self.u_knots.clone()
    }

    #[getter]
    fn VKnotSequence(&self) -> Vec<f64> {
        self.v_knots.clone()
    }

    /// Linearly rescale both knot vectors to the requested bounds.
    fn scaleKnotsToBounds(&mut self, u0: f64, u1: f64, v0: f64, v1: f64) {
        scale_knots(&mut self.u_knots, u0, u1);
        scale_knots(&mut self.v_knots, v0, v1);
        if let Some(surface) = &mut self.surface {
            rescale_knots(&mut surface.u_knots, u0, u1);
            rescale_knots(&mut surface.v_knots, v0, v1);
            surface.u_bounds = (u0, u1);
            surface.v_bounds = (v0, v1);
        }
        self.u_bounds = Some((u0, u1));
        self.v_bounds = Some((v0, v1));
    }

    fn __repr__(&self) -> String {
        "<BSplineSurface object>".to_string()
    }
}

impl PyBSplineSurface {
    fn knot_bounds(&self, knots: &[f64]) -> (f64, f64) {
        (
            knots.first().copied().unwrap_or(0.0),
            knots.last().copied().unwrap_or(0.0),
        )
    }

    /// Whether the two surfaces carry the same description: the control-point
    /// geometry when both have it, otherwise the recorded knot/pole data.
    fn same_geometry(&self, other: &Self) -> bool {
        match (self.surface.as_ref(), other.surface.as_ref()) {
            (Some(first), Some(second)) => first == second,
            (None, None) => {
                self.u_knots == other.u_knots
                    && self.v_knots == other.v_knots
                    && self.points == other.points
                    && self.u_periodic == other.u_periodic
                    && self.v_periodic == other.v_periodic
                    && self.u_bounds == other.u_bounds
                    && self.v_bounds == other.v_bounds
            }
            _ => false,
        }
    }
}

/// The 1-based `(u, v)` pole and weight of a surface.
fn pole_at(
    geometry: &ferrocad_occt::part_ops::BsplineSurface,
    u: i32,
    v: i32,
) -> PyResult<(Vector3, f64)> {
    if u < 1 || v < 1 {
        return Err(PyIndexError::new_err("pole indices are 1-based"));
    }
    geometry
        .poles
        .get(u as usize - 1)
        .and_then(|row| row.get(v as usize - 1))
        .copied()
        .ok_or_else(|| PyIndexError::new_err("pole index out of range"))
}

fn knot_at(knots: &[(f64, i32)], index: i32) -> PyResult<f64> {
    if index < 1 {
        return Err(PyIndexError::new_err("knot indices are 1-based"));
    }
    knots
        .get(index as usize - 1)
        .map(|(knot, _)| *knot)
        .ok_or_else(|| PyIndexError::new_err("knot index out of range"))
}

fn knot_multiplicity(knots: &[(f64, i32)], index: i32) -> PyResult<i32> {
    if index < 1 {
        return Err(PyIndexError::new_err("knot indices are 1-based"));
    }
    knots
        .get(index as usize - 1)
        .map(|(_, multiplicity)| *multiplicity)
        .ok_or_else(|| PyIndexError::new_err("knot index out of range"))
}

/// Whether a row/column pair of `points` closes within the linear tolerance.

/// A uniform knot vector on `[0, 1]` for `count` knots (upstream's default).
fn uniform_knots(count: usize) -> Vec<f64> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![0.0];
    }
    (0..count)
        .map(|index| index as f64 / (count - 1) as f64)
        .collect()
}

/// Rescale a knots-with-multiplicities list onto new bounds.
fn rescale_knots(knots: &mut [(f64, i32)], first: f64, last: f64) {
    let Some(&(old_first, _)) = knots.first() else {
        return;
    };
    let Some(&(old_last, _)) = knots.last() else {
        return;
    };
    let span = old_last - old_first;
    if span == 0.0 {
        return;
    }
    for (knot, _) in knots.iter_mut() {
        *knot = first + (*knot - old_first) * (last - first) / span;
    }
}

fn scale_knots(knots: &mut [f64], new_first: f64, new_last: f64) {
    let Some(&first) = knots.first() else {
        return;
    };
    let Some(&last) = knots.last() else {
        return;
    };
    let span = last - first;
    if span == 0.0 {
        return;
    }
    for knot in knots.iter_mut() {
        *knot = new_first + (*knot - first) * (new_last - new_first) / span;
    }
}

/// `Part.Precision` — the kernel tolerances the Part module exposes.
///
/// `confusion` is `Precision::Confusion` (1e-7), `angular` is
/// `Precision::Angular` (1e-12) and `infinite` is `Precision::Infinite` (1e100).
#[pyfunction]
fn confusion() -> f64 {
    CONFUSION
}

#[pyfunction]
fn angular() -> f64 {
    1e-12
}

#[pyfunction]
fn infinite() -> f64 {
    1e100
}

#[pyfunction]
fn isInfinite(value: f64) -> bool {
    value.is_infinite() || value.abs() >= 1e100
}

/// A planar face's surface (`Face.Surface`), with the elementary surface API the
/// upstream tests use.
#[pyclass(name = "PlaneSurface", module = "Part")]
#[derive(Clone)]
pub struct PyPlaneSurface {
    normal: Vector3,
}

impl PyPlaneSurface {
    /// The fallback planar surface (kept for callers that only know a normal);
    /// `Face.Surface` itself now returns a fully classified `Part.Plane`.
    #[allow(dead_code)]
    pub(crate) fn new(normal: Vector3) -> Self {
        Self { normal }
    }
}

#[pymethods]
impl PyPlaneSurface {
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::GeomPlane"
    }

    /// The surface normal, constant over a plane.
    fn normal(&self, py: Python<'_>, _u: f64, _v: f64) -> PyResult<PyObject> {
        curves::py_vector(py, self.normal)
    }

    fn __repr__(&self) -> String {
        format!("Part.Plane(normal={:?})", self.normal)
    }
}

// ---------------------------------------------------------------------------
// Sketcher: `Sketcher::SketchObject` (geometry, constraints, no solver)
// ---------------------------------------------------------------------------
//
// The Part tests need four things from a sketch: `addGeometry` (so a sketch
// participates in a document at all), `Shape` (a wire/compound of the geometry
// edges, moved by `Placement`), `InternalShape` (the bounded faces built from
// those wires when `MakeInternals` is set) and — since P4 SK3 — a constraint
// model that a Draft `make_sketch` script can drive.
//
// The model is FerroCAD's, not upstream's C++ solver front end. A constraint is a
/// record in a `ConstraintData` property (a `StringList`, so it persists through
/// the native and `.FCStd` serializers without a new value type): kind, geometry
/// and point references, datum. `addConstraint`/`delConstraint`/`setDatum`/`getDatum`
/// edit that list, `Constraints` decodes it into `Sketcher.Constraint` objects,
/// `solve` checks the references, and `RedundantConstraints` is empty because there
/// is no rank analysis to report. **No numerical solve happens** — geometry is
/// authored, not moved — which is the documented approximation for this slice
/// (P4 SK2 binds PlanGCS).
//
// The encoding is `kind;first=0;firstPos=2;second=1;secondPos=1` with a trailing
// `;name=…` when named; see `docs/developer/workbenches/sketch-constraints.md`.
//
// The geometry list is stored as the edges each geometry object's `toShape()`
// returns (`Part::PropertyTopoShapeList`), not as upstream's `Geom...` objects:
// FerroCAD has no geometry-list property type yet. The kernel still builds every
// edge and every wire/face, so nothing here is synthesized. Constraints address
// the `Geometry` list by index; construction geometry is stored separately and is
// not addressable by a constraint until SK1's stable geometry identity lands.

/// `Sketcher::SketchObject`.
pub const SKETCH_TYPE: &str = "Sketcher::SketchObject";
/// `PartDesign::Body` (the Part composition root owns its shape half).
pub const BODY_TYPE: &str = "PartDesign::Body";
/// `PartDesign::AdditiveBox`: the minimal shape source a `PartDesign::Body`
/// needs. Upstream it is `Part::Box` plus the PartDesign additive extension; the
/// dimensions and the derived `Shape` are the same.
pub const ADDITIVE_BOX_TYPE: &str = "PartDesign::AdditiveBox";
/// `Part::Mirroring` (`FeatureMirroring.cpp`).
pub const MIRRORING_TYPE: &str = "Part::Mirroring";

const GEOMETRY: &str = "Geometry";
const CONSTRUCTION_GEOMETRY: &str = "ConstructionGeometry";
const INTERNAL_SHAPE: &str = "InternalShape";
/// The hidden property that stores the constraints, one encoded record per
/// entry. It is deliberately not called `Constraints`: that name is the Python
/// surface (a getter that decodes these records into `Sketcher.Constraint`
/// objects), and a class attribute of that name would shadow a property of the
/// same name. Upstream has no `Constraints` *property* either — the list is
/// computed from the solver state.
const CONSTRAINT_DATA: &str = "ConstraintData";
/// The constraints `detectMissing…` found but has not committed yet. Upstream
/// keeps them in C++ members; a hidden property keeps them inspectable and
/// survives a save/restore without inventing per-object Rust state.
const MISSING_POINT_ON_POINT: &str = "MissingPointOnPointConstraints";
const MISSING_VERTICAL_HORIZONTAL: &str = "MissingVerticalHorizontalConstraints";

/// The constraint kinds the sketch model stores. Upstream has a few more (the
/// `InternalAlignment` family and `SnellsLaw`); a kind outside this list is
/// refused by `addConstraint` rather than stored as an opaque string that no
/// later reader understands.
const CONSTRAINT_KINDS: &[&str] = &[
    "Coincident",
    "PointOnObject",
    "Vertical",
    "Horizontal",
    "Parallel",
    "Perpendicular",
    "Tangent",
    "TangentViaPoint",
    "Equal",
    "Symmetric",
    "Block",
    "Distance",
    "DistanceX",
    "DistanceY",
    "Radius",
    "Diameter",
    "Angle",
    "Weight",
];

/// The kinds whose datum (`setDatum`/`getDatum`) is meaningful.
const DIMENSIONAL_KINDS: &[&str] = &[
    "Distance",
    "DistanceX",
    "DistanceY",
    "Radius",
    "Diameter",
    "Angle",
    "Weight",
];

/// Upstream's `PointPos` numbering, as the API carries it.
const POINT_POS_NONE: i64 = 0;
const POINT_POS_START: i64 = 1;
const POINT_POS_END: i64 = 2;

/// How close two edge endpoints must be to be joined into one wire (OCCT's
/// `Precision::Confusion`, loosened slightly for coordinates reached through a
/// curve evaluation).
const SKETCH_TOLERANCE: f64 = 1e-6;

/// Store an output property, marking the object invalid when the rebuild failed.
fn store_named(doc: &mut Document, id: ObjectId, name: &str, result: Result<Shape, String>) {
    match result {
        Ok(shape) => {
            doc.set_property_raw(
                id,
                name,
                make_shape_property(ShapeProperty::with_lineage(shape, Lineage::default())),
            );
        }
        Err(_) => {
            doc.mark_invalid(id);
            doc.set_property_raw(id, name, output_shape());
        }
    }
}

/// A vector-valued property. A Python tuple assigned to a `PropertyVector` is
/// stored as a float list by the generic converter, and upstream accepts a
/// 3-tuple there, so both spellings are read.
fn vector_like_property(doc: &Document, id: ObjectId, name: &str) -> Option<Vector3> {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Vector(value)) => Some(*value),
        Some(Property::FloatList(values)) if values.len() == 3 => {
            Some(Vector3::new(values[0], values[1], values[2]))
        }
        Some(Property::IntegerList(values)) if values.len() == 3 => Some(Vector3::new(
            values[0] as f64,
            values[1] as f64,
            values[2] as f64,
        )),
        _ => None,
    }
}

/// The shape held by an object's `Shape` property.
fn object_shape(doc: &Document, id: ObjectId) -> Option<Shape> {
    doc.object(id)
        .and_then(|object| object.properties.get(SHAPE))
        .and_then(shape_of)
        .and_then(|property| property.shape())
}

/// The stored edges of a sketch's geometry list.
fn sketch_geometry(doc: &Document, id: ObjectId, name: &str) -> Vec<Shape> {
    doc.object(id)
        .and_then(|object| object.properties.get(name))
        .and_then(shape_list_of)
        .map(ShapeListProperty::shapes)
        .unwrap_or_default()
}

/// The two endpoints of an open edge. `None` for a closed edge (a full circle or
/// ellipse has one seam vertex, not two).
fn edge_endpoints(edge: &Shape) -> Option<(Vector3, Vector3)> {
    let backend = ferrocad_part::backend();
    let mut points = backend
        .elements(edge, ElementKind::Vertex)
        .into_iter()
        .filter_map(|element| backend.vertex_point(&element.shape));
    let start = points.next()?;
    let end = points.next()?;
    Some((start, end))
}

fn endpoints_meet(a: &Vector3, b: &Vector3) -> bool {
    a.sub(b).length() <= SKETCH_TOLERANCE
}

/// Join edges that share endpoints into wires (upstream's `WireJoiner`), one
/// wire per connected chain. A closed edge is a wire on its own.
fn chain_edges(edges: &[Shape]) -> Result<Vec<Shape>, String> {
    let backend = ferrocad_part::backend();
    let mut used = vec![false; edges.len()];
    let mut wires = Vec::new();
    for index in 0..edges.len() {
        if used[index] {
            continue;
        }
        used[index] = true;
        let mut chain = vec![edges[index].clone()];
        if let Some((_, mut end)) = edge_endpoints(&edges[index]) {
            loop {
                let mut next = None;
                for candidate in 0..edges.len() {
                    if used[candidate] {
                        continue;
                    }
                    let Some((first, second)) = edge_endpoints(&edges[candidate]) else {
                        continue;
                    };
                    if endpoints_meet(&end, &first) {
                        next = Some((candidate, second));
                        break;
                    }
                    if endpoints_meet(&end, &second) {
                        next = Some((candidate, first));
                        break;
                    }
                }
                let Some((candidate, far)) = next else {
                    break;
                };
                used[candidate] = true;
                chain.push(edges[candidate].clone());
                end = far;
            }
        }
        wires.push(backend.make_wire(&chain).map_err(|error| error.to_string())?);
    }
    Ok(wires)
}

/// The sketch wires moved by the sketch's `Placement`.
fn placed_shape(wires: Vec<Shape>, placement: Option<Placement>) -> Result<Vec<Shape>, String> {
    let Some(placement) = placement else {
        return Ok(wires);
    };
    if placement.is_same(&Placement::identity(), 1e-12) {
        return Ok(wires);
    }
    let backend = ferrocad_part::backend();
    wires
        .into_iter()
        .map(|wire| {
            backend
                .place(&wire, &placement)
                .map_err(|error| error.to_string())
        })
        .collect()
}

/// `Shape`: a compound of the geometry wires (or edges when a wire cannot be
/// joined), as upstream's `SketchObject::buildShape`.
fn sketch_shape(edges: &[Shape], placement: Option<Placement>) -> Result<Shape, String> {
    let wires = placed_shape(chain_edges(edges)?, placement)?;
    ferrocad_part::backend()
        .make_compound(&wires)
        .map_err(|error| error.to_string())
}

/// `InternalShape`: the bounded faces of the sketch wires (`buildInternals`).
/// `make_faces` is the kernel's `FaceMakerBuildFace`: closed wires bound regions
/// and open wire chains split them, which is how the two connecting segments of
/// the upstream annulus case produce two faces.
fn sketch_internal_shape(edges: &[Shape], placement: Option<Placement>) -> Result<Shape, String> {
    let wires = placed_shape(chain_edges(edges)?, placement)?;
    ferrocad_part::backend()
        .make_faces(&wires)
        .map_err(|error| error.to_string())
}

fn sketch_placement(doc: &Document, id: ObjectId) -> Option<Placement> {
    match doc.object(id).and_then(|object| object.properties.get("Placement")) {
        Some(Property::Placement(placement)) => Some(*placement),
        _ => None,
    }
}

// --- The constraint model -------------------------------------------------

/// One sketch constraint as the `Constraints` property stores it.
///
/// Field names mirror upstream's `Sketcher.Constraint` attributes so the
/// `Sketcher.Constraint` round trip is exact. `driving`/`active` default to
/// `true`, and every other field to the value upstream gives an unset one, so an
/// omitted key in the encoding decodes to the same record.
#[derive(Clone, Debug, PartialEq)]
struct SketchConstraint {
    kind: String,
    first: i64,
    first_pos: i64,
    second: i64,
    second_pos: i64,
    third: i64,
    third_pos: i64,
    value: f64,
    name: String,
    driving: bool,
    active: bool,
}

impl SketchConstraint {
    fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            first: 0,
            first_pos: POINT_POS_NONE,
            second: 0,
            second_pos: POINT_POS_NONE,
            third: 0,
            third_pos: POINT_POS_NONE,
            value: 0.0,
            name: String::new(),
            driving: true,
            active: true,
        }
    }

    /// The `;`-separated record. `name` is emitted last so a name may contain
    /// `;` or `=`; every other key is `<key>=<value>`.
    fn encode(&self) -> String {
        let mut out = format!("{};first={}", self.kind, self.first);
        if self.first_pos != POINT_POS_NONE {
            out.push_str(&format!(";firstPos={}", self.first_pos));
        }
        if self.second != 0 || self.second_pos != POINT_POS_NONE {
            out.push_str(&format!(";second={}", self.second));
        }
        if self.second_pos != POINT_POS_NONE {
            out.push_str(&format!(";secondPos={}", self.second_pos));
        }
        if self.third != 0 || self.third_pos != POINT_POS_NONE {
            out.push_str(&format!(";third={}", self.third));
        }
        if self.third_pos != POINT_POS_NONE {
            out.push_str(&format!(";thirdPos={}", self.third_pos));
        }
        if self.value != 0.0 {
            out.push_str(&format!(";value={}", self.value));
        }
        if !self.driving {
            out.push_str(";driving=0");
        }
        if !self.active {
            out.push_str(";active=0");
        }
        if !self.name.is_empty() {
            out.push_str(";name=");
            out.push_str(&self.name);
        }
        out
    }

    /// Decode one record; `None` for a record this model did not write.
    fn decode(entry: &str) -> Option<Self> {
        let (body, name) = match entry.find(";name=") {
            Some(at) => (&entry[..at], Some(&entry[at + ";name=".len()..])),
            None => (entry, None),
        };
        let mut fields = body.split(';');
        let kind = fields.next()?;
        if !CONSTRAINT_KINDS.contains(&kind) {
            return None;
        }
        let mut constraint = Self::new(kind);
        for field in fields {
            let (key, raw) = field.split_once('=')?;
            match key {
                "first" => constraint.first = raw.parse().ok()?,
                "firstPos" => constraint.first_pos = raw.parse().ok()?,
                "second" => constraint.second = raw.parse().ok()?,
                "secondPos" => constraint.second_pos = raw.parse().ok()?,
                "third" => constraint.third = raw.parse().ok()?,
                "thirdPos" => constraint.third_pos = raw.parse().ok()?,
                "value" => constraint.value = raw.parse().ok()?,
                "driving" => constraint.driving = raw != "0",
                "active" => constraint.active = raw != "0",
                _ => return None,
            }
        }
        if let Some(name) = name {
            constraint.name = name.to_string();
        }
        Some(constraint)
    }
}

/// Refuse a method on an object that is not a sketch (the descriptor already
/// gates this; a direct `wrap_pyfunction` call reaches here too).
fn require_sketch(object: &Bound<'_, PyAny>, method: &str) -> PyResult<()> {
    let type_id: String = object.getattr("TypeId")?.extract()?;
    if type_id != SKETCH_TYPE {
        return Err(PyAttributeError::new_err(format!(
            "'{type_id}' object has no attribute '{method}'"
        )));
    }
    Ok(())
}

/// The stored string list of a property (empty when the property is absent).
fn sketch_string_list(object: &Bound<'_, PyAny>, name: &str) -> PyResult<Vec<String>> {
    let value = object.getattr(name)?;
    let mut out = Vec::new();
    for item in value.try_iter()? {
        out.push(item?.extract::<String>()?);
    }
    Ok(out)
}

fn store_sketch_string_list(
    object: &Bound<'_, PyAny>,
    name: &str,
    values: &[String],
) -> PyResult<()> {
    let list = PyList::new(object.py(), values.iter().map(String::as_str))?;
    object.setattr(name, list)
}

/// The committed constraints, decoded. An entry this model cannot decode is a
/// `ValueError` naming it: it means something wrote the property behind
/// `addConstraint`'s back.
fn sketch_constraints(object: &Bound<'_, PyAny>) -> PyResult<Vec<SketchConstraint>> {
    sketch_string_list(object, CONSTRAINT_DATA)?
        .iter()
        .map(|entry| {
            SketchConstraint::decode(entry).ok_or_else(|| {
                PyValueError::new_err(format!(
                    "{CONSTRAINT_DATA} holds an entry this sketch model cannot read: {entry:?}"
                ))
            })
        })
        .collect()
}

fn store_sketch_constraints(
    object: &Bound<'_, PyAny>,
    constraints: &[SketchConstraint],
) -> PyResult<()> {
    let encoded: Vec<String> = constraints.iter().map(SketchConstraint::encode).collect();
    store_sketch_string_list(object, CONSTRAINT_DATA, &encoded)
}

/// One `addConstraint` argument: a `Sketcher.Constraint` (or anything with its
/// attributes), or a sequence of them.
fn constraint_arguments(value: &Bound<'_, PyAny>) -> PyResult<Vec<SketchConstraint>> {
    if value.downcast::<pyo3::types::PyString>().is_ok() {
        return Err(PyTypeError::new_err(
            "addConstraint expects a Sketcher.Constraint, not a string",
        ));
    }
    if let Ok(items) = value.try_iter() {
        let mut constraints = Vec::new();
        for item in items {
            constraints.push(constraint_argument(&item?)?);
        }
        return Ok(constraints);
    }
    Ok(vec![constraint_argument(value)?])
}

fn constraint_argument(value: &Bound<'_, PyAny>) -> PyResult<SketchConstraint> {
    let kind: String = value.getattr("Type").and_then(|t| t.extract()).map_err(|_| {
        PyTypeError::new_err(
            "addConstraint expects a Sketcher.Constraint (an object with a string `Type`)",
        )
    })?;
    if !CONSTRAINT_KINDS.contains(&kind.as_str()) {
        return Err(PyValueError::new_err(format!(
            "unsupported sketch constraint type {kind:?}; this sketch model stores {}",
            CONSTRAINT_KINDS.join(", ")
        )));
    }
    let mut constraint = SketchConstraint::new(&kind);
    constraint.first = int_attribute(value, "First")?;
    constraint.first_pos = int_attribute(value, "FirstPos")?;
    constraint.second = int_attribute(value, "Second")?;
    constraint.second_pos = int_attribute(value, "SecondPos")?;
    constraint.third = int_attribute(value, "Third")?;
    constraint.third_pos = int_attribute(value, "ThirdPos")?;
    constraint.value = match value.getattr("Value") {
        Ok(item) if !item.is_none() => item.extract().map_err(|_| {
            PyTypeError::new_err("a sketch constraint's Value must be a number")
        })?,
        _ => 0.0,
    };
    constraint.name = match value.getattr("Name") {
        Ok(item) if !item.is_none() => item.extract().map_err(|_| {
            PyTypeError::new_err("a sketch constraint's Name must be a string")
        })?,
        _ => String::new(),
    };
    constraint.driving = bool_attribute(value, "Driving", true)?;
    constraint.active = bool_attribute(value, "IsActive", true)?;
    Ok(constraint)
}

fn int_attribute(value: &Bound<'_, PyAny>, name: &str) -> PyResult<i64> {
    match value.getattr(name) {
        Ok(item) if !item.is_none() => item
            .extract()
            .map_err(|_| PyTypeError::new_err(format!("a sketch constraint's {name} must be an int"))),
        _ => Ok(0),
    }
}

fn bool_attribute(value: &Bound<'_, PyAny>, name: &str, default: bool) -> PyResult<bool> {
    match value.getattr(name) {
        Ok(item) if !item.is_none() => item
            .extract()
            .map_err(|_| PyTypeError::new_err(format!("a sketch constraint's {name} must be a bool"))),
        _ => Ok(default),
    }
}

/// Whether two constraints say the same thing, ignoring the order of a
/// two-point reference (`(0,end)-(1,start)` == `(1,start)-(0,end)`).
fn same_constraint(a: &SketchConstraint, b: &SketchConstraint) -> bool {
    if a.kind != b.kind {
        return false;
    }
    let forward = a.first == b.first
        && a.first_pos == b.first_pos
        && a.second == b.second
        && a.second_pos == b.second_pos
        && a.third == b.third
        && a.third_pos == b.third_pos;
    let backward = a.first == b.second
        && a.first_pos == b.second_pos
        && a.second == b.first
        && a.second_pos == b.first_pos
        && a.third == b.third
        && a.third_pos == b.third_pos;
    forward || backward
}

/// The number of geometry edges a sketch holds (upstream's `GeometryCount`).
/// Construction geometry is stored in its own list and is not counted: a
/// constraint index addresses the `Geometry` list until SK1's stable identity
/// lands.
fn sketch_geometry_len(object: &Bound<'_, PyAny>) -> PyResult<usize> {
    Ok(object.getattr(GEOMETRY)?.len()?)
}

/// Whether an edge is a straight segment (the only kind the vertical/horizontal
/// detection can classify).
fn is_straight_edge(edge: &Shape) -> bool {
    matches!(
        ferrocad_part::backend().curve_of(edge),
        Ok(EdgeCurve::Line(_) | EdgeCurve::LineSegment(_))
    )
}

/// The pending detections, decoded; `[]` when the property is empty.
fn pending_constraints(object: &Bound<'_, PyAny>, name: &str) -> PyResult<Vec<SketchConstraint>> {
    sketch_string_list(object, name)?
        .iter()
        .map(|entry| {
            SketchConstraint::decode(entry).ok_or_else(|| {
                PyValueError::new_err(format!("{name} holds an unreadable entry: {entry:?}"))
            })
        })
        .collect()
}

fn store_pending_constraints(
    object: &Bound<'_, PyAny>,
    name: &str,
    constraints: &[SketchConstraint],
) -> PyResult<()> {
    let encoded: Vec<String> = constraints.iter().map(SketchConstraint::encode).collect();
    store_sketch_string_list(object, name, &encoded)
}

/// Commit the pending constraints of `pending_name` into `Constraints`.
///
/// `include_second` is upstream's flag. Detection here already emits each
/// `(point, point)` pair explicitly, so when the flag is `false` this keeps one
/// coincident/axis constraint per *first* reference — which is the property the
/// flag names — instead of skipping the second half of every pair.
fn commit_pending(
    object: &Bound<'_, PyAny>,
    pending_name: &str,
    include_second: bool,
) -> PyResult<usize> {
    let pending = pending_constraints(object, pending_name)?;
    let mut constraints = sketch_constraints(object)?;
    let mut added = 0;
    for candidate in pending {
        let duplicate = constraints.iter().any(|have| same_constraint(have, &candidate));
        let already_referenced = !include_second
            && constraints
                .iter()
                .any(|have| have.kind == candidate.kind && have.first == candidate.first);
        if duplicate || already_referenced {
            continue;
        }
        constraints.push(candidate);
        added += 1;
    }
    store_sketch_constraints(object, &constraints)?;
    store_pending_constraints(object, pending_name, &[])?;
    Ok(added)
}

/// Build `Sketcher.Constraint` objects from the stored records.
fn constraint_objects(
    py: Python<'_>,
    constraints: &[SketchConstraint],
) -> PyResult<PyObject> {
    let module = py.import("Sketcher").map_err(|_| {
        PyRuntimeError::new_err(
            "Sketcher.Constraint is unavailable: the Sketcher module is not importable",
        )
    })?;
    let class = module.getattr("Constraint")?;
    let mut items = Vec::with_capacity(constraints.len());
    for constraint in constraints {
        let kwargs = PyDict::new(py);
        kwargs.set_item("First", constraint.first)?;
        kwargs.set_item("FirstPos", constraint.first_pos)?;
        kwargs.set_item("Second", constraint.second)?;
        kwargs.set_item("SecondPos", constraint.second_pos)?;
        kwargs.set_item("Third", constraint.third)?;
        kwargs.set_item("ThirdPos", constraint.third_pos)?;
        kwargs.set_item("Value", constraint.value)?;
        kwargs.set_item("Name", constraint.name.as_str())?;
        kwargs.set_item("Driving", constraint.driving)?;
        kwargs.set_item("IsActive", constraint.active)?;
        items.push(class.call((constraint.kind.as_str(),), Some(&kwargs))?);
    }
    Ok(PyList::new(py, items)?.into_any().unbind())
}

struct SketchObject;

impl ObjectType for SketchObject {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            // Attachment is not implemented: `MapMode` is carried so scripts that
            // set it (upstream's `MapMode = "Deactivated"`) work, and nothing
            // reads it back.
            (
                "MapMode".to_string(),
                Property::String("Deactivated".to_string()),
                prop_status::NONE,
            ),
            (
                "MakeInternals".to_string(),
                Property::Bool(false),
                prop_status::NONE,
            ),
            (
                GEOMETRY.to_string(),
                make_shape_list_property(ShapeListProperty::empty()),
                prop_status::HIDDEN,
            ),
            (
                CONSTRUCTION_GEOMETRY.to_string(),
                make_shape_list_property(ShapeListProperty::empty()),
                prop_status::HIDDEN,
            ),
            (
                CONSTRAINT_DATA.to_string(),
                Property::StringList(Vec::new()),
                prop_status::HIDDEN,
            ),
            (
                MISSING_POINT_ON_POINT.to_string(),
                Property::StringList(Vec::new()),
                prop_status::HIDDEN,
            ),
            (
                MISSING_VERTICAL_HORIZONTAL.to_string(),
                Property::StringList(Vec::new()),
                prop_status::HIDDEN,
            ),
            output_property(),
            (
                INTERNAL_SHAPE.to_string(),
                output_shape(),
                prop_status::OUTPUT,
            ),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Sketch".to_string(), format!("Sketch {name}")))
    }

    /// A sketch supports a body but never becomes its `Tip`.
    fn body_membership(&self) -> BodyMembership {
        BodyMembership::Support
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let geometry = sketch_geometry(doc, id, GEOMETRY);
        let placement = sketch_placement(doc, id);
        if geometry.is_empty() {
            doc.set_property_raw(id, SHAPE, output_shape());
            doc.set_property_raw(id, INTERNAL_SHAPE, output_shape());
            return;
        }
        store_named(doc, id, SHAPE, sketch_shape(&geometry, placement));
        if bool_property(doc, id, "MakeInternals", false) {
            store_named(
                doc,
                id,
                INTERNAL_SHAPE,
                sketch_internal_shape(&geometry, placement),
            );
        } else {
            doc.set_property_raw(id, INTERNAL_SHAPE, output_shape());
        }
    }
}

/// `addGeometry(geometry, construction=False)` for a sketch object.
///
/// A geometry object is anything exposing `toShape()` (`Part.LineSegment`,
/// `Part.Circle`, `Part.Ellipse`, `Part.Point`, …); a sequence of them is
/// accepted too, as upstream's `SketchObjectPy::addGeometry` does. The edges are
/// appended to the sketch's geometry list, which touches the object so the next
/// recompute rebuilds `Shape`.
#[pyfunction]
#[pyo3(signature = (object, geometry, construction = false))]
fn sketch_add_geometry(
    object: &Bound<'_, PyAny>,
    geometry: &Bound<'_, PyAny>,
    construction: bool,
) -> PyResult<usize> {
    let type_id: String = object.getattr("TypeId")?.extract()?;
    if type_id != SKETCH_TYPE {
        return Err(PyAttributeError::new_err(format!(
            "'{type_id}' object has no attribute 'addGeometry'"
        )));
    }
    let added = sketch_geometry_edges(geometry)?;
    let name = if construction {
        CONSTRUCTION_GEOMETRY
    } else {
        GEOMETRY
    };
    let mut shapes = sketch_stored_geometry(object, name)?;
    let first = shapes.len();
    shapes.extend(added.iter().cloned());
    sketch_store_geometry(object, name, &shapes)?;
    Ok(if added.is_empty() {
        first
    } else {
        first + added.len() - 1
    })
}

/// The geometry edges named by one `addGeometry` argument: one geometry object,
/// a `Part.Shape`, or a sequence of either.
fn sketch_geometry_edges(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if value.downcast::<PartShape>().is_ok() {
        return Ok(vec![shape_argument(value)?]);
    }
    if value.downcast::<pyo3::types::PyString>().is_ok() {
        return Err(PyTypeError::new_err(
            "sketch geometry must be a geometry object or a sequence of them",
        ));
    }
    if let Ok(items) = value.try_iter() {
        let mut edges = Vec::new();
        for item in items {
            edges.push(sketch_geometry_edge(&item?)?);
        }
        return Ok(edges);
    }
    Ok(vec![sketch_geometry_edge(value)?])
}

fn sketch_geometry_edge(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let to_shape = value.getattr("toShape").map_err(|_| {
        let name = value
            .get_type()
            .name()
            .map(|name| name.to_string())
            .unwrap_or_else(|_| "?".to_string());
        PyTypeError::new_err(format!("unsupported sketch geometry {name}"))
    })?;
    shape_argument(&to_shape.call0()?)
}

fn sketch_stored_geometry(object: &Bound<'_, PyAny>, name: &str) -> PyResult<Vec<Shape>> {
    let value = object.getattr(name)?;
    let mut shapes = Vec::new();
    for item in value.try_iter()? {
        shapes.push(shape_argument(&item?)?);
    }
    Ok(shapes)
}

fn sketch_store_geometry(object: &Bound<'_, PyAny>, name: &str, shapes: &[Shape]) -> PyResult<()> {
    let py = object.py();
    let items = shapes
        .iter()
        .map(|shape| Py::new(py, PartShape::from_shape(shape.clone())))
        .collect::<PyResult<Vec<_>>>()?;
    object.setattr(name, PyList::new(py, items)?)
}

/// `GeometryCount`: the number of entries in the constraint-addressable geometry
/// list.
#[pyfunction]
fn sketch_geometry_count(object: &Bound<'_, PyAny>) -> PyResult<usize> {
    require_sketch(object, "GeometryCount")?;
    sketch_geometry_len(object)
}

/// `ConstraintCount`: the number of stored constraints.
#[pyfunction]
fn sketch_constraint_count(object: &Bound<'_, PyAny>) -> PyResult<usize> {
    require_sketch(object, "ConstraintCount")?;
    Ok(sketch_string_list(object, CONSTRAINT_DATA)?.len())
}

/// `Constraints`: the stored constraints as `Sketcher.Constraint` objects.
#[pyfunction]
fn sketch_constraint_objects(object: &Bound<'_, PyAny>) -> PyResult<PyObject> {
    require_sketch(object, "Constraints")?;
    let constraints = sketch_constraints(object)?;
    constraint_objects(object.py(), &constraints)
}

/// `RedundantConstraints`: upstream's one-based indices of the redundant set.
///
/// FerroCAD has no solver and therefore no rank analysis, so there is nothing to
/// report and the list is empty rather than invented. A script that filters with
/// this list (Draft's `make_sketch` does) keeps every constraint it added.
#[pyfunction]
fn sketch_redundant_constraints(object: &Bound<'_, PyAny>) -> PyResult<Vec<usize>> {
    require_sketch(object, "RedundantConstraints")?;
    Ok(Vec::new())
}

/// `addConstraint(constraint)`: append one constraint or a sequence of them.
///
/// Returns the index of the last constraint added, or `-1` when the argument was
/// an empty sequence (upstream's return convention).
#[pyfunction]
fn sketch_add_constraint(
    object: &Bound<'_, PyAny>,
    constraint: &Bound<'_, PyAny>,
) -> PyResult<isize> {
    require_sketch(object, "addConstraint")?;
    let added = constraint_arguments(constraint)?;
    if added.is_empty() {
        return Ok(-1);
    }
    let mut constraints = sketch_constraints(object)?;
    let first = constraints.len();
    constraints.extend(added.iter().cloned());
    store_sketch_constraints(object, &constraints)?;
    Ok((first + added.len() - 1) as isize)
}

/// `delConstraint(index)`: drop one constraint by its zero-based index.
#[pyfunction]
fn sketch_del_constraint(object: &Bound<'_, PyAny>, index: usize) -> PyResult<()> {
    require_sketch(object, "delConstraint")?;
    let mut constraints = sketch_constraints(object)?;
    if index >= constraints.len() {
        return Err(PyIndexError::new_err(format!(
            "constraint index {index} is out of range ({n} constraints)",
            n = constraints.len()
        )));
    }
    constraints.remove(index);
    store_sketch_constraints(object, &constraints)
}

/// `setDatum(index, value)`: change a dimensional constraint's datum.
#[pyfunction]
fn sketch_set_datum(object: &Bound<'_, PyAny>, index: usize, value: f64) -> PyResult<()> {
    require_sketch(object, "setDatum")?;
    let mut constraints = sketch_constraints(object)?;
    let count = constraints.len();
    let constraint = constraints.get_mut(index).ok_or_else(|| {
        PyIndexError::new_err(format!(
            "constraint index {index} is out of range ({count} constraints)"
        ))
    })?;
    if !DIMENSIONAL_KINDS.contains(&constraint.kind.as_str()) {
        return Err(PyValueError::new_err(format!(
            "constraint {index} is {:?}, which carries no datum",
            constraint.kind
        )));
    }
    constraint.value = value;
    store_sketch_constraints(object, &constraints)
}

/// `getDatum(index)`: a dimensional constraint's datum.
#[pyfunction]
fn sketch_get_datum(object: &Bound<'_, PyAny>, index: usize) -> PyResult<f64> {
    require_sketch(object, "getDatum")?;
    let constraints = sketch_constraints(object)?;
    let constraint = constraints.get(index).ok_or_else(|| {
        PyIndexError::new_err(format!(
            "constraint index {index} is out of range ({n} constraints)",
            n = constraints.len()
        ))
    })?;
    if !DIMENSIONAL_KINDS.contains(&constraint.kind.as_str()) {
        return Err(PyValueError::new_err(format!(
            "constraint {index} is {:?}, which carries no datum",
            constraint.kind
        )));
    }
    Ok(constraint.value)
}

/// `solve()`: check the model, and report `0` when every constraint names a
/// geometry that exists (upstream's "converged" code) or `-1` when one does not.
///
/// **This does not move geometry.** FerroCAD stores the constraint model and
/// diagnoses its references; the numerical solve is P4 SK2's PlanGCS binding.
/// A sketch whose constraints are not yet satisfied by its geometry therefore
/// still reports `0` — the datum is the authored value, not a solved one.
#[pyfunction]
fn sketch_solve(object: &Bound<'_, PyAny>) -> PyResult<i32> {
    require_sketch(object, "solve")?;
    let count = sketch_geometry_len(object)? as i64;
    for constraint in sketch_constraints(object)? {
        if constraint.first < 0 || constraint.first >= count {
            return Ok(-1);
        }
    }
    Ok(0)
}

/// `detectMissingPointOnPointConstraints(tol)`: find endpoint pairs that
/// coincide but carry no `Coincident` constraint yet, and stage them for
/// `makeMissingPointOnPointCoincident`.
#[pyfunction]
#[pyo3(signature = (object, tolerance = SKETCH_TOLERANCE))]
fn sketch_detect_point_on_point(
    object: &Bound<'_, PyAny>,
    tolerance: f64,
) -> PyResult<usize> {
    require_sketch(object, "detectMissingPointOnPointConstraints")?;
    let edges = sketch_stored_geometry(object, GEOMETRY)?;
    let endpoints: Vec<Option<(Vector3, Vector3)>> =
        edges.iter().map(edge_endpoints).collect();
    let existing = sketch_constraints(object)?;
    let mut found: Vec<SketchConstraint> = Vec::new();
    for i in 0..edges.len() {
        let Some((i_start, i_end)) = endpoints[i] else {
            continue;
        };
        for j in (i + 1)..edges.len() {
            let Some((j_start, j_end)) = endpoints[j] else {
                continue;
            };
            for (first_pos, first_point) in
                [(POINT_POS_START, i_start), (POINT_POS_END, i_end)]
            {
                for (second_pos, second_point) in
                    [(POINT_POS_START, j_start), (POINT_POS_END, j_end)]
                {
                    if first_point.sub(&second_point).length() > tolerance {
                        continue;
                    }
                    let mut candidate = SketchConstraint::new("Coincident");
                    candidate.first = i as i64;
                    candidate.first_pos = first_pos;
                    candidate.second = j as i64;
                    candidate.second_pos = second_pos;
                    let known = existing
                        .iter()
                        .chain(found.iter())
                        .any(|have| same_constraint(have, &candidate));
                    if !known {
                        found.push(candidate);
                    }
                }
            }
        }
    }
    store_pending_constraints(object, MISSING_POINT_ON_POINT, &found)?;
    Ok(found.len())
}

/// `makeMissingPointOnPointCoincident(includeSecond=True)`: commit the staged
/// `Coincident` constraints. Returns how many were added.
#[pyfunction]
#[pyo3(signature = (object, include_second = true))]
fn sketch_make_point_on_point(
    object: &Bound<'_, PyAny>,
    include_second: bool,
) -> PyResult<usize> {
    require_sketch(object, "makeMissingPointOnPointCoincident")?;
    commit_pending(object, MISSING_POINT_ON_POINT, include_second)
}

/// `detectMissingVerticalHorizontalConstraints(tol)`: find straight edges that
/// are already axis-aligned (in sketch coordinates) but carry no `Vertical` /
/// `Horizontal` constraint, and stage them.
#[pyfunction]
#[pyo3(signature = (object, tolerance = SKETCH_TOLERANCE))]
fn sketch_detect_vertical_horizontal(
    object: &Bound<'_, PyAny>,
    tolerance: f64,
) -> PyResult<usize> {
    require_sketch(object, "detectMissingVerticalHorizontalConstraints")?;
    let edges = sketch_stored_geometry(object, GEOMETRY)?;
    let existing = sketch_constraints(object)?;
    let mut found: Vec<SketchConstraint> = Vec::new();
    for (index, edge) in edges.iter().enumerate() {
        let index = index as i64;
        let already = existing.iter().any(|have| {
            have.first == index && (have.kind == "Vertical" || have.kind == "Horizontal")
        });
        if already || !is_straight_edge(edge) {
            continue;
        }
        let Some((start, end)) = edge_endpoints(edge) else {
            continue;
        };
        let dx = (end.x - start.x).abs();
        let dy = (end.y - start.y).abs();
        let kind = if dx <= tolerance {
            if dy <= tolerance {
                // A degenerate edge: neither horizontal nor vertical.
                continue;
            }
            "Vertical"
        } else if dy <= tolerance {
            "Horizontal"
        } else {
            continue;
        };
        let mut candidate = SketchConstraint::new(kind);
        candidate.first = index;
        found.push(candidate);
    }
    store_pending_constraints(object, MISSING_VERTICAL_HORIZONTAL, &found)?;
    Ok(found.len())
}

/// `makeMissingVerticalHorizontal(includeSecond=True)`: commit the staged
/// axis-alignment constraints. Returns how many were added.
#[pyfunction]
#[pyo3(signature = (object, include_second = true))]
fn sketch_make_vertical_horizontal(
    object: &Bound<'_, PyAny>,
    include_second: bool,
) -> PyResult<usize> {
    require_sketch(object, "makeMissingVerticalHorizontal")?;
    commit_pending(object, MISSING_VERTICAL_HORIZONTAL, include_second)
}

/// Install the sketch surface on `ferrocad.DocumentObject` at `Part` import
/// time.
///
/// A document object is the core's `PyDocumentObject`, whose methods live in
/// `ferrocad_py`; a module cannot add `#[pymethods]` to it. The class is a heap
/// type, so descriptors can still be installed on it: each hands a sketch the
/// bound function and raises `AttributeError` for every other type, exactly as
/// if the attribute did not exist. Skipped when `ferrocad` is not importable
/// (a standalone `Part` build has no document object to extend).
fn install_sketch_methods(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    let Ok(ferrocad) = py.import("ferrocad") else {
        return Ok(());
    };
    let Ok(document_object) = ferrocad.getattr("DocumentObject") else {
        return Ok(());
    };
    let globals = PyDict::new(py);
    globals.set_item("_document_object", document_object)?;

    let attributes = PyDict::new(py);
    attributes.set_item("GeometryCount", wrap_pyfunction!(sketch_geometry_count, m)?)?;
    attributes.set_item("ConstraintCount", wrap_pyfunction!(sketch_constraint_count, m)?)?;
    attributes.set_item("Constraints", wrap_pyfunction!(sketch_constraint_objects, m)?)?;
    attributes.set_item(
        "RedundantConstraints",
        wrap_pyfunction!(sketch_redundant_constraints, m)?,
    )?;
    globals.set_item("_SKETCH_ATTRIBUTES", attributes)?;

    let methods = PyDict::new(py);
    methods.set_item("addGeometry", wrap_pyfunction!(sketch_add_geometry, m)?)?;
    methods.set_item("addConstraint", wrap_pyfunction!(sketch_add_constraint, m)?)?;
    methods.set_item("delConstraint", wrap_pyfunction!(sketch_del_constraint, m)?)?;
    methods.set_item("setDatum", wrap_pyfunction!(sketch_set_datum, m)?)?;
    methods.set_item("getDatum", wrap_pyfunction!(sketch_get_datum, m)?)?;
    methods.set_item("solve", wrap_pyfunction!(sketch_solve, m)?)?;
    methods.set_item(
        "detectMissingPointOnPointConstraints",
        wrap_pyfunction!(sketch_detect_point_on_point, m)?,
    )?;
    methods.set_item(
        "makeMissingPointOnPointCoincident",
        wrap_pyfunction!(sketch_make_point_on_point, m)?,
    )?;
    methods.set_item(
        "detectMissingVerticalHorizontalConstraints",
        wrap_pyfunction!(sketch_detect_vertical_horizontal, m)?,
    )?;
    methods.set_item(
        "makeMissingVerticalHorizontal",
        wrap_pyfunction!(sketch_make_vertical_horizontal, m)?,
    )?;
    globals.set_item("_SKETCH_METHODS", methods)?;

    let source = CString::new(SKETCH_METHODS).expect("no NUL");
    py.run(source.as_c_str(), Some(&globals), None)?;
    Ok(())
}

const SKETCH_METHODS: &str = r#"
class _SketchAttribute:
    __slots__ = ("_name", "_getter")

    def __init__(self, name, getter):
        self._name = name
        self._getter = getter

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        _require_sketch(obj, self._name)
        return self._getter(obj)


class _SketchMethod:
    __slots__ = ("_name", "_call")

    def __init__(self, name, call):
        self._name = name
        self._call = call

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        _require_sketch(obj, self._name)
        call = self._call

        def method(*args, **kwargs):
            return call(obj, *args, **kwargs)

        method.__name__ = self._name
        return method


def _require_sketch(obj, name):
    if obj.TypeId != "Sketcher::SketchObject":
        raise AttributeError("'%s' object has no attribute '%s'" % (obj.TypeId, name))


for _name, _getter in _SKETCH_ATTRIBUTES.items():
    setattr(_document_object, _name, _SketchAttribute(_name, _getter))

for _name, _call in _SKETCH_METHODS.items():
    setattr(_document_object, _name, _SketchMethod(_name, _call))
"#;

// ---------------------------------------------------------------------------
// `PartDesign::Body` and its minimal additive box
// ---------------------------------------------------------------------------

/// `PartDesign::Body`: upstream's `Shape` is its `Tip`'s shape (or, before a Tip
/// is recorded, the last feature in `Group`). The body is otherwise the core
/// group container; `Group`/`Tip`/`Origin` stay core properties.
struct Body;

impl ObjectType for Body {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Group".to_string(), Property::LinkList(Vec::new()), prop_status::NONE),
            (
                "Tip".to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
            (
                "BaseFeature".to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
            (
                "Origin".to_string(),
                Property::Link(String::new()),
                prop_status::HIDDEN,
            ),
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            (
                "AllowCompound".to_string(),
                Property::Bool(true),
                prop_status::NONE,
            ),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Body".to_string(), format!("Body {name}")))
    }

    /// A whole `Group` assignment is checked against the members' declared
    /// capabilities (upstream `Body::isAllowed`; at most one base in the list).
    /// A rejected assignment leaves the body untouched.
    fn on_property_change(
        &self,
        doc: &Document,
        _id: ObjectId,
        name: &str,
        value: &Property,
    ) -> Result<(), PropertyChangeError> {
        if name == "Group" {
            if let Property::LinkList(links) = value {
                doc.body_group_check(links)
                    .map_err(PropertyChangeError::value)?;
            }
        }
        Ok(())
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        // `Tip` first, then the last `Group` member (`addObject` does not set the
        // Tip the way upstream's `Body::addObject` does).
        let tip = link_sub_property(doc, id, "Tip").map(|(name, _)| name);
        let tip = tip.and_then(|name| doc.get_by_name(&name).map(|member| (name, member)));
        let member = tip.or_else(|| {
            let group = match doc.object(id).and_then(|object| object.properties.get("Group")) {
                Some(Property::LinkList(names)) => names.clone(),
                _ => Vec::new(),
            };
            group
                .into_iter()
                .rev()
                .find_map(|name| doc.get_by_name(&name).map(|member| (name, member)))
        });
        let Some((name, member)) = member else {
            doc.set_property_raw(id, SHAPE, output_shape());
            return;
        };

        // `Group` membership does not order a recompute (core skips it), so the
        // body can be visited before the feature that builds its shape. Build the
        // member on demand; an acyclic document makes this safe.
        if object_shape(doc, member).is_none() && member != id {
            let member_type = doc
                .object(member)
                .map(|object| object.type_id.clone())
                .unwrap_or_default();
            if member_type != BODY_TYPE {
                doc.execute_object(member);
            }
        }
        if link_sub_property(doc, id, "Tip").is_none() {
            doc.set_property_raw(id, "Tip", Property::Link(name));
        }
        let shape = object_shape(doc, member).and_then(|shape| {
            // The body's own placement positions the whole group, on top of the
            // member's own placement (which its shape already carries).
            match doc.object(id).and_then(|object| object.properties.get("Placement")) {
                Some(Property::Placement(placement))
                    if !placement.is_same(&Placement::identity(), 1e-12) =>
                {
                    ferrocad_part::backend().place(&shape, placement).ok()
                }
                _ => Some(shape),
            }
        });
        match shape {
            Some(shape) => {
                doc.set_property_raw(
                    id,
                    SHAPE,
                    make_shape_property(ShapeProperty::with_lineage(shape, Lineage::default())),
                );
            }
            None => {
                doc.set_property_raw(id, SHAPE, output_shape());
            }
        }
    }
}

/// `PartDesign::AdditiveBox`: a parametric box, the shape source the PartDesign
/// tests put in a Body. It is upstream's `Part::Box` dimensions and `Shape`; no
/// PartDesign feature tree (`BaseFeature`, `AddSubShape`) is modelled.
struct AdditiveBox;

impl ObjectType for AdditiveBox {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Length".to_string(), Property::Float(10.0), prop_status::NONE),
            ("Width".to_string(), Property::Float(10.0), prop_status::NONE),
            ("Height".to_string(), Property::Float(10.0), prop_status::NONE),
            (
                "Placement".to_string(),
                Property::Placement(Placement::identity()),
                prop_status::NONE,
            ),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Additive".to_string(), format!("Additive box {name}")))
    }

    /// A PartDesign feature: the body's newest one becomes the `Tip`.
    fn body_membership(&self) -> BodyMembership {
        BodyMembership::Feature
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let length = float_property(doc, id, "Length", 10.0);
        let width = float_property(doc, id, "Width", 10.0);
        let height = float_property(doc, id, "Height", 10.0);
        let placement = match doc.object(id).and_then(|object| object.properties.get("Placement")) {
            Some(Property::Placement(placement)) => Some(*placement),
            _ => None,
        };
        if !(length > 0.0 && width > 0.0 && height > 0.0) {
            doc.mark_invalid(id);
            doc.set_property_raw(id, SHAPE, output_shape());
            return;
        }
        let result = ferrocad_part::backend()
            .make_box(length, width, height)
            .map_err(|error| error.to_string())
            .and_then(|shape| match placement {
                Some(placement) if !placement.is_same(&Placement::identity(), 1e-12) => {
                    ferrocad_part::backend()
                        .place(&shape, &placement)
                        .map_err(|error| error.to_string())
                }
                _ => Ok(shape),
            });
        store_named(doc, id, SHAPE, result);
    }
}

// ---------------------------------------------------------------------------
// `Part::Mirroring`
// ---------------------------------------------------------------------------

/// The reflection about the plane through `base` with direction `normal`.
fn reflection_matrix(base: Vector3, normal: Vector3) -> Matrix4 {
    let n = normal.normalize();
    let offset = 2.0 * n.dot(&base);
    let mut m = [0.0; 16];
    m[0] = 1.0 - 2.0 * n.x * n.x;
    m[1] = -2.0 * n.x * n.y;
    m[2] = -2.0 * n.x * n.z;
    m[3] = offset * n.x;
    m[4] = -2.0 * n.y * n.x;
    m[5] = 1.0 - 2.0 * n.y * n.y;
    m[6] = -2.0 * n.y * n.z;
    m[7] = offset * n.y;
    m[8] = -2.0 * n.z * n.x;
    m[9] = -2.0 * n.z * n.y;
    m[10] = 1.0 - 2.0 * n.z * n.z;
    m[11] = offset * n.z;
    m[15] = 1.0;
    Matrix4::from_values(m)
}

/// `Part::Mirroring`: `Shape` is `Source`'s shape reflected about the plane
/// through `Base` with direction `Normal`.
///
/// Unlike upstream, the source's `Placement` is not applied here: in FerroCAD a
/// primitive's `Shape` already contains its placement (and an `App::Link`'s
/// contains its link transform), so applying it again would move the mirror
/// twice. The tests in `TestPartMirror.py` pin exactly this world-space
/// behaviour and its stability across recomputes.
struct Mirroring;

impl ObjectType for Mirroring {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Source".to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
            (
                "Base".to_string(),
                Property::Vector(Vector3::zero()),
                prop_status::NONE,
            ),
            (
                "Normal".to_string(),
                Property::Vector(Vector3::new(0.0, 0.0, 1.0)),
                prop_status::NONE,
            ),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Mirroring".to_string(), format!("Part mirroring {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let source = link_sub_property(doc, id, "Source")
                .ok_or_else(|| "No object linked".to_string())?;
            let shape = linked_shape(doc, &source)?;
            let base = vector_like_property(doc, id, "Base").unwrap_or_else(Vector3::zero);
            let normal = vector_like_property(doc, id, "Normal")
                .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
            if normal.is_null() {
                return Err("Cannot mirror across a plane with a null normal".to_string());
            }
            let matrix = reflection_matrix(base, normal);
            let backend = ferrocad_part::backend();
            // A reflection has determinant -1, so it is not a placement; prefer
            // the gp_Trsf path and fall back to the general affine transform when
            // the kernel refuses it.
            backend
                .transform(&shape, &matrix)
                .or_else(|_| backend.general_transform(&shape, &matrix))
                .map_err(|error| error.to_string())
        })();
        store_named(doc, id, SHAPE, result);
    }
}

// ---------------------------------------------------------------------------
// Extended geometry: torus, wedge, revolution, tube and thread
// ---------------------------------------------------------------------------

/// Apply upstream's `gp_Ax2(pnt, dir)` frame to a shape that was built at the
/// origin with its axis along +Z.
fn locate(shape: Shape, base: Vector3, direction: Vector3) -> Result<Shape, String> {
    let rotation = if direction.length() > CONFUSION {
        Rotation::from_vectors(Vector3::new(0.0, 0.0, 1.0), direction)
    } else {
        Rotation::identity()
    };
    ferrocad_part::backend()
        .place(&shape, &Placement { base, rotation })
        .map_err(|error| error.to_string())
}

/// The kernel's `BRepPrimAPI_MakeTorus`, built from the seam's face/revolve pair.
///
/// Upstream's `TopoShape::makeTorus` makes exactly this construction: a circular
/// arc of `radius2` in the XZ plane centred at `(radius1, 0, 0)`, closed with two
/// radial segments into a sector, turned about the Z axis. A full section is a
/// plain circle, so no closing segments are added.
fn torus_shape(
    radius1: f64,
    radius2: f64,
    angle1: f64,
    angle2: f64,
    angle3: f64,
) -> Result<Shape, String> {
    if radius1 < CONFUSION || radius2 < CONFUSION {
        return Err("Radius of torus too small".to_string());
    }
    if !(angle1.is_finite() && angle2.is_finite() && angle3.is_finite()) {
        return Err("Angle of torus is not finite".to_string());
    }
    let first = angle1.to_radians();
    let last = angle2.to_radians();
    let sweep = last - first;
    if sweep.abs() <= 1e-12 || sweep.abs() > std::f64::consts::TAU + 1e-9 {
        return Err("Angle range of torus is invalid".to_string());
    }
    let revolution = angle3.to_radians();
    if revolution.abs() <= 1e-12 || revolution.abs() > std::f64::consts::TAU + 1e-9 {
        return Err("Revolution angle of torus is invalid".to_string());
    }

    let backend = ferrocad_part::backend();
    let circle = Circle::with_axis(
        Vector3::new(radius1, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        radius2,
    )
    .map_err(|error| error.to_string())?;

    let mut edges = Vec::new();
    if (sweep.abs() - std::f64::consts::TAU).abs() <= 1e-9 {
        edges.push(
            backend
                .make_edge(&EdgeCurve::Circle(circle))
                .map_err(|error| error.to_string())?,
        );
    } else {
        let arc = CurveArc::new(circle, first, last).map_err(|error| error.to_string())?;
        let start = arc.start();
        let end = arc.end();
        let center = circle.center();
        edges.push(
            backend
                .make_edge(&EdgeCurve::Arc(arc))
                .map_err(|error| error.to_string())?,
        );
        edges.push(
            backend
                .make_edge(&EdgeCurve::LineSegment(
                    LineSegment::new(end, center).map_err(|error| error.to_string())?,
                ))
                .map_err(|error| error.to_string())?,
        );
        edges.push(
            backend
                .make_edge(&EdgeCurve::LineSegment(
                    LineSegment::new(center, start).map_err(|error| error.to_string())?,
                ))
                .map_err(|error| error.to_string())?,
        );
    }

    let wire = backend.make_wire(&edges).map_err(|error| error.to_string())?;
    let face = backend
        .make_face(&[wire])
        .map_err(|error| error.to_string())?;
    let axis = if revolution < 0.0 {
        Vector3::new(0.0, 0.0, -1.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    };
    backend
        .revolve(&face, Vector3::zero(), axis, revolution.abs())
        .map_err(|error| error.to_string())
}

/// The ten parameters of `BRepPrim_Wedge`.
struct WedgeParameters {
    xmin: f64,
    ymin: f64,
    zmin: f64,
    z2min: f64,
    x2min: f64,
    xmax: f64,
    ymax: f64,
    zmax: f64,
    z2max: f64,
    x2max: f64,
}

/// A closed polygon wire through `points`.
fn polygon_wire(points: &[Vector3]) -> Result<Shape, String> {
    let curves = ferrocad_curves::polygon(points, true).map_err(|error| error.to_string())?;
    let backend = ferrocad_part::backend();
    let mut edges = Vec::with_capacity(curves.len());
    for curve in &curves {
        edges.push(
            backend
                .make_edge(curve)
                .map_err(|error| error.to_string())?,
        );
    }
    backend
        .make_wire(&edges)
        .map_err(|error| error.to_string())
}

/// The dimension checks `BRepPrim_Wedge`/upstream's `makeWedge` perform.
fn validate_wedge(parameters: &WedgeParameters) -> Result<(), String> {
    let dx = parameters.xmax - parameters.xmin;
    let dy = parameters.ymax - parameters.ymin;
    let dz = parameters.zmax - parameters.zmin;
    let dz2 = parameters.z2max - parameters.z2min;
    let dx2 = parameters.x2max - parameters.x2min;
    if dx < CONFUSION {
        return Err("delta x of wedge too small".to_string());
    }
    if dy < CONFUSION {
        return Err("delta y of wedge too small".to_string());
    }
    if dz < CONFUSION {
        return Err("delta z of wedge too small".to_string());
    }
    if dz2 < 0.0 {
        return Err("delta z2 of wedge is negative".to_string());
    }
    if dx2 < 0.0 {
        return Err("delta x2 of wedge is negative".to_string());
    }
    Ok(())
}

/// `BRepPrim_Wedge` rebuilt as a ruled loft between its two rectangular faces.
///
/// The primitive is exactly the linear interpolation between the rectangle at
/// `Y = ymin` (`[xmin, xmax] × [zmin, zmax]`) and the rectangle at `Y = ymax`
/// (`[x2min, x2max] × [z2min, z2max]`); both are planar, so a two-section ruled
/// loft is the same solid with no approximation.
fn wedge_shape(parameters: &WedgeParameters) -> Result<Shape, String> {
    validate_wedge(parameters)?;
    let left = [
        Vector3::new(parameters.xmin, parameters.ymin, parameters.zmin),
        Vector3::new(parameters.xmax, parameters.ymin, parameters.zmin),
        Vector3::new(parameters.xmax, parameters.ymin, parameters.zmax),
        Vector3::new(parameters.xmin, parameters.ymin, parameters.zmax),
    ];
    let right = [
        Vector3::new(parameters.x2min, parameters.ymax, parameters.z2min),
        Vector3::new(parameters.x2max, parameters.ymax, parameters.z2min),
        Vector3::new(parameters.x2max, parameters.ymax, parameters.z2max),
        Vector3::new(parameters.x2min, parameters.ymax, parameters.z2max),
    ];
    let left = polygon_wire(&left)?;
    let right = polygon_wire(&right)?;
    ferrocad_part::backend()
        .make_loft(&[left, right], true)
        .map_err(|error| error.to_string())
}

/// `Shape.revolve` about an arbitrary axis, with upstream's angular bounds.
fn revolution_shape(
    source: &Shape,
    base: Vector3,
    axis: Vector3,
    angle_radians: f64,
) -> Result<Shape, String> {
    if !axis.length().is_finite() || axis.length() <= CONFUSION {
        return Err("Direction of revolution axis is zero-length".to_string());
    }
    if !angle_radians.is_finite() || angle_radians.abs() <= 1e-12 {
        return Err("Angle of revolution is zero".to_string());
    }
    if angle_radians.abs() > std::f64::consts::TAU + 1e-9 {
        return Err("Angle of revolution exceeds 360 degrees".to_string());
    }
    let (axis, angle) = if angle_radians < 0.0 {
        (axis.neg(), -angle_radians)
    } else {
        (axis, angle_radians)
    };
    ferrocad_part::backend()
        .revolve(source, base, axis, angle)
        .map_err(|error| error.to_string())
}

/// The frame at the start of a tube spine: `(point, unit tangent)`.
///
/// The kernel's adaptors (`BRepAdaptor_Curve` for an edge, and
/// `BRepAdaptor_CompCurve` for a wire) evaluate every bounded spine, including
/// a B-spline, so this no longer needs the analytic `curve_of` description. A
/// spine that is neither an edge nor a wire is reported.
fn spine_frame(shape: &Shape) -> Result<(Vector3, Vector3), String> {
    surface_ops::spine_start_frame(shape)
        .map_err(|_| "makeTube needs a bounded edge or wire spine".to_string())
}

/// A circular profile centred on `frame`'s point, normal to its tangent.
fn circular_profile(
    start: Vector3,
    tangent: Vector3,
    radius: f64,
) -> Result<Shape, String> {
    let x_axis = plane_x_direction(&tangent);
    let circle = Circle::with_axis(start, tangent, x_axis, radius)
        .map_err(|error| error.to_string())?;
    ferrocad_part::backend()
        .make_edge(&EdgeCurve::Circle(circle))
        .map_err(|error| error.to_string())
}

/// `TopoShape::makeTube`: a circular profile swept along the spine edge.
fn tube_shape(spine: &Shape, radius: f64, tolerance: f64) -> Result<Shape, String> {
    if radius < CONFUSION {
        return Err("Radius of tube too small".to_string());
    }
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err("Tolerance of tube must be positive".to_string());
    }
    let (start, tangent) = spine_frame(spine)?;
    let profile = circular_profile(start, tangent, radius)?;
    // `BRepOffsetAPI_MakePipeShell` uses the corrected-Frenet trihedron, the
    // same frame law upstream's `makeTube` asks `GeomFill` for. The approximation
    // tolerance is not a parameter of the pipe shell; it is accepted for the
    // upstream signature.
    let _ = tolerance;
    surface_ops::make_pipe_shell(spine, &[profile], false, false)
        .map_err(|error| error.to_string())
}

/// `TopoShape::makeThread`: a solid lofted between two thread profiles.
///
/// Upstream builds two thread-profile wires on coaxial cylinders of `radius`
/// and `radius + depth` and lofts them with `BRepOffsetAPI_ThruSections`; this
/// uses the same construction through [`surface_ops::make_thread`].
fn thread_shape(pitch: f64, depth: f64, height: f64, radius: f64) -> Result<Shape, String> {
    if pitch < CONFUSION {
        return Err("Pitch of thread too small".to_string());
    }
    if depth < CONFUSION {
        return Err("Depth of thread too small".to_string());
    }
    if height < CONFUSION {
        return Err("Height of thread too small".to_string());
    }
    if radius < CONFUSION {
        return Err("Radius of thread too small".to_string());
    }
    surface_ops::make_thread(pitch, depth, height, radius).map_err(|error| error.to_string())
}

/// `Part.makeTorus(radius1, radius2, pnt=None, dir=None, angle1=0, angle2=360, angle=360)`
/// → a torus solid.
///
/// `angle1`/`angle2` span the tube's cross-section (degrees) and `angle` the
/// revolution, exactly as `BRepPrimAPI_MakeTorus`.
#[pyfunction]
#[pyo3(signature = (radius1, radius2, pnt = None, dir = None, angle1 = 0.0, angle2 = 360.0, angle = 360.0))]
fn makeTorus(
    radius1: f64,
    radius2: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle1: f64,
    angle2: f64,
    angle: f64,
) -> PyResult<PartShape> {
    let shape = torus_shape(radius1, radius2, angle1, angle2, angle)
        .map_err(OCCError::new_err)?;
    let base = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(Vector3::zero);
    let direction = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    let shape = locate(shape, base, direction).map_err(OCCError::new_err)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeWedge(xmin, ymin, zmin, z2min, x2min, xmax, ymax, zmax, z2max,
/// x2max, pnt=None, dir=None)` → a wedge solid.
///
/// This is `BRepPrim_Wedge`'s ten-parameter form, as upstream's `makeWedge`. The
/// seven-parameter shorthand `(lx, ly, lz, lxmin, lzmin, lxmax, lzmax)` is also
/// accepted: it places the first rectangle on the origin plane, i.e. `Xmin = Ymin
/// = Zmin = 0`, `Xmax = lx`, `Ymax = ly`, `Zmax = lz`, `X2min = lxmin`, `Z2min =
/// lzmin`, `X2max = lxmax`, `Z2max = lzmax`.
#[pyfunction]
#[pyo3(signature = (*args, pnt = None, dir = None))]
fn makeWedge(
    args: &Bound<'_, PyTuple>,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
) -> PyResult<PartShape> {
    let mut values: Vec<f64> = Vec::new();
    let mut positional_pnt: Option<Vector3> = None;
    let mut positional_dir: Option<Vector3> = None;
    for index in 0..args.len() {
        let value = args.get_item(index)?;
        if let Ok(number) = value.extract::<f64>() {
            values.push(number);
        } else if positional_pnt.is_none() {
            positional_pnt = Some(curves::vector(&value)?);
        } else if positional_dir.is_none() {
            positional_dir = Some(curves::vector(&value)?);
        } else {
            return Err(PyTypeError::new_err("too many arguments to makeWedge"));
        }
    }
    let parameters = match values.as_slice() {
        [xmin, ymin, zmin, z2min, x2min, xmax, ymax, zmax, z2max, x2max] => WedgeParameters {
            xmin: *xmin,
            ymin: *ymin,
            zmin: *zmin,
            z2min: *z2min,
            x2min: *x2min,
            xmax: *xmax,
            ymax: *ymax,
            zmax: *zmax,
            z2max: *z2max,
            x2max: *x2max,
        },
        [lx, ly, lz, lxmin, lzmin, lxmax, lzmax] => WedgeParameters {
            xmin: 0.0,
            ymin: 0.0,
            zmin: 0.0,
            z2min: *lzmin,
            x2min: *lxmin,
            xmax: *lx,
            ymax: *ly,
            zmax: *lz,
            z2max: *lzmax,
            x2max: *lxmax,
        },
        _ => {
            return Err(PyTypeError::new_err(
                "makeWedge expects the ten OCC parameters (xmin, ymin, zmin, z2min, \
                 x2min, xmax, ymax, zmax, z2max, x2max) or the seven-parameter \
                 shorthand (lx, ly, lz, lxmin, lzmin, lxmax, lzmax)",
            ))
        }
    };
    validate_wedge(&parameters).map_err(PyValueError::new_err)?;
    let shape = wedge_shape(&parameters).map_err(OCCError::new_err)?;
    let base = match pnt {
        Some(value) => curves::vector(value)?,
        None => positional_pnt.unwrap_or_else(Vector3::zero),
    };
    let direction = match dir {
        Some(value) => curves::vector(value)?,
        None => positional_dir.unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0)),
    };
    let shape = locate(shape, base, direction).map_err(OCCError::new_err)?;
    Ok(PartShape::from_shape(shape))
}

/// The shape a revolution input names: a `Part.Shape`, an edge, or an analytic
/// geometry object with `toShape()` (`Part.Circle`, `Part.LineSegment`, …).
fn revolution_source(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    if let Ok(shape) = value.downcast::<PartShape>() {
        return shape
            .borrow()
            .shape()
            .ok_or_else(|| OCCError::new_err("shape is empty"));
    }
    let converted = value.call_method0("toShape").map_err(|_| {
        PyTypeError::new_err("makeRevolution expects a shape, an edge or a geometry")
    })?;
    let shape = converted.downcast::<PartShape>().map_err(|_| {
        PyTypeError::new_err("the geometry's toShape() did not return a Part.Shape")
    })?;
    shape
        .borrow()
        .shape()
        .ok_or_else(|| OCCError::new_err("shape is empty"))
}

/// `Part.makeRevolution(shape, vmin=None, vmax=None, angle=360, pnt=None,
/// dir=None, shapetype=None)` → the revolved shape.
///
/// Upstream revolves a curve or an edge about the axis through `pnt` along `dir`.
/// `vmin`/`vmax` trim the curve's parameter range: a curve geometry is trimmed
/// with its own `trim`, while an edge is reported because the seam exposes no
/// edge parameter query. The shape is revolved with the backend's `revolve`.
///
/// The shorthand `makeRevolution(shape, axis, angle=360, solid=False)` is also
/// accepted when the second positional argument is a vector: `axis` becomes
/// `dir` and `solid` asks for a `Part.Solid` result.
#[pyfunction]
#[pyo3(signature = (curve, *args, pnt = None, dir = None, shapetype = None))]
fn makeRevolution(
    curve: &Bound<'_, PyAny>,
    args: &Bound<'_, PyTuple>,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    shapetype: Option<&Bound<'_, PyAny>>,
) -> PyResult<PartShape> {
    let mut base = pnt.map(curves::vector).transpose()?;
    let mut direction = dir.map(curves::vector).transpose()?;
    let mut angle = 360.0f64;
    let mut solid = false;
    let mut vmin: Option<f64> = None;
    let mut vmax: Option<f64> = None;
    let mut wanted_type: Option<String> = shapetype
        .and_then(|value| value.getattr("__name__").ok())
        .and_then(|name| name.extract::<String>().ok());

    let positional_is_range = if args.is_empty() {
        true
    } else {
        let first = args.get_item(0)?;
        // A leading `None` is upstream's `vmin=None`; a float is a range too.
        first.is_none() || first.extract::<f64>().is_ok()
    };
    if positional_is_range {
        let get = |index: usize| -> PyResult<Option<Bound<'_, PyAny>>> {
            if args.len() > index {
                let value = args.get_item(index)?;
                Ok((!value.is_none()).then_some(value))
            } else {
                Ok(None)
            }
        };
        vmin = get(0)?.map(|value| value.extract::<f64>()).transpose()?;
        vmax = get(1)?.map(|value| value.extract::<f64>()).transpose()?;
        if let Some(value) = get(2)? {
            angle = value.extract::<f64>()?;
        }
        if let Some(value) = get(3)? {
            base = Some(curves::vector(&value)?);
        }
        if let Some(value) = get(4)? {
            direction = Some(curves::vector(&value)?);
        }
        if let Some(value) = get(5)? {
            wanted_type = value
                .getattr("__name")
                .ok()
                .and_then(|name| name.extract::<String>().ok());
        }
    } else {
        // The task's shorthand: (shape, axis, angle=360, solid=False).
        direction = Some(curves::vector(&args.get_item(0)?)?);
        if args.len() > 1 {
            let value = args.get_item(1)?;
            if !value.is_none() {
                angle = value.extract::<f64>()?;
            }
        }
        if args.len() > 2 {
            solid = args.get_item(2)?.extract::<bool>()?;
        }
    }

    if vmin.is_some() || vmax.is_some() {
        // Upstream trims the curve to `[vmin, vmax]`. An analytic geometry exposes
        // `trim`; an edge would need its parameter range, which the seam cannot
        // report, so that case is refused rather than silently ignored.
        if curve.downcast::<PartShape>().is_ok() {
            return Err(OCCError::new_err(
                "makeRevolution: trimming an edge with vmin/vmax is not implemented \
                 (the seam exposes no edge parameter query); pass a curve geometry \
                 instead",
            ));
        }
    }

    let source = if vmin.is_some() || vmax.is_some() {
        let trimmed = curve.call_method1("trim", (vmin, vmax)).map_err(|_| {
            OCCError::new_err(
                "makeRevolution: this geometry cannot be trimmed with vmin/vmax",
            )
        })?;
        revolution_source(&trimmed)?
    } else {
        revolution_source(curve)?
    };
    let base = base.unwrap_or_else(Vector3::zero);
    let direction = direction.unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    let mut shape = revolution_shape(&source, base, direction, angle.to_radians())
        .map_err(OCCError::new_err)?;
    if solid {
        shape = single_solid(shape).map_err(OCCError::new_err)?;
    }
    if let Some(wanted) = wanted_type {
        let natural = PartShape::from_shape(shape.clone()).shape_type();
        if wanted != natural {
            // A partial revolve of a face can be a shell; asking for `Part.Solid`
            // promotes it when the result is one closed shell (upstream's
            // `shape2pyshape` cast). Nothing else is converted.
            if wanted == "Solid" {
                shape = single_solid(shape).map_err(OCCError::new_err)?;
            } else {
                return Err(OCCError::new_err(format!(
                    "makeRevolution: converting the revolved {natural} to {wanted} is not \
                     implemented; pass shapetype=None for the natural result"
                )));
            }
        }
    }
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeTube(edge, radius, tolerance=1e-3)` → the tube surface around `edge`.
///
/// Upstream's `TopoShape::makeTube` sweeps a circle along the spine with a
/// corrected-Frenet law. The profile is placed at the spine's start, normal to
/// its tangent; the kernel's `BRepAdaptor_Curve` (or `BRepAdaptor_CompCurve` for
/// a wire) supplies that frame for every bounded spine, including a B-spline.
/// The upstream extra arguments `scont`, `maxdegree` and `maxsegment` are
/// accepted but have no effect: the pipe shell uses OCCT's own tolerances.
#[pyfunction]
#[pyo3(signature = (edge, radius, *args, tolerance = 0.001))]
fn makeTube(
    edge: &Bound<'_, PyAny>,
    radius: f64,
    args: &Bound<'_, PyTuple>,
    tolerance: f64,
) -> PyResult<PartShape> {
    let tolerance = match args.get_item(0) {
        Ok(value) if value.extract::<f64>().is_ok() => value.extract::<f64>()?,
        // Upstream's `scont` is a string; the remaining arguments are approximation
        // settings that `make_pipe` does not take.
        _ => tolerance,
    };
    let spine = shape_argument(edge)?;
    let shape = tube_shape(&spine, radius, tolerance).map_err(OCCError::new_err)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeThread(pitch, depth, height, radius)` → a thread solid.
///
/// Upstream lofts two thread-profile wires on coaxial cylinders of `radius` and
/// `radius + depth` into a solid with `BRepOffsetAPI_ThruSections`; this is that
/// same two-profile solid, built by [`surface_ops::make_thread`].
#[pyfunction]
#[pyo3(signature = (pitch, depth, height, radius))]
fn makeThread(pitch: f64, depth: f64, height: f64, radius: f64) -> PyResult<PartShape> {
    let shape = thread_shape(pitch, depth, height, radius).map_err(OCCError::new_err)?;
    Ok(PartShape::from_shape(shape))
}

/// The single solid in `shape`, or a solid built from its single shell.
///
/// A revolve/pipe may already be a solid; otherwise one closed shell is promoted
/// (which `BRepBuilderAPI_MakeSolid` accepts) and anything else is reported.
fn single_solid(shape: Shape) -> Result<Shape, String> {
    let backend = ferrocad_part::backend();
    if !backend.elements(&shape, ElementKind::Solid).is_empty() {
        return Ok(shape);
    }
    let shells = backend.elements(&shape, ElementKind::Shell);
    match shells.as_slice() {
        [shell] => {
            let solid = backend
                .make_solid(&shell.shape)
                .map_err(|error| error.to_string())?;
            // Promoting an open shell yields an invalid solid; report it rather
            // than storing geometry the kernel considers broken.
            if !backend.is_valid(&solid) {
                return Err(
                    "the result is an open shell and cannot become a solid".to_string(),
                );
            }
            Ok(solid)
        }
        [] => Err("the result contains no solid or shell".to_string()),
        _ => Err("the result has several shells, so it cannot become one solid".to_string()),
    }
}

/// The object's baked `Placement`, when it has one.
fn placement_property(doc: &Document, id: ObjectId) -> Option<Placement> {
    match doc.object(id)?.properties.get("Placement")? {
        Property::Placement(value) => Some(*value),
        _ => None,
    }
}

fn apply_object_placement(doc: &Document, id: ObjectId, shape: Shape) -> Result<Shape, String> {
    match placement_property(doc, id) {
        Some(placement) => ferrocad_part::backend()
            .place(&shape, &placement)
            .map_err(|error| error.to_string()),
        None => Ok(shape),
    }
}

/// The object names of a `LinkList` property.
fn link_list_property(doc: &Document, id: ObjectId, name: &str) -> Vec<String> {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::LinkList(names)) => names.clone(),
        _ => Vec::new(),
    }
}

/// Resolve every sub-shape a link names, or the whole linked shape when it names
/// no sub-elements.
fn linked_sub_shapes(doc: &Document, link: &(String, Vec<String>)) -> Result<Vec<Shape>, String> {
    let (name, subs) = link;
    let id = doc
        .get_by_name(name)
        .ok_or_else(|| format!("no object named {name}"))?;
    let base = doc
        .object(id)
        .and_then(|object| object.properties.get(SHAPE))
        .and_then(shape_of)
        .and_then(|property| property.shape())
        .ok_or_else(|| format!("object {name} has no shape"))?;
    if subs.is_empty() {
        return Ok(vec![base]);
    }
    subs.iter()
        .map(|sub| {
            ferrocad_part::backend()
                .resolve(&base, sub)
                .ok_or_else(|| format!("object {name} has no sub-shape {sub}"))
        })
        .collect()
}

/// The spine of a sweep: one shape, or the named sub-edges joined into a wire.
fn sweep_spine(doc: &Document, link: &(String, Vec<String>)) -> Result<Shape, String> {
    let shapes = linked_sub_shapes(doc, link)?;
    match shapes.len() {
        0 => Err("No spine".to_string()),
        1 => Ok(shapes.into_iter().next().expect("one shape")),
        _ => ferrocad_part::backend()
            .make_wire(&shapes)
            .map_err(|error| error.to_string()),
    }
}

/// A solid sweep needs a face section; a wire is filled first.
fn sweep_section_face(profile: &Shape) -> Result<Shape, String> {
    let backend = ferrocad_part::backend();
    match PartShape::from_shape(profile.clone()).shape_type() {
        "Face" => Ok(profile.clone()),
        "Wire" => backend
            .make_face(std::slice::from_ref(profile))
            .map_err(|error| error.to_string()),
        kind => Err(format!(
            "a solid sweep needs a wire or a face section, got {kind}"
        )),
    }
}

/// The unit direction of an axis edge (upstream `Revolution::fetchAxisLink`).
fn axis_from_edge(shape: &Shape) -> Result<(Vector3, Vector3, Option<f64>), String> {
    let backend = ferrocad_part::backend();
    let flipped = backend.orientation(shape) == Some("Reversed");
    let (center, direction, span) = match backend.curve_of(shape) {
        Ok(EdgeCurve::LineSegment(segment)) => {
            let point = if flipped {
                segment.start()
            } else {
                segment.end()
            };
            let direction = segment.end().sub(&segment.start()).normalize();
            (point, direction, None)
        }
        Ok(EdgeCurve::Line(line)) => (line.origin(), line.direction(), None),
        Ok(EdgeCurve::Circle(circle)) => (
            circle.center(),
            circle.normal(),
            Some(std::f64::consts::TAU),
        ),
        Ok(EdgeCurve::Arc(arc)) => (
            arc.circle().center(),
            arc.circle().normal(),
            Some(arc.sweep().abs()),
        ),
        _ => {
            return Err("AxisLink edge is neither line nor arc of circle.".to_string())
        }
    };
    let direction = if flipped { direction.neg() } else { direction };
    Ok((center, direction, span))
}

/// A rotation by `angle` about the axis through `base`.
fn rotation_about(base: Vector3, axis: Vector3, angle: f64) -> Matrix4 {
    let rotation = Rotation::from_axis_angle(&axis, angle).to_matrix();
    let mut to_origin = Matrix4::identity();
    to_origin.m[3] = -base.x;
    to_origin.m[7] = -base.y;
    to_origin.m[11] = -base.z;
    let mut from_origin = Matrix4::identity();
    from_origin.m[3] = base.x;
    from_origin.m[7] = base.y;
    from_origin.m[11] = base.z;
    from_origin.mul(&rotation).mul(&to_origin)
}

// ---------------------------------------------------------------------------
// Document types: `Part::Torus`, `Part::Wedge`, `Part::Revolution`,
// `Part::Helix`, `Part::Sweep`
// ---------------------------------------------------------------------------

/// `Part::Torus` (`PrimitiveFeature.cpp`).
pub const TORUS_TYPE: &str = "Part::Torus";
/// `Part::Prism` (`PrimitiveFeature.cpp`).
pub const PRISM_TYPE: &str = "Part::Prism";
/// `Part::Wedge` (`PrimitiveFeature.cpp`).
pub const WEDGE_TYPE: &str = "Part::Wedge";
/// `Part::Revolution` (`FeatureRevolution.cpp`).
pub const REVOLUTION_TYPE: &str = "Part::Revolution";
/// `Part::Helix` (`PrimitiveFeature.cpp`).
pub const HELIX_TYPE: &str = "Part::Helix";
/// `Part::Sweep` (`PartFeatures.cpp`).
pub const SWEEP_TYPE: &str = "Part::Sweep";

fn placement_property_entry() -> (String, Property, u32) {
    (
        "Placement".to_string(),
        Property::Placement(Placement::identity()),
        prop_status::NONE,
    )
}

/// `Part::Prism`: a regular polygon extruded along `+Z` (upstream's
/// `Part::Prism` with `PrismExtension`).
///
/// `Polygon` is the number of sides (3 or more), `Circumradius` the centre-to-vertex
/// radius, `Height` the extrusion distance, and `FirstAngle`/`SecondAngle` slant the
/// extrusion in X and Y (upstream feeds `h·tan(angle)` into `BRepPrimAPI_MakePrism`).
struct PrismPrimitive;

impl ObjectType for PrismPrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Polygon".to_string(),
                Property::Integer(6),
                prop_status::NONE,
            ),
            (
                "Circumradius".to_string(),
                Property::Float(2.0),
                prop_status::NONE,
            ),
            ("Height".to_string(), Property::Float(10.0), prop_status::NONE),
            (
                "FirstAngle".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            (
                "SecondAngle".to_string(),
                Property::Float(0.0),
                prop_status::NONE,
            ),
            placement_property_entry(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Prism".to_string(), format!("Part prism {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let polygon = integer_property(doc, id, "Polygon", 6);
            let shape = prism_shape(
                polygon,
                float_property(doc, id, "Circumradius", 2.0),
                float_property(doc, id, "Height", 10.0),
                float_property(doc, id, "FirstAngle", 0.0),
                float_property(doc, id, "SecondAngle", 0.0),
            )?;
            apply_object_placement(doc, id, shape)
        })();
        store(doc, id, result);
    }
}

/// The B-Rep of a `Part::Prism`: a regular polygon face extruded along `+Z`.
fn prism_shape(
    polygon: i32,
    circumradius: f64,
    height: f64,
    first_angle: f64,
    second_angle: f64,
) -> Result<Shape, String> {
    if polygon < 3 {
        return Err("Polygon of prism is invalid, must have 3 or more sides".to_string());
    }
    if circumradius <= ferrocad_curves::TOLERANCE {
        return Err("Circumradius of the polygon, of the prism, is too small".to_string());
    }
    if height <= ferrocad_curves::TOLERANCE {
        return Err("Height of prism is too small".to_string());
    }
    let mut points = Vec::with_capacity(polygon as usize);
    for index in 0..polygon {
        let angle = std::f64::consts::TAU * f64::from(index) / f64::from(polygon);
        points.push(Vector3::new(
            circumradius * angle.cos(),
            circumradius * angle.sin(),
            0.0,
        ));
    }
    let mut edges = Vec::with_capacity(points.len());
    for index in 0..points.len() {
        let segment = LineSegment::new(points[index], points[(index + 1) % points.len()])
            .map_err(|error| error.to_string())?;
        edges.push(
            ferrocad_part::backend()
                .make_edge(&EdgeCurve::LineSegment(segment))
                .map_err(|error| error.to_string())?,
        );
    }
    let wire = ferrocad_part::backend()
        .make_wire(&edges)
        .map_err(|error| error.to_string())?;
    let face = ferrocad_part::backend()
        .make_face(&[wire])
        .map_err(|error| error.to_string())?;
    let direction = Vector3::new(
        height * first_angle.to_radians().tan(),
        height * second_angle.to_radians().tan(),
        height,
    );
    ferrocad_part::backend()
        .extrude(&face, direction)
        .map_err(|error| error.to_string())
}

fn integer_property(doc: &Document, id: ObjectId, name: &str, default: i32) -> i32 {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::Integer(value)) => *value as i32,
        Some(Property::Float(value)) => *value as i32,
        _ => default,
    }
}

/// `Part::Torus`: `Radius1`/`Radius2`, the two cross-section angles and the
/// revolution angle, exactly upstream's primitive.
struct TorusPrimitive;

impl ObjectType for TorusPrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Radius1".to_string(), Property::Float(10.0), prop_status::NONE),
            ("Radius2".to_string(), Property::Float(2.0), prop_status::NONE),
            ("Angle1".to_string(), Property::Float(-180.0), prop_status::NONE),
            ("Angle2".to_string(), Property::Float(180.0), prop_status::NONE),
            ("Angle3".to_string(), Property::Float(360.0), prop_status::NONE),
            placement_property_entry(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Torus".to_string(), format!("Part torus {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let shape = torus_shape(
                float_property(doc, id, "Radius1", 10.0),
                float_property(doc, id, "Radius2", 2.0),
                float_property(doc, id, "Angle1", -180.0),
                float_property(doc, id, "Angle2", 180.0),
                float_property(doc, id, "Angle3", 360.0),
            )?;
            apply_object_placement(doc, id, shape)
        })();
        store(doc, id, result);
    }
}

/// `Part::Wedge`: `BRepPrim_Wedge`'s ten coordinates.
struct WedgePrimitive;

impl ObjectType for WedgePrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Xmin".to_string(), Property::Float(0.0), prop_status::NONE),
            ("Ymin".to_string(), Property::Float(0.0), prop_status::NONE),
            ("Zmin".to_string(), Property::Float(0.0), prop_status::NONE),
            ("X2min".to_string(), Property::Float(2.0), prop_status::NONE),
            ("Z2min".to_string(), Property::Float(2.0), prop_status::NONE),
            ("Xmax".to_string(), Property::Float(10.0), prop_status::NONE),
            ("Ymax".to_string(), Property::Float(10.0), prop_status::NONE),
            ("Zmax".to_string(), Property::Float(10.0), prop_status::NONE),
            ("X2max".to_string(), Property::Float(8.0), prop_status::NONE),
            ("Z2max".to_string(), Property::Float(8.0), prop_status::NONE),
            placement_property_entry(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Wedge".to_string(), format!("Part wedge {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let parameters = WedgeParameters {
                xmin: float_property(doc, id, "Xmin", 0.0),
                ymin: float_property(doc, id, "Ymin", 0.0),
                zmin: float_property(doc, id, "Zmin", 0.0),
                z2min: float_property(doc, id, "Z2min", 2.0),
                x2min: float_property(doc, id, "X2min", 2.0),
                xmax: float_property(doc, id, "Xmax", 10.0),
                ymax: float_property(doc, id, "Ymax", 10.0),
                zmax: float_property(doc, id, "Zmax", 10.0),
                z2max: float_property(doc, id, "Z2max", 8.0),
                x2max: float_property(doc, id, "X2max", 8.0),
            };
            let shape = wedge_shape(&parameters)?;
            apply_object_placement(doc, id, shape)
        })();
        store(doc, id, result);
    }
}

/// `Part::Revolution`: `Source` revolved about `Base`/`Axis` (or the edge named by
/// `AxisLink`) by `Angle` degrees.
///
/// `Symmetric` rotates the profile back by half the angle first, as upstream.
/// `FaceMakerClass` is accepted for compatibility; the seam's `revolve` already
/// returns a solid for a face and a face for an edge.
struct Revolution;

impl ObjectType for Revolution {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Source".to_string(),
                Property::Link(String::new()),
                prop_status::NONE,
            ),
            (
                "Base".to_string(),
                Property::Vector(Vector3::zero()),
                prop_status::NONE,
            ),
            (
                "Axis".to_string(),
                Property::Vector(Vector3::new(0.0, 0.0, 1.0)),
                prop_status::NONE,
            ),
            (
                "AxisLink".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            ("Angle".to_string(), Property::Float(360.0), prop_status::NONE),
            ("Symmetric".to_string(), Property::Bool(false), prop_status::NONE),
            ("Solid".to_string(), Property::Bool(false), prop_status::NONE),
            (
                "FaceMakerClass".to_string(),
                Property::String("Part::FaceMakerUnified".to_string()),
                prop_status::NONE,
            ),
            placement_property_entry(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Revolve".to_string(), format!("Part revolution {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let source_link = link_sub_property(doc, id, "Source")
                .ok_or_else(|| "No object linked".to_string())?;
            let mut source = linked_shape(doc, &source_link)?;
            let mut base = vector_property(doc, id, "Base").unwrap_or_else(Vector3::zero);
            let mut axis = vector_property(doc, id, "Axis")
                .unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
            let mut angle = float_property(doc, id, "Angle", 360.0).to_radians();
            if let Some(axis_link) = link_sub_property(doc, id, "AxisLink") {
                let edge = linked_shape(doc, &axis_link)?;
                let (center, direction, span) = axis_from_edge(&edge)?;
                base = center;
                axis = direction;
                if angle.abs() < 1e-12 {
                    angle = span.unwrap_or(0.0);
                }
            }
            if bool_property(doc, id, "Symmetric", false) {
                let matrix = rotation_about(base, axis, -0.5 * angle);
                source = ferrocad_part::backend()
                    .transform(&source, &matrix)
                    .map_err(|error| error.to_string())?;
            }
            let mut shape = revolution_shape(&source, base, axis, angle)?;
            if bool_property(doc, id, "Solid", false) {
                shape = single_solid(shape)?;
            }
            apply_object_placement(doc, id, shape)
        })();
        store(doc, id, result);
    }
}

/// `Part::Helix`: upstream's `Pitch`/`Height`/`Radius`/`Angle` primitive.
///
/// `Length` is the edge's linear length, recomputed on every execution.
/// `SegmentLength` only subdivides upstream's wire and `Style` is ignored by
/// upstream's own `execute`, so neither changes the geometry here either.
struct HelixPrimitive;

impl ObjectType for HelixPrimitive {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            ("Pitch".to_string(), Property::Float(1.0), prop_status::NONE),
            ("Height".to_string(), Property::Float(2.0), prop_status::NONE),
            ("Radius".to_string(), Property::Float(1.0), prop_status::NONE),
            ("SegmentLength".to_string(), Property::Float(0.0), prop_status::NONE),
            ("Angle".to_string(), Property::Float(0.0), prop_status::NONE),
            (
                "LocalCoord".to_string(),
                Property::Enumeration(
                    vec!["Right-handed".to_string(), "Left-handed".to_string()],
                    0,
                ),
                prop_status::NONE,
            ),
            (
                "Style".to_string(),
                Property::Enumeration(
                    vec!["Old style".to_string(), "New style".to_string()],
                    0,
                ),
                prop_status::HIDDEN,
            ),
            (
                "Length".to_string(),
                Property::Float(0.0),
                prop_status::READONLY,
            ),
            placement_property_entry(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Helix".to_string(), format!("Part helix {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let pitch = float_property(doc, id, "Pitch", 1.0);
            if pitch < CONFUSION {
                return Err("Pitch too small".to_string());
            }
            let height = float_property(doc, id, "Height", 2.0);
            let radius = float_property(doc, id, "Radius", 1.0);
            let angle = float_property(doc, id, "Angle", 0.0);
            let turns = height / pitch;
            if turns > 1e4 {
                return Err("Number of turns too high (> 1e4)".to_string());
            }
            let left_handed = enum_index_property(doc, id, "LocalCoord") == 1;
            // The exact-by-construction helix: an edge whose pcurve is a straight
            // line on an analytic cylinder/cone (upstream's `TopoShape::makeHelix`).
            let shape = surface_ops::make_helix_edge(pitch, height, radius, angle, left_handed)
                .map_err(|error| error.to_string())?;
            apply_object_placement(doc, id, shape)
        })();
        if let Ok(shape) = &result {
            if let Some(mass) = ferrocad_part::backend().mass_properties(shape) {
                doc.set_property_raw(id, "Length", Property::Float(mass.length));
            }
        }
        store(doc, id, result);
    }
}

/// `Part::Sweep`: `Sections` swept along `Spine`.
///
/// The kernel's `BRepOffsetAPI_MakePipeShell` sweeps one or several profiles in
/// order, with the `Frenet` trihedron the property selects. The `Transition`
/// property is accepted for upstream compatibility but the shell always uses
/// OCCT's `Transformed` transition; `Linearize` has no pipe-shell counterpart.
/// A `Solid` sweep closes the swept shell into a solid.
struct Sweep;

impl ObjectType for Sweep {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Sections".to_string(),
                Property::LinkList(Vec::new()),
                prop_status::NONE,
            ),
            (
                "Spine".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            ("Solid".to_string(), Property::Bool(true), prop_status::NONE),
            ("Frenet".to_string(), Property::Bool(true), prop_status::NONE),
            (
                "Transition".to_string(),
                Property::Enumeration(
                    vec![
                        "Transformed".to_string(),
                        "Right corner".to_string(),
                        "Round corner".to_string(),
                    ],
                    1,
                ),
                prop_status::NONE,
            ),
            ("Linearize".to_string(), Property::Bool(false), prop_status::NONE),
            placement_property_entry(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Sweep".to_string(), format!("Part sweep {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<Shape, String> {
            let sections = link_list_property(doc, id, "Sections");
            if sections.is_empty() {
                return Err("No sections linked.".to_string());
            }
            let spine_link = link_sub_property(doc, id, "Spine")
                .ok_or_else(|| "No spine".to_string())?;
            let spine = sweep_spine(doc, &spine_link)?;
            let profiles = sections
                .iter()
                .map(|name| linked_shape(doc, &(name.clone(), Vec::new())))
                .collect::<Result<Vec<_>, _>>()?;
            let solid = bool_property(doc, id, "Solid", true);
            let frenet = bool_property(doc, id, "Frenet", true);
            let profile_shapes = if solid {
                profiles
                    .iter()
                    .map(sweep_section_face)
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                profiles
            };
            let mut shape = surface_ops::make_pipe_shell(&spine, &profile_shapes, solid, frenet)
                .map_err(|error| error.to_string())?;
            if solid {
                shape = single_solid(shape)?;
            }
            apply_object_placement(doc, id, shape)
        })();
        store(doc, id, result);
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyShell>()?;
    m.add_class::<PySolid>()?;
    // `Cone`, `Toroid` and `BSplineSurface` are also plausible members of the
    // analytic half of `Part`; only add ours when the module does not already
    // define them, so the two halves can grow independently.
    if !m.hasattr("Cone")? {
        m.add_class::<PyCone>()?;
    }
    if !m.hasattr("Toroid")? {
        m.add_class::<PyToroid>()?;
    }
    if !m.hasattr("BSplineSurface")? {
        m.add_class::<PyBSplineSurface>()?;
    }
    m.add_function(wrap_pyfunction!(makePlane, m)?)?;
    m.add_function(wrap_pyfunction!(insert, m)?)?;
    m.add_class::<PyPlaneSurface>()?;
    let precision = PyModule::new(m.py(), "Part.Precision")?;
    precision.add_function(wrap_pyfunction!(confusion, &precision)?)?;
    precision.add_function(wrap_pyfunction!(angular, &precision)?)?;
    precision.add_function(wrap_pyfunction!(infinite, &precision)?)?;
    precision.add_function(wrap_pyfunction!(isInfinite, &precision)?)?;
    m.add("Precision", &precision)?;
    m.add_function(wrap_pyfunction!(makeShell, m)?)?;
    m.add_function(wrap_pyfunction!(makeShellFromWires, m)?)?;
    m.add_function(wrap_pyfunction!(makeSolid, m)?)?;
    m.add_function(wrap_pyfunction!(makeLoft, m)?)?;
    m.add_function(wrap_pyfunction!(makeRuledSurface, m)?)?;
    m.add_function(wrap_pyfunction!(makeFilledFace, m)?)?;
    m.add_function(wrap_pyfunction!(makeTorus, m)?)?;
    m.add_function(wrap_pyfunction!(makeWedge, m)?)?;
    m.add_function(wrap_pyfunction!(makeRevolution, m)?)?;
    m.add_function(wrap_pyfunction!(makeTube, m)?)?;
    m.add_function(wrap_pyfunction!(makeThread, m)?)?;
    // `curves::register` runs first and owns the ordinary makers; this override
    // adds the segmentation dispatch `FaceMakerBuildFace` needs.  Keep it last so
    // the module attribute is the one registered here.
    m.add_function(wrap_pyfunction!(makeFace, m)?)?;
    install_sketch_methods(m)?;
    Ok(())
}
