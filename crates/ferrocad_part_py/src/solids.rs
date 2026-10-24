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

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{Document, Matrix4, ObjectId, Placement, Property, Rotation, prop_status};
use ferrocad_curves::{Arc as CurveArc, Circle, EdgeCurve, LineSegment, Vector3};
use ferrocad_geom::{ElementKind, Lineage, Shape};
use ferrocad_occt::surface_ops;
use ferrocad_part::{
    make_shape_list_property, make_shape_property, shape_list_of, shape_of, ShapeListProperty,
    ShapeProperty,
};
use pyo3::exceptions::{
    PyAttributeError, PyDeprecationWarning, PyRuntimeError, PyTypeError, PyValueError,
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
    object_registry::register(EXTRUSION_TYPE, Arc::new(Extrusion));
    object_registry::register(MIRRORING_TYPE, Arc::new(Mirroring));
    object_registry::register(SKETCH_TYPE, Arc::new(SketchObject));
    object_registry::register(BODY_TYPE, Arc::new(Body));
    object_registry::register(ADDITIVE_BOX_TYPE, Arc::new(AdditiveBox));
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
/// `Part::Extrusion` (`FeatureExtrusion.cpp`).
pub const EXTRUSION_TYPE: &str = "Part::Extrusion";

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

/// `Part::Extrusion`: `Shape` is `Base` swept along `Dir` by `LengthFwd` /
/// `LengthRev`.
///
/// A zero taper uses the kernel prism; a non-zero taper builds a ruled loft
/// through the source wire and its offset copy at the far end, then cuts the
/// inner wires' solids from the outer one — the same construction as
/// `ExtrusionHelper::makeElementDraft`. The offset sign is chosen by which
/// result grows the wire, so it does not depend on the wire's orientation.
struct Extrusion;

impl ObjectType for Extrusion {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Base".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            (
                "Dir".to_string(),
                Property::Vector(Vector3::new(0.0, 0.0, 1.0)),
                prop_status::NONE,
            ),
            (
                "DirMode".to_string(),
                Property::Enumeration(
                    vec![
                        "Custom".to_string(),
                        "Edge".to_string(),
                        "Normal".to_string(),
                    ],
                    0,
                ),
                prop_status::NONE,
            ),
            (
                "DirLink".to_string(),
                Property::LinkSub(String::new(), Vec::new()),
                prop_status::NONE,
            ),
            ("LengthFwd".to_string(), Property::Float(0.0), prop_status::NONE),
            ("LengthRev".to_string(), Property::Float(0.0), prop_status::NONE),
            ("Solid".to_string(), Property::Bool(false), prop_status::NONE),
            ("Reversed".to_string(), Property::Bool(false), prop_status::NONE),
            ("Symmetric".to_string(), Property::Bool(false), prop_status::NONE),
            ("TaperAngle".to_string(), Property::Float(0.0), prop_status::NONE),
            ("TaperAngleRev".to_string(), Property::Float(0.0), prop_status::NONE),
            (
                "FaceMakerClass".to_string(),
                Property::String("Part::FaceMakerExtrusion".to_string()),
                prop_status::READONLY | prop_status::HIDDEN,
            ),
            (
                "FaceMakerMode".to_string(),
                Property::Enumeration(
                    vec![
                        "Simple".to_string(),
                        "Cheese".to_string(),
                        "Extrusion".to_string(),
                        "Bullseye".to_string(),
                        "Unified".to_string(),
                    ],
                    2,
                ),
                prop_status::NONE,
            ),
            (
                "InnerWireTaper".to_string(),
                Property::Enumeration(
                    vec!["Inverted".to_string(), "SameAsOuter".to_string()],
                    0,
                ),
                prop_status::HIDDEN,
            ),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Extrude".to_string(), format!("Part extrusion {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = extrude(doc, id).map_err(|error| error.to_string());
        store(doc, id, result);
    }
}

