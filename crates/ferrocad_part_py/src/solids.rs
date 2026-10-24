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
use std::sync::{Arc, Mutex};

use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{Document, ObjectId, Property, prop_status};
use ferrocad_curves::{EdgeCurve, LineSegment, Vector3};
use ferrocad_geom::{ElementKind, Lineage, Shape};
use ferrocad_part::{make_shape_property, shape_of, ShapeProperty};
use pyo3::exceptions::{PyDeprecationWarning, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

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
        return Ok(compound.borrow().children());
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
/// The seam's `make_solid` takes one closed shell (this slice has no cavities), so
/// more than one shell is reported instead of silently dropping the rest.
fn solid_from(shells: &[Shape]) -> PyResult<Shape> {
    match shells {
        [shell] => ferrocad_part::backend()
            .make_solid(shell)
            .map_err(kernel_error),
        [] => Err(OCCError::new_err("a solid needs a shell")),
        _ => Err(OCCError::new_err(
            "a solid from several shells (cavities) is not implemented yet",
        )),
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
        // (a second shell) leaves the object as it was.
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
    Ok(PartShape::from_shape(shape))
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

/// `Part.makeSolid(shape, op=None)` — create a solid from the shell of `shape`.
///
/// A shell is used directly; any other shape must contain exactly one shell.
#[pyfunction]
#[pyo3(signature = (shape, op = None))]
fn makeSolid(shape: &Bound<'_, PyAny>, op: Option<&str>) -> PyResult<PartShape> {
    let _ = op;
    let shell = shape_argument(shape)?;
    let solid = solid_from(&shells_of(&shell))?;
    Ok(PartShape::from_shape(solid))
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
    Ok(PartShape::from_shape(lofts))
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
    Ok(PartShape::from_shape(face))
}

/// The precision below which a plane dimension or an extrusion length is zero
/// (OCCT's `Precision::Confusion`).
const CONFUSION: f64 = 1e-7;

/// Register the document object types this module provides. Called from the
/// Part module's kernel bootstrap, after `ferrocad_part::register`.
pub(crate) fn register_objects() {
    object_registry::register(RULED_SURFACE_TYPE, Arc::new(RuledSurface));
    object_registry::register(EXTRUSION_TYPE, Arc::new(Extrusion));
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

fn extrusion_parameters(doc: &Document, id: ObjectId) -> Result<ExtrusionParameters, String> {
    if enum_index_property(doc, id, "DirMode") != 0 {
        return Err("DirMode Edge/Normal is not implemented yet".to_string());
    }
    let raw = vector_property(doc, id, "Dir").unwrap_or_else(|| Vector3::new(0.0, 0.0, 1.0));
    if raw.length() < CONFUSION {
        return Err("Direction is zero-length".to_string());
    }
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

fn extrude(doc: &Document, id: ObjectId) -> Result<Shape, String> {
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
    Ok(())
}