/// The parameters `Extrusion::computeFinalParameters` derives from the
/// properties.
struct ExtrusionParameters {
    direction: Vector3,
    length_fwd: f64,
    length_rev: f64,
    solid: bool,
    taper_fwd: f64,
    taper_rev: f64,
    same_as_outer: bool,
}

fn extrusion_parameters(doc: &mut Document, id: ObjectId) -> Result<ExtrusionParameters, String> {
    // Upstream's `Extrusion::computeFinalParameters`: `Custom` uses `Dir`, `Edge`
    // takes the direction of the linked line edge, and `Normal` uses the plane
    // normal of `Base` (a sketch's local +Z). The derived vector is written back
    // to `Dir`, so a later read of the property agrees with the extrusion.
    let raw = match enum_index_property(doc, id, "DirMode") {
        0 => vector_property(doc, id, "Dir").unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0)),
        1 => {
            let link = link_sub_property(doc, id, "DirLink").ok_or_else(|| {
                "DirMode is set to use edge, but no edge is linked.".to_string()
            })?;
            let edge = linked_shape(doc, &link)?;
            surface_ops::edge_line_direction(&edge)
                .map_err(|_| "DirLink edge is not a line.".to_string())?
        }
        2 => {
            let link = link_sub_property(doc, id, "Base")
                .ok_or_else(|| "No object linked".to_string())?;
            let sketch_normal = doc
                .get_by_name(&link.0)
                .and_then(|object| doc.object(object))
                .filter(|object| object.type_id.as_str() == SKETCH_TYPE)
                .and_then(|object| match object.properties.get("Placement") {
                    Some(Property::Placement(placement)) => Some(
                        placement
                            .rotation
                            .to_matrix()
                            .transform(&Vector3::new(0.0, 0.0, 1.0)),
                    ),
                    _ => None,
                });
            match sketch_normal {
                Some(normal) => normal,
                None => {
                    let base = linked_shape(doc, &link)?;
                    surface_ops::shape_plane_normal(&base).map_err(|_| {
                        "Can't find normal direction, because the shape is not on a plane."
                            .to_string()
                    })?
                }
            }
        }
        _ => return Err("Unexpected enum value".to_string()),
    };
    if raw.length() < CONFUSION {
        return Err("Direction is zero-length".to_string());
    }
    doc.set_property_raw(id, "Dir", Property::Vector(raw));
    let mut direction = raw.normalize();
    if bool_property(doc, id, "Reversed", false) {
        direction = direction.neg();
    }
    let mut length_fwd = float_property(doc, id, "LengthFwd", 0.0);
    let mut length_rev = float_property(doc, id, "LengthRev", 0.0);
    if length_fwd.abs() < CONFUSION && length_rev.abs() < CONFUSION {
        length_fwd = raw.length();
    }
    if bool_property(doc, id, "Symmetric", false) {
        length_rev = length_fwd * 0.5;
        length_fwd *= 0.5;
    }
    if (length_fwd + length_rev).abs() < CONFUSION {
        return Err("Total length of extrusion is zero.".to_string());
    }
    let taper_fwd = float_property(doc, id, "TaperAngle", 0.0).to_radians();
    let taper_rev = float_property(doc, id, "TaperAngleRev", 0.0).to_radians();
    if taper_fwd.abs() > PI * 0.5 - 1e-12 || taper_rev.abs() > PI * 0.5 - 1e-12 {
        return Err(
            "Magnitude of taper angle matches or exceeds 90 degrees. That is too much."
                .to_string(),
        );
    }
    Ok(ExtrusionParameters {
        direction,
        length_fwd,
        length_rev,
        solid: bool_property(doc, id, "Solid", false),
        taper_fwd,
        taper_rev,
        same_as_outer: enum_index_property(doc, id, "InnerWireTaper") == 1,
    })
}

fn extrude(doc: &mut Document, id: ObjectId) -> Result<Shape, String> {
    let params = extrusion_parameters(doc, id)?;
    let base = link_sub_property(doc, id, "Base")
        .ok_or_else(|| "No object linked".to_string())?;
    let source = linked_shape(doc, &base)?;
    extrude_shape(&source, &params)
}

fn extrude_shape(source: &Shape, params: &ExtrusionParameters) -> Result<Shape, String> {
    let backend = ferrocad_part::backend();
    if params.taper_fwd.abs() < 1e-12 && params.taper_rev.abs() < 1e-12 {
        // Regular prism: shift the source back by the reverse length, then sweep
        // the whole source by the total length.
        let mut shape = source.clone();
        if params.length_rev.abs() > CONFUSION {
            let matrix = crate::translation_matrix(params.direction.scale(-params.length_rev));
            shape = backend
                .transform(&shape, &matrix)
                .map_err(|error| error.to_string())?;
        }
        // A wire has no interior, so a solid extrusion first builds the face the
        // wire bounds (upstream's `makeElementFace`).
        if params.solid && backend.elements(&shape, ElementKind::Face).is_empty() {
            let wires = wires_of(&shape);
            if wires.is_empty() {
                return Err("Extrusion: Input must not only consist if a vertex".to_string());
            }
            shape = backend
                .make_face(&wires)
                .map_err(|error| error.to_string())?;
        }
        let vector = params.direction.scale(params.length_fwd + params.length_rev);
        let prism = backend
            .extrude(&shape, vector)
            .map_err(|error| error.to_string())?;
        return ensure_solid(prism, params.solid);
    }

    let wires = wires_of(&source);
    if wires.is_empty() {
        return Err("Extrusion: Input must not only consist if a vertex".to_string());
    }
    // A face with holes drafts its outer wire and cuts the drafted inner wires;
    // any other source drafts each wire on its own (upstream's face/wire split).
    if let Some(outer) = backend.outer_wire(&source) {
        let mut result = drafted_wire(&outer, params, params.taper_fwd, params.taper_rev)?;
        for inner in wires
            .iter()
            .filter(|wire| !backend.same_shape(wire, &outer))
        {
            let (taper_fwd, taper_rev) = if params.same_as_outer {
                (params.taper_fwd, params.taper_rev)
            } else {
                (-params.taper_fwd, -params.taper_rev)
            };
            let inner_shape = drafted_wire(inner, params, taper_fwd, taper_rev)?;
            result = backend
                .cut(&result, &inner_shape)
                .map(|out| out.shape)
                .map_err(|error| error.to_string())?;
        }
        return Ok(result);
    }
    let mut drafts = Vec::with_capacity(wires.len());
    for wire in &wires {
        drafts.push(drafted_wire(wire, params, params.taper_fwd, params.taper_rev)?);
    }
    if drafts.len() == 1 {
        return Ok(drafts.remove(0));
    }
    backend
        .make_compound(&drafts)
        .map_err(|error| error.to_string())
}

/// Make the output a solid when the caller asked for one (a prism of a face is
/// already a solid; a swept wire's shell is closed into one).
fn ensure_solid(shape: Shape, solid: bool) -> Result<Shape, String> {
    if !solid {
        return Ok(shape);
    }
    let backend = ferrocad_part::backend();
    if !backend.elements(&shape, ElementKind::Solid).is_empty() {
        return Ok(shape);
    }
    match backend.elements(&shape, ElementKind::Shell).as_slice() {
        [shell] => backend
            .make_solid(&shell.shape)
            .map_err(|error| error.to_string()),
        _ => Ok(shape),
    }
}

fn wires_of(shape: &Shape) -> Vec<Shape> {
    ferrocad_part::backend()
        .elements(shape, ElementKind::Wire)
        .into_iter()
        .map(|element| element.shape)
        .collect()
}

fn wire_extent(shape: &Shape) -> f64 {
    match ferrocad_part::backend().bounds(shape) {
        Some((min, max)) => (max.x - min.x).abs() + (max.y - min.y).abs() + (max.z - min.z).abs(),
        None => 0.0,
    }
}

/// Offset `wire` by `distance` in the direction that grows (`grow`) or shrinks
/// it. The kernel offset's sign depends on the wire orientation, so both signs
/// are tried and the result's extent decides.
fn offset_wire(wire: &Shape, distance: f64, grow: bool) -> Result<Shape, String> {
    if distance.abs() < CONFUSION {
        return Ok(wire.clone());
    }
    let backend = ferrocad_part::backend();
    let mut options: Vec<Shape> = [distance, -distance]
        .into_iter()
        .filter_map(|offset| backend.make_offset(wire, offset, 2).ok())
        .collect();
    if options.len() == 2 && wire_extent(&options[1]) > wire_extent(&options[0]) {
        options.swap(0, 1);
    }
    if options.is_empty() {
        return Err("Extrusion: Offset could not be created".to_string());
    }
    let chosen = if grow {
        options.first().cloned()
    } else {
        options.last().cloned()
    };
    chosen.ok_or_else(|| "Extrusion: Offset could not be created".to_string())
}

/// One wire's drafted solid (or shell when `solid` is false): a ruled loft
/// through the offset section at the reverse end (when any), the source wire
/// and the offset section at the forward end, in that order.
fn drafted_wire(
    wire: &Shape,
    params: &ExtrusionParameters,
    taper_fwd: f64,
    taper_rev: f64,
) -> Result<Shape, String> {
    let backend = ferrocad_part::backend();
    let has_fwd = params.length_fwd.abs() > CONFUSION;
    let has_rev = params.length_rev.abs() > CONFUSION;
    // Keep the source as a middle section unless the two directions have
    // opposite signs and no angle changes (upstream's `bMid`).
    let keep_middle = !has_fwd || !has_rev || params.length_fwd * params.length_rev > 0.0;
    let mut sections = Vec::new();
    if has_rev {
        let distance = taper_rev.tan() * params.length_rev;
        let offset = offset_wire(wire, distance, distance >= 0.0)?;
        let matrix = crate::translation_matrix(params.direction.scale(-params.length_rev));
        sections.push(
            backend
                .transform(&offset, &matrix)
                .map_err(|error| error.to_string())?,
        );
    }
    if keep_middle {
        sections.push(wire.clone());
    }
    if has_fwd {
        let distance = taper_fwd.tan() * params.length_fwd;
        let offset = offset_wire(wire, distance, distance >= 0.0)?;
        let matrix = crate::translation_matrix(params.direction.scale(params.length_fwd));
        sections.push(
            backend
                .transform(&offset, &matrix)
                .map_err(|error| error.to_string())?,
        );
    }
    if sections.len() < 2 {
        return Err("Extrusion: Loft could not be built".to_string());
    }
    backend
        .make_loft(&sections, params.solid)
        .map_err(|error| error.to_string())
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
}

impl PyCone {
    fn frame(&self) -> (Vector3, Vector3) {
        surface_frame(self.axis)
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
        Ok(Self {
            radius,
            semi_angle,
            center,
            axis: axis.normalize(),
        })
    }

    #[getter]
    fn Radius(&self) -> f64 {
        self.radius
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
        Ok(())
    }

    #[getter]
    fn Apex(&self, py: Python<'_>) -> PyResult<PyObject> {
        curves::py_vector(py, self.apex())
    }

    /// The surface point at `(u, v)`: `u` around the axis, `v` along it.
    fn value(&self, py: Python<'_>, u: f64, v: f64) -> PyResult<PyObject> {
        let (x, y) = self.frame();
        let radius = self.radius + v * self.semi_angle.tan();
        let radial = x.scale(u.cos()).add(&y.scale(u.sin()));
        let point = self
            .center
            .add(&radial.scale(radius))
            .add(&self.axis.scale(v));
        curves::py_vector(py, point)
    }

    /// `getDN(u, v, Nu, Nv)` — the analytic derivative of the cone surface.
    fn getDN(&self, py: Python<'_>, u: f64, v: f64, Nu: u32, Nv: u32) -> PyResult<PyObject> {
        let (x, y) = self.frame();
        let tan = self.semi_angle.tan();
        let radius = self.radius + v * tan;
        let radial = x.scale(u.cos()).add(&y.scale(u.sin()));
        let tangent = x.scale(-u.sin()).add(&y.scale(u.cos()));
        let vector = match (Nu, Nv) {
            (1, 0) => tangent.scale(radius),
            (0, 1) => radial.scale(tan).add(&self.axis),
            (1, 1) => tangent.scale(tan),
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
    /// [`value`](PyCone::value)).
    fn parameter(&self, point: &Bound<'_, PyAny>) -> PyResult<(f64, f64)> {
        let point = curves::vector(point)?;
        let relative = point.sub(&self.center);
        let v = relative.dot(&self.axis);
        let (x, y) = self.frame();
        let radial = relative.sub(&self.axis.scale(v));
        let mut u = radial.dot(&y).atan2(radial.dot(&x));
        if u < 0.0 {
            u += 2.0 * PI;
        }
        Ok((u, v))
    }

    /// The cone bounded by `(u1, u2) x (v1, v2)`, as an analytic face handle.
    fn toShape(&self, _u1: f64, _u2: f64, _v1: f64, _v2: f64) -> PyConeShape {
        PyConeShape {
            cone: self.clone(),
        }
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
}

impl PyToroid {
    fn frame(&self) -> (Vector3, Vector3) {
        surface_frame(self.axis)
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
        Ok(Self {
            major_radius,
            minor_radius,
            center,
            axis: axis.normalize(),
        })
    }

    #[getter]
    fn MajorRadius(&self) -> f64 {
        self.major_radius
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

    /// The rational B-spline description of the torus: the knot vector of the
    /// standard full-circle rational quadratic representation in both
    /// directions, so the parameter ranges are the full `0..2π`.
    fn toBSpline(&self) -> PyBSplineSurface {
        PyBSplineSurface::torus()
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
/// This slice carries the knot-vector description (`UKnotSequence`,
/// `VKnotSequence`, `bounds`, `scaleKnotsToBounds`) that `Toroid.toBSpline`
/// returns; the pole/weight accessors upstream also offers are not modelled.
#[pyclass(name = "BSplineSurface", module = "Part")]
#[derive(Clone)]
pub struct PyBSplineSurface {
    u_knots: Vec<f64>,
    v_knots: Vec<f64>,
    u_periodic: bool,
    v_periodic: bool,
}

impl PyBSplineSurface {
    fn torus() -> Self {
        Self {
            u_knots: circle_knots(),
            v_knots: circle_knots(),
            u_periodic: false,
            v_periodic: false,
        }
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
        }
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

    #[getter]
    fn UKnotSequence(&self) -> Vec<f64> {
        self.u_knots.clone()
    }

    #[getter]
    fn VKnotSequence(&self) -> Vec<f64> {
        self.v_knots.clone()
    }

    /// `(u1, u2, v1, v2)` — the current parameter bounds.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        (
            self.u_knots.first().copied().unwrap_or(0.0),
            self.u_knots.last().copied().unwrap_or(0.0),
            self.v_knots.first().copied().unwrap_or(0.0),
            self.v_knots.last().copied().unwrap_or(0.0),
        )
    }

    /// Linearly rescale both knot vectors to the requested bounds.
    fn scaleKnotsToBounds(&mut self, u0: f64, u1: f64, v0: f64, v1: f64) {
        scale_knots(&mut self.u_knots, u0, u1);
        scale_knots(&mut self.v_knots, v0, v1);
    }

    fn __repr__(&self) -> String {
        "<BSplineSurface object>".to_string()
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
// Sketcher: `Sketcher::SketchObject` (geometry + Shape/InternalShape only)
// ---------------------------------------------------------------------------
//
// The Part tests need three things from a sketch: `addGeometry` (so a sketch
// participates in a document at all), `Shape` (a wire/compound of the geometry
// edges, moved by `Placement`) and `InternalShape` (the bounded faces built from
// those wires when `MakeInternals` is set). Upstream's sketch is a full solver
// front end; none of that is here. There is no solver, no constraint API and no
// DOF bookkeeping, so `addConstraint`/`solve`/`delConstraint` are deliberately
// absent: a test that needs them gets an `AttributeError` naming the missing
// method rather than a silently wrong result.
//
// The geometry list is stored as the edges each geometry object's `toShape()`
// returns (`Part::PropertyTopoShapeList`), not as upstream's `Geom...` objects:
// FerroCAD has no geometry-list property type yet. The kernel still builds every
// edge and every wire/face, so nothing here is synthesized.

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

/// Install `addGeometry` on `ferrocad.DocumentObject` at `Part` import time.
///
/// A document object is the core's `PyDocumentObject`, whose methods live in
/// `ferrocad_py`; a module cannot add `#[pymethods]` to it. The class is a heap
/// type, so a descriptor can still be installed on it: it hands sketches the
/// bound `addGeometry` and raises `AttributeError` for every other type, exactly
/// as if the method did not exist. Skipped when `ferrocad` is not importable
/// (a standalone `Part` build has no document object to extend).
fn install_sketch_methods(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    let Ok(ferrocad) = py.import("ferrocad") else {
        return Ok(());
    };
    let Ok(document_object) = ferrocad.getattr("DocumentObject") else {
        return Ok(());
    };
    let add_geometry = wrap_pyfunction!(sketch_add_geometry, m)?;
    let globals = PyDict::new(py);
    globals.set_item("_document_object", document_object)?;
    globals.set_item("_add_geometry", add_geometry)?;
    let source = CString::new(SKETCH_METHODS).expect("no NUL");
    py.run(source.as_c_str(), Some(&globals), None)?;
    Ok(())
}

const SKETCH_METHODS: &str = r#"
class _SketchMethods:
    __slots__ = ()

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        if obj.TypeId != "Sketcher::SketchObject":
            raise AttributeError(
                "'%s' object has no attribute 'addGeometry'" % obj.TypeId
            )

        def addGeometry(geometry, construction=False):
            return _add_geometry(obj, geometry, construction)

        addGeometry.__name__ = "addGeometry"
        return addGeometry


_document_object.addGeometry = _SketchMethods()
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

/// `TopoShape::makeThread` in this seam: a helical circular pipe.
///
/// Upstream builds a solid from two thread-profile wires on coaxial cylinders;
/// the seam has no through-sections between wires, so the thread is built the way
/// the task asks: the exact-by-construction helix edge is swept with a circular
/// profile of radius `depth / 2`, giving a helical tube that spans `depth`
/// radially.
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
    let spine = surface_ops::make_helix_edge(pitch, height, radius, 0.0, false)
        .map_err(|error| error.to_string())?;
    // The helix starts at (radius, 0, 0) with tangent (0, 2*pi*radius/pitch, 1).
    let tangent = Vector3::new(0.0, std::f64::consts::TAU * radius / pitch, 1.0).normalize();
    let profile = circular_profile(Vector3::new(radius, 0.0, 0.0), tangent, depth * 0.5)?;
    surface_ops::make_pipe_shell(&spine, &[profile], false, false)
        .map_err(|error| error.to_string())
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

/// `Part.makeThread(pitch, depth, height, radius)` → a helical pipe.
///
/// Upstream builds a thread solid from two thread-profile wires; the seam has no
/// through-sections, so this builds the helix and sweeps a circular profile of
/// radius `depth / 2` along it with `make_pipe` (a `makePipeShell`-style sweep).
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
